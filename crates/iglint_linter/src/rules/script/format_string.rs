//! SCR112 format-string-mismatch.

use iglint_python::ast::{Expr, Operator, StringValue};
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR112",
    name: "format-string-mismatch",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "String formatting that fails at run time",
    explanation: r#"## What it does

Checks string literals formatted with `%` or `.format(...)` against their
arguments, with Jython 2.7's rules (verified on Jython):

* `%`: unsupported conversion characters (`%q`, Python 3's `%a`), a
  trailing `%`, a tuple with too many or too few values for the
  placeholders (`*` widths count), `%(name)s` with a tuple or a single
  value instead of a dict, and `%(name)s` keys missing from a dict literal.
  A single non-tuple literal counts as one value.
* `str.format`: a single `{` or `}`, mixing `{}` and `{0}`, conversions
  other than `!r` and `!s`, positional fields beyond the arguments given,
  and keyword fields that are not passed.

Only literal format strings with literal arguments (or literal argument
lists) are checked; anything computed, `*args` and `**kwargs` are left
alone. Extra arguments to `.format` are allowed (Python ignores them).

## Why is this bad?

The expression raises `TypeError`, `ValueError`, `KeyError` or
`IndexError` when it runs, typically while building a log message or an
alarm text, where the failure hides the original problem.

## Example

```python
logger.info("Line %s stopped at %s" % (line,))
label = "{} of {}".format(done)
```

Use instead:

```python
logger.info("Line %s stopped at %s" % (line, stopTime))
label = "{} of {}".format(done, total)
```

## pylint

Corresponds to `E1300`-`E1306` and `E1310`-`E1311`/`W1303`-`W1307`
(format errors and argument mismatches). In Jython a dict on the right
of `%` satisfies any number of positional placeholders, so iglint does
not report `"%s %s" % {...}` as pylint does.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&FormatString)],
    options: None,
};

struct FormatString;

/// What a `%` format string needs.
#[derive(Debug, Default, PartialEq)]
struct PercentSpec {
    /// Values consumed by positional placeholders and `*`.
    positional: usize,
    /// `%(key)` names.
    keys: Vec<String>,
}

/// Why a format string cannot be checked.
#[derive(Debug, PartialEq)]
enum Bad {
    /// Formatting raises whatever the arguments are.
    Error(String),
    /// The string uses something we do not model exactly (non-ASCII
    /// digits, which Java's `Character.isDigit` accepts): no finding.
    Unknown,
}

impl From<&str> for Bad {
    fn from(s: &str) -> Self {
        Bad::Error(s.to_owned())
    }
}

impl From<String> for Bad {
    fn from(s: String) -> Self {
        Bad::Error(s)
    }
}

/// A non-ASCII character Java may take for a digit: we cannot tell.
fn maybe_java_digit(c: char) -> bool {
    !c.is_ascii() && c.is_numeric()
}

/// Parses a `%` format string like Jython's `StringFormatter.format`
/// (verified on Jython): `%`, an optional `(key)` with nested parentheses,
/// any flags of `-+ #0`, a width (digits or `*`), a precision (`.` and
/// digits or `*`), at most one length modifier of `hlL`, the conversion.
/// `%%` (also `%5%`, `%*%`) prints `%` but still consumes `*` values.
fn parse_percent(s: &str) -> Result<PercentSpec, Bad> {
    let chars: Vec<char> = s.chars().collect();
    let mut spec = PercentSpec::default();
    let mut i = 0;
    let pop = |i: &mut usize| -> Result<char, Bad> {
        let c = *chars.get(*i).ok_or(Bad::from("incomplete format"))?;
        *i += 1;
        Ok(c)
    };
    // `getNumber`: `*` (one value), digits, or nothing.
    let number = |i: &mut usize, stars: &mut usize| -> Result<(), Bad> {
        let c = pop(i)?;
        if c == '*' {
            *stars += 1;
            return Ok(());
        }
        if maybe_java_digit(c) {
            return Err(Bad::Unknown);
        }
        if !c.is_ascii_digit() {
            *i -= 1;
            return Ok(());
        }
        let start = *i - 1;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        if chars.get(*i).is_some_and(|c| maybe_java_digit(*c)) {
            return Err(Bad::Unknown);
        }
        let digits: String = chars[start..*i].iter().collect();
        if digits.parse::<i32>().is_err() {
            return Err(Bad::Error(format!(
                "`{digits}` does not fit in an int (Jython raises NumberFormatException)"
            )));
        }
        Ok(())
    };
    while i < chars.len() {
        if chars[i] != '%' {
            i += 1;
            continue;
        }
        i += 1;
        let mut keyed = false;
        if pop(&mut i)? == '(' {
            let key_start = i;
            let mut depth = 1;
            while depth > 0 {
                match pop(&mut i)? {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            spec.keys.push(chars[key_start..i - 1].iter().collect());
            keyed = true;
        } else {
            i -= 1;
        }
        while matches!(pop(&mut i)?, '-' | '+' | ' ' | '#' | '0') {}
        i -= 1;
        let mut stars = 0;
        number(&mut i, &mut stars)?;
        let mut c = pop(&mut i)?;
        if c == '.' {
            number(&mut i, &mut stars)?;
            c = pop(&mut i)?;
        }
        if matches!(c, 'h' | 'l' | 'L') {
            c = pop(&mut i)?;
        }
        match c {
            // Prints `%`; `*` values are still taken.
            '%' => spec.positional += stars,
            'd' | 'i' | 'o' | 'u' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'c' | 'r'
            | 's' => {
                spec.positional += stars + usize::from(!keyed);
            }
            other => {
                if maybe_java_digit(other) {
                    return Err(Bad::Unknown);
                }
                // Java's index counts UTF-16 code units.
                let index: usize = chars[..i - 1].iter().map(|c| c.len_utf16()).sum();
                return Err(Bad::Error(format!(
                    "unsupported format character '{other}' (0x{:x}) at index {index}",
                    other as u32,
                )));
            }
        }
    }
    Ok(spec)
}

/// What a `str.format` string needs.
#[derive(Debug, Default, PartialEq)]
struct BraceSpec {
    /// Highest positional index used, plus one.
    positional: usize,
    keywords: Vec<String>,
}

/// Parses a `str.format` string like Jython's `MarkupIterator` and
/// `FieldNameIterator` (verified on Jython).
fn parse_braces(s: &str) -> Result<BraceSpec, Bad> {
    let mut spec = BraceSpec::default();
    let mut auto: Option<bool> = None;
    let mut next_auto = 0usize;
    parse_braces_into(s, &mut spec, &mut auto, &mut next_auto, 0)?;
    Ok(spec)
}

fn parse_braces_into(
    s: &str,
    spec: &mut BraceSpec,
    auto: &mut Option<bool>,
    next_auto: &mut usize,
    depth: u32,
) -> Result<(), Bad> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => i += 2,
            '}' if chars.get(i + 1) == Some(&'}') => i += 2,
            '}' => return Err("Single '}' encountered in format string".into()),
            '{' => {
                // Find the matching close brace (nested fields in the spec;
                // brackets do not protect braces).
                let mut level = 1;
                let mut j = i + 1;
                while j < chars.len() && level > 0 {
                    match chars[j] {
                        '{' => level += 1,
                        '}' => level -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                if level > 0 {
                    return Err("Single '{' encountered in format string".into());
                }
                let field: String = chars[i + 1..j - 1].iter().collect();
                field_spec(&field, spec, auto, next_auto, depth)?;
                i = j;
            }
            _ => i += 1,
        }
    }
    Ok(())
}

fn field_spec(
    field: &str,
    spec: &mut BraceSpec,
    auto: &mut Option<bool>,
    next_auto: &mut usize,
    depth: u32,
) -> Result<(), Bad> {
    // field_name [!conversion] [:format_spec]: Jython ends the name at the
    // first `!` or `:`, also inside `[...]`.
    let name_end = field.find(['!', ':']).unwrap_or(field.len());
    let name = &field[..name_end];
    let first_end = name.find(['.', '[']).unwrap_or(name.len());
    let head = &name[..first_end];
    if head.chars().any(maybe_java_digit) {
        return Err(Bad::Unknown);
    }
    // Numbering is manual when the name starts with a digit, automatic
    // when it is empty; `{+0}` and `{-0}` are neither.
    if head.is_empty() {
        if *auto == Some(false) {
            return Err(
                "cannot switch from manual field specification to automatic field numbering".into(),
            );
        }
        *auto = Some(true);
        *next_auto += 1;
        spec.positional = spec.positional.max(*next_auto);
    } else if head.starts_with(|c: char| c.is_ascii_digit()) {
        if *auto == Some(true) {
            return Err(
                "cannot switch from automatic field numbering to manual field specification".into(),
            );
        }
        *auto = Some(false);
    }
    // `Integer.parseInt`: an index when it fits in an int, else a keyword.
    if !head.is_empty() {
        let digits = head.strip_prefix(['+', '-']).unwrap_or(head);
        let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
        match head.parse::<i32>() {
            Ok(index) if numeric => match usize::try_from(index) {
                Ok(index) => spec.positional = spec.positional.max(index.saturating_add(1)),
                // A negative index fails with a Java exception, or not.
                Err(_) => return Err(Bad::Unknown),
            },
            Err(_) if numeric && !head.starts_with(['+', '-']) => {
                return Err(format!(
                    "field `{{{head}}}` is too large for an index, so it is looked up as a keyword (KeyError)"
                )
                .into());
            }
            _ => {
                if !spec.keywords.iter().any(|k| k == head) {
                    spec.keywords.push(head.to_owned());
                }
            }
        }
    }
    field_rest(&name[first_end..])?;
    let mut rest = &field[name_end..];
    if let Some(r) = rest.strip_prefix('!') {
        let mut chars = r.chars();
        match chars.next() {
            Some('r' | 's') => {}
            Some(c) => return Err(format!("Unknown conversion specifier {c}").into()),
            None => return Err("end of format while looking for conversion specifier".into()),
        }
        rest = chars.as_str();
        if !rest.is_empty() && !rest.starts_with(':') {
            return Err("expected ':' after conversion specifier".into());
        }
    }
    if let Some(format_spec) = rest.strip_prefix(':')
        && format_spec.contains('{')
    {
        if depth >= 1 {
            return Err("Max string recursion exceeded".into());
        }
        parse_braces_into(format_spec, spec, auto, next_auto, depth + 1)?;
    }
    Ok(())
}

/// The `.attr` and `[key]` parts after a field's first name, like
/// `FieldNameIterator`.
fn field_rest(mut rest: &str) -> Result<(), Bad> {
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            if end == 0 {
                return Err("Empty attribute in format string".into());
            }
            rest = &r[end..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else {
                return Err("Missing ']' in format string".into());
            };
            if end == 0 {
                return Err("Empty attribute in format string".into());
            }
            rest = &r[end + 1..];
            if !rest.is_empty() && !rest.starts_with(['.', '[']) {
                return Err("Only '.' or '[' may follow ']' in format field specifier".into());
            }
        } else {
            return Ok(());
        }
    }
    Ok(())
}

fn string_of(e: &Expr) -> Option<String> {
    match e {
        Expr::Str(s) => match &s.s {
            StringValue::Unicode(u) => Some(u.to_string()),
            StringValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
        },
        _ => None,
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

impl ScriptRule for FormatString {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| match e {
            Expr::BinOp(b) if b.op == Operator::Mod => {
                let Some(format) = string_of(&b.left) else {
                    return;
                };
                if let Some(problem) = check_percent(&format, &b.right) {
                    found.push((e.range(), problem));
                }
            }
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else { return };
                if &*a.attr.id != "format" {
                    return;
                }
                let Some(format) = string_of(&a.value) else {
                    return;
                };
                if let Some(problem) = check_format_call(&format, c) {
                    found.push((e.range(), problem));
                }
            }
            _ => {}
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

fn check_percent(format: &str, right: &Expr) -> Option<String> {
    let spec = match parse_percent(format) {
        Ok(s) => s,
        Err(Bad::Error(e)) => return Some(format!("Bad `%` format string: {e}")),
        Err(Bad::Unknown) => return None,
    };
    match right {
        Expr::Tuple(t) => {
            if !spec.keys.is_empty() {
                return Some("`%(key)` format requires a mapping, not a tuple".into());
            }
            let given = t.elts.len();
            if given < spec.positional {
                Some(format!(
                    "Not enough arguments for format string: it needs {}, {} given",
                    spec.positional, given
                ))
            } else if given > spec.positional {
                Some(format!(
                    "Not all arguments converted during string formatting: it uses {}, {} given",
                    plural(spec.positional, "value"),
                    given
                ))
            } else {
                None
            }
        }
        Expr::Dict(d) => {
            if spec.keys.is_empty() {
                return None;
            }
            let mut given = Vec::new();
            for k in &d.keys {
                given.push(string_of(k)?);
            }
            let missing: Vec<&String> = spec.keys.iter().filter(|k| !given.contains(k)).collect();
            missing
                .first()
                .map(|k| format!("Format key `{k}` is missing from the dict"))
        }
        Expr::Str(_) | Expr::Num(_) | Expr::List(_) | Expr::ListComp(_) => {
            if !spec.keys.is_empty() {
                return Some("`%(key)` format requires a mapping".into());
            }
            match spec.positional {
                1 => None,
                0 => Some("Not all arguments converted during string formatting: the format string has no placeholder".into()),
                n => Some(format!("Not enough arguments for format string: it needs {n}, 1 given")),
            }
        }
        _ => None,
    }
}

fn check_format_call(format: &str, call: &iglint_python::ast::ExprCall) -> Option<String> {
    let spec = match parse_braces(format) {
        Ok(s) => s,
        Err(Bad::Error(e)) => return Some(format!("Bad `format` string: {e}")),
        Err(Bad::Unknown) => return None,
    };
    if call.starargs.is_none() && spec.positional > call.args.len() {
        return Some(format!(
            "Format string uses {} but only {} given",
            plural(spec.positional, "positional argument"),
            call.args.len()
        ));
    }
    if call.kwargs.is_none() {
        for k in &spec.keywords {
            if !call.keywords.iter().any(|kw| &*kw.arg.id == k) {
                return Some(format!(
                    "Format string uses keyword argument `{k}`, which is not given"
                ));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::script::testing::*;

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("%s %d %%").unwrap().positional, 2);
        assert_eq!(parse_percent("%*.*f").unwrap().positional, 3);
        assert_eq!(parse_percent("%(a)s %(b(c))r").unwrap().keys, ["a", "b(c)"]);
        assert_eq!(parse_percent("%ld %-+ #05.2f").unwrap().positional, 2);
        assert!(matches!(
            parse_percent("%q"),
            Err(Bad::Error(e)) if e.contains("unsupported format character 'q'")
        ));
        assert!(parse_percent("%a").is_err());
        assert_eq!(
            parse_percent("abc %").unwrap_err(),
            "incomplete format".into()
        );
    }

    #[test]
    fn brace_parsing() {
        assert_eq!(parse_braces("{} {}").unwrap().positional, 2);
        assert_eq!(parse_braces("{0} {2} {{x}}").unwrap().positional, 3);
        assert_eq!(
            parse_braces("{a.b} {c[d]!r:>{w}}").unwrap().keywords,
            ["a", "c", "w"]
        );
        assert_eq!(parse_braces("{:{}}").unwrap().positional, 2);
        assert!(parse_braces("{} {0}").is_err());
        assert!(parse_braces("a } b").is_err());
        assert!(parse_braces("a { b").is_err());
        assert!(parse_braces("{0!a}").is_err());
        assert!(parse_braces("{0[]}").is_err());
        assert_eq!(parse_braces("{00}").unwrap().positional, 1);
        assert_eq!(parse_braces("{-1}").unwrap_err(), Bad::Unknown);
    }

    #[test]
    fn reports_mismatches() {
        let code = r#"a = "%s %s" % (1,)
b = "%s" % (1, 2)
c = "%(x)s" % {"y": 1}
d = "%(x)s" % (1,)
e = "{} {}".format(1)
f = "{a}".format(b=1)
g = "abc" % 5
h = "%s" % [1, 2]
i = "%s %s" % {"a": 1}
j = "{0}".format(1, 2, z=3)
k = "%s %s" % args
l = "{} {}".format(*args)
m = u"%d%%" % (5,)
"#;
        let d = lint_module("SCR112", code);
        assert_eq!(
            lines(&d).into_iter().map(|(l, _)| l).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7]
        );
    }

    fn percent_error(format: &str) -> String {
        match parse_percent(format) {
            Err(Bad::Error(e)) => e,
            other => panic!("{format}: {other:?}"),
        }
    }

    fn brace_error(format: &str) -> String {
        match parse_braces(format) {
            Err(Bad::Error(e)) => e,
            other => panic!("{format}: {other:?}"),
        }
    }

    /// Each case was run on Jython 2.7 (Ignition 8.3.3).
    #[test]
    fn percent_grammar_matches_jython() {
        // One length modifier at most.
        assert_eq!(
            percent_error("%lls"),
            "unsupported format character 'l' (0x6c) at index 2"
        );
        assert_eq!(
            percent_error("%hls"),
            "unsupported format character 'l' (0x6c) at index 2"
        );
        assert_eq!(parse_percent("%ls %Ls %hd").unwrap().positional, 3);
        // `%%` after flags, width or precision; `*` values are still taken.
        assert_eq!(parse_percent("%5% %.%").unwrap().positional, 0);
        assert_eq!(parse_percent("%*%").unwrap().positional, 1);
        assert_eq!(parse_percent("%0*.*f").unwrap().positional, 3);
        assert_eq!(parse_percent("%(a)%").unwrap().keys, ["a"]);
        assert_eq!(parse_percent("%(a(b))s").unwrap().keys, ["a(b)"]);
        assert_eq!(
            parse_percent("%.s %0-5d %-+ #05.3ld").unwrap().positional,
            3
        );
        assert_eq!(
            percent_error("%5.2.3f"),
            "unsupported format character '.' (0x2e) at index 4"
        );
        for incomplete in ["%", "%5", "%(a", "%5.", "%l"] {
            assert_eq!(
                percent_error(incomplete),
                "incomplete format",
                "{incomplete}"
            );
        }
        assert!(percent_error("%99999999999s").contains("NumberFormatException"));
        assert!(percent_error("%.2147483648f").contains("NumberFormatException"));
        // Java counts UTF-16 code units.
        assert_eq!(
            percent_error("\u{1F600}%q"),
            "unsupported format character 'q' (0x71) at index 3"
        );
        // Java's isDigit accepts other digits: not modelled, no finding.
        assert_eq!(parse_percent("%\u{663}s").unwrap_err(), Bad::Unknown);
    }

    /// Each case was run on Jython 2.7 (Ignition 8.3.3).
    #[test]
    fn brace_grammar_matches_jython() {
        // Indexes that do not fit in an int are keywords: KeyError.
        assert!(brace_error("{18446744073709551615}").contains("KeyError"));
        assert!(brace_error("{2147483648}").contains("KeyError"));
        assert_eq!(parse_braces("{2147483647}").unwrap().positional, 2147483648);
        // A sign makes an index without switching to manual numbering.
        assert_eq!(parse_braces("{+0}{}").unwrap().positional, 1);
        assert_eq!(parse_braces("{-0}{}").unwrap().positional, 1);
        assert_eq!(parse_braces("{+1}").unwrap().positional, 2);
        assert_eq!(
            parse_braces("{+2147483648}").unwrap().keywords,
            ["+2147483648"]
        );
        assert!(brace_error("{00}{}").contains("cannot switch"));
        assert!(brace_error("{}{2147483648}").contains("cannot switch"));
        assert_eq!(parse_braces("{a}{}").unwrap().positional, 1);
        assert_eq!(
            parse_braces("{0x1} { 0} {0]}").unwrap().keywords,
            ["0x1", " 0", "0]"]
        );
        // The name ends at the first `!` or `:`, even inside brackets.
        assert_eq!(brace_error("{0[!]}"), "Missing ']' in format string");
        assert_eq!(brace_error("{0[:]}"), "Missing ']' in format string");
        assert_eq!(brace_error("{0[a}b]}"), "Missing ']' in format string");
        assert_eq!(
            brace_error("{0[0]x}"),
            "Only '.' or '[' may follow ']' in format field specifier"
        );
        assert_eq!(
            brace_error("{0!rr}"),
            "expected ':' after conversion specifier"
        );
        assert_eq!(brace_error("{0:{1:{2}}}"), "Max string recursion exceeded");
        assert_eq!(brace_error("{:{:{}}}"), "Max string recursion exceeded");
        assert_eq!(parse_braces("{0:{1:x}} {0[0].real}").unwrap().positional, 2);
        assert_eq!(parse_braces("{\u{661}}").unwrap_err(), Bad::Unknown);
    }

    #[test]
    fn huge_indexes_do_not_overflow() {
        let d = lint_module("SCR112", "x = \"{18446744073709551615}\".format(1)\n");
        assert_eq!(d.len(), 1);
        assert!(lint_module("SCR112", "x = '%lls' % 'x'\n").len() == 1);
        assert!(lint_module("SCR112", "x = '%*%' % (1,)\n").is_empty());
    }
}
