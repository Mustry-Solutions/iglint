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

/// Parses a `%` format string like `PyString.StringFormatter`.
fn parse_percent(s: &str) -> Result<PercentSpec, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut spec = PercentSpec::default();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '%' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut keyed = false;
        if chars.get(i) == Some(&'(') {
            let mut depth = 1;
            let key_start = i + 1;
            i += 1;
            while i < chars.len() && depth > 0 {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth > 0 {
                return Err("incomplete format key".into());
            }
            spec.keys.push(chars[key_start..i - 1].iter().collect());
            keyed = true;
        }
        while i < chars.len() && matches!(chars[i], '-' | '+' | ' ' | '#' | '0') {
            i += 1;
        }
        let mut stars = 0;
        if chars.get(i) == Some(&'*') {
            stars += 1;
            i += 1;
        } else {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
        if chars.get(i) == Some(&'.') {
            i += 1;
            if chars.get(i) == Some(&'*') {
                stars += 1;
                i += 1;
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
        while i < chars.len() && matches!(chars[i], 'h' | 'l' | 'L') {
            i += 1;
        }
        let Some(&conversion) = chars.get(i) else {
            return Err("incomplete format".into());
        };
        i += 1;
        match conversion {
            '%' => {
                // `%%` consumes nothing; `%(key)%` neither.
                let _ = start;
            }
            'd' | 'i' | 'o' | 'u' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'c' | 'r'
            | 's' => {
                spec.positional += stars + usize::from(!keyed);
            }
            other => {
                return Err(format!(
                    "unsupported format character '{other}' (0x{:x}) at index {}",
                    other as u32,
                    i - 1
                ));
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

/// Parses a `str.format` string like Jython's `MarkupIterator`.
fn parse_braces(s: &str) -> Result<BraceSpec, String> {
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
) -> Result<(), String> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => i += 2,
            '}' if chars.get(i + 1) == Some(&'}') => i += 2,
            '}' => return Err("Single '}' encountered in format string".into()),
            '{' => {
                // Find the matching close brace (nested fields in the spec).
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
) -> Result<(), String> {
    // field_name [!conversion] [:format_spec]; `!` and `:` inside `[...]`
    // belong to the key.
    let mut name_end = field.len();
    let mut in_bracket = false;
    for (k, c) in field.char_indices() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            '!' | ':' if !in_bracket => {
                name_end = k;
                break;
            }
            _ => {}
        }
    }
    let name = &field[..name_end];
    let mut rest = &field[name_end..];
    if let Some(r) = rest.strip_prefix('!') {
        let mut chars = r.chars();
        let conversion = chars.next();
        match conversion {
            Some('r' | 's') => {}
            Some(c) => return Err(format!("Unknown conversion specifier {c}")),
            None => return Err("end of format while looking for conversion specifier".into()),
        }
        rest = chars.as_str();
        if !rest.is_empty() && !rest.starts_with(':') {
            return Err("expected ':' after format specifier".into());
        }
    }
    let first_end = name.find(['.', '[']).unwrap_or(name.len());
    let arg = &name[..first_end];
    if name[first_end..].starts_with(".")
        && name[first_end + 1..].split(['.', '[']).next() == Some("")
    {
        return Err("Empty attribute in format string".into());
    }
    if name[first_end..].contains("[]") {
        return Err("Empty attribute in format string".into());
    }
    if arg.is_empty() {
        if *auto == Some(false) {
            return Err(
                "cannot switch from manual field specification to automatic field numbering".into(),
            );
        }
        *auto = Some(true);
        *next_auto += 1;
        spec.positional = spec.positional.max(*next_auto);
    } else if arg.bytes().all(|b| b.is_ascii_digit()) {
        if *auto == Some(true) {
            return Err(
                "cannot switch from automatic field numbering to manual field specification".into(),
            );
        }
        *auto = Some(false);
        if let Ok(index) = arg.parse::<usize>() {
            spec.positional = spec.positional.max(index + 1);
        }
    } else if !arg.starts_with('-') && !spec.keywords.iter().any(|k| k == arg) {
        spec.keywords.push(arg.to_owned());
    }
    if let Some(format_spec) = rest.strip_prefix(':')
        && format_spec.contains('{')
        && depth < 2
    {
        parse_braces_into(format_spec, spec, auto, next_auto, depth + 1)?;
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
        Err(e) => return Some(format!("Bad `%` format string: {e}")),
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
        Err(e) => return Some(format!("Bad `format` string: {e}")),
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
        assert!(
            parse_percent("%q")
                .unwrap_err()
                .contains("unsupported format character 'q'")
        );
        assert!(parse_percent("%a").is_err());
        assert_eq!(parse_percent("abc %").unwrap_err(), "incomplete format");
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
        assert_eq!(parse_braces("{-1}").unwrap(), BraceSpec::default());
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
}
