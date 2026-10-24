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
  value instead of a dict, `%(name)s` keys missing from a dict literal,
  `%c` with a string literal that is not one character (`'%c' %
  'ab'`) or an integer that is negative or beyond Unicode, a numeric
  conversion (`%d %i %o %u %x %X %e %E %f %F %g %G`) or a `*` width with a
  string, complex number, container or `None` literal (`'%d' % 'ab'`). A
  single non-tuple literal counts as one value.
* `str.format`: a single `{` or `}`, mixing `{}` and `{0}`, conversions
  other than `!r` and `!s`, positional fields beyond the arguments given,
  and keyword fields that are not passed. A numeric field too large for a
  Java `int` (`{2147483648}`) is a keyword field, which only `**kwargs`
  can pass. A negative index (`{-1}`) always fails
  (`ArrayIndexOutOfBoundsException`; `{-0}` is index 0). A format spec
  whose width or precision is too large for a Java `int`
  (`{0:2147483648}`) fails for a literal argument and with `!r` or `!s`
  (other objects may have their own `__format__`), and so does one that
  Jython's parser rejects whatever the value: characters after the type
  (`'{:dd}'.format(1)`, `'{:%Y}'.format(1)`, `'{:.2.2}'.format(1.5)`) or a
  `.` without digits (`'{:5.}'.format(1)`). A format spec that the
  literal argument does not support (checked against Jython on more than
  five million generated specs and literals): an unknown presentation type
  (`'{:z}'.format(1)`, `'{:d}'.format('a')`, `'{:s}'.format(1)`,
  `'{:d}'.format(1.5)`, any type but `s` after `!r` or `!s`); for strings
  (and containers and `None`, formatted as their `str`) a sign, `#`, `=`
  alignment (also implied by a `0` before the width) or `,`
  (`'{:+s}'.format('ab')`); for integers a precision with an integer type
  (`'{:.2}'.format(1)`), a sign or `#` with `c`, `,` with a type other than
  `d`; for floats `#`, `,` with `n`; for complex numbers `#`, zero padding,
  `=` alignment, `,` with `n`.

Only literal format strings (also concatenated with `+`: `('%s' +
'%s') % (1,)`) with literal arguments (or literal argument lists) are
checked; anything computed is left alone. `*args` and
`**kwargs` count when they are literals (`*(1,)`, `**{'a': 1}`). Extra
arguments to `.format` are allowed (Python ignores them).

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
    /// The conversion of each positional value, in order (`None` for a
    /// `*` width or precision).
    conversions: Vec<Option<char>>,
    /// The conversion of each `%(key)` placeholder.
    key_conversions: Vec<(String, char)>,
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
        spec.conversions.extend(std::iter::repeat_n(None, stars));
        match c {
            // Prints `%`; `*` values are still taken.
            '%' => spec.positional += stars,
            'd' | 'i' | 'o' | 'u' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'c' | 'r'
            | 's' => {
                spec.positional += stars + usize::from(!keyed);
                if keyed {
                    let key = spec.keys.last().cloned().unwrap_or_default();
                    spec.key_conversions.push((key, c));
                } else {
                    spec.conversions.push(Some(c));
                }
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
    /// Fields whose literal format spec has a width or precision too large
    /// for a Java `int`.
    oversized: Vec<Oversized>,
    /// Fields of an argument itself with a well-formed literal format
    /// spec (`{0:d}`, `{:.2f}`, `{x!s:>5}`, `{:+}`).
    typed: Vec<TypedField>,
}

/// A field formatted with a well-formed standard format spec.
#[derive(Debug, PartialEq)]
struct TypedField {
    argument: FieldArg,
    /// `!r` or `!s`: the value formatted is a string.
    converted: bool,
    parsed: StandardSpec,
    spec: String,
}

/// A field whose format spec has a width or precision that Jython's
/// standard format spec parser (used by `str`, numbers, containers and
/// every object without its own `__format__`) cannot read: `ValueError:
/// For input string`.
#[derive(Debug, PartialEq)]
struct Oversized {
    /// The argument formatted, when it is the field itself (no `.attr` or
    /// `[key]` after the name).
    argument: Option<FieldArg>,
    /// `!r` or `!s`: the value is a string, so the standard parser runs
    /// whatever the argument is.
    converted: bool,
    spec: String,
}

#[derive(Debug, PartialEq)]
enum FieldArg {
    Index(usize),
    Keyword(String),
}

impl FieldArg {
    fn clone_arg(&self) -> FieldArg {
        match self {
            FieldArg::Index(i) => FieldArg::Index(*i),
            FieldArg::Keyword(k) => FieldArg::Keyword(k.clone()),
        }
    }
}

/// A standard format spec, `[[fill]align][sign][#][0][width][,]
/// [.precision][type]`, as Jython's `InternalFormat` reads it.
#[derive(Clone, Debug, Default, PartialEq)]
struct StandardSpec {
    fill: Option<char>,
    align: Option<char>,
    sign: bool,
    alternate: bool,
    zero: bool,
    grouping: bool,
    precision: bool,
    kind: Option<char>,
    /// Why Jython's parser rejects the spec whatever the value is: a `.`
    /// without digits, or characters after the type (`dd`, `%Y`, `.2.2`).
    invalid: Option<&'static str>,
}

impl StandardSpec {
    /// Whether the value is padded with zeros: an explicit `0` fill, or
    /// the `0` flag without a fill.
    fn zero_fill(&self) -> bool {
        self.fill == Some('0') || (self.fill.is_none() && self.zero)
    }

    /// The alignment, with the `0` flag's implied `=` when none is given.
    fn alignment(&self) -> Option<char> {
        self.align.or(self.zero.then_some('='))
    }
}

/// The standard format spec `spec`, when it is well formed (and ASCII);
/// `None` otherwise (left alone).
fn standard_spec(spec: &str) -> Option<StandardSpec> {
    let chars: Vec<char> = spec.chars().collect();
    if chars.iter().any(|c| !c.is_ascii()) {
        return None;
    }
    let mut out = StandardSpec::default();
    let mut i = 0;
    let align = |c: Option<&char>| matches!(c, Some('<' | '>' | '=' | '^'));
    // Jython reads an alignment character first as the alignment, never as
    // a fill (`<<` is alignment `<` and type `<`, verified), unlike CPython.
    if align(chars.first()) {
        out.align = Some(chars[0]);
        i = 1;
    } else if align(chars.get(1)) {
        out.fill = Some(chars[0]);
        out.align = Some(chars[1]);
        i = 2;
    }
    if chars.get(i).is_some_and(|c| matches!(c, '+' | '-' | ' ')) {
        out.sign = true;
        i += 1;
    }
    if chars.get(i) == Some(&'#') {
        out.alternate = true;
        i += 1;
    }
    if chars.get(i) == Some(&'0') {
        out.zero = true;
        i += 1;
    }
    while chars.get(i).is_some_and(char::is_ascii_digit) {
        i += 1;
    }
    if chars.get(i) == Some(&',') {
        out.grouping = true;
        i += 1;
    }
    if chars.get(i) == Some(&'.') {
        i += 1;
        let start = i;
        while chars.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            out.invalid = Some("Format specifier missing precision");
            return Some(out);
        }
        out.precision = true;
    }
    match &chars[i..] {
        [] => {}
        [t] => out.kind = Some(*t),
        [t, ..] => {
            out.kind = Some(*t);
            out.invalid = Some("Invalid conversion specification");
        }
    }
    Some(out)
}

/// What a literal argument is, for the type checks of `%` conversions and
/// format specs. `True` and `False` can be rebound in Python 2 and are not
/// literals; `None` cannot.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Literal {
    Str,
    /// An `int` or `long` (the value when it fits).
    Int(Option<i64>),
    Float,
    Complex,
    /// A list, tuple, dict or set display (or comprehension).
    Container,
    None,
}

fn literal(e: &Expr) -> Option<Literal> {
    use iglint_python::ast::{Number, UnaryOp};
    Some(match e {
        Expr::Str(_) => Literal::Str,
        Expr::Num(n) => match &n.n {
            Number::Int(v) => Literal::Int(Some(*v)),
            Number::Long(v) => Literal::Int(v.parse().ok()),
            Number::Float(_) => Literal::Float,
            Number::Complex(_) => Literal::Complex,
        },
        Expr::UnaryOp(u) if matches!(u.op, UnaryOp::USub | UnaryOp::UAdd) => {
            match (literal(&u.operand)?, u.op) {
                (Literal::Int(Some(v)), UnaryOp::USub) => Literal::Int(v.checked_neg()),
                (Literal::Int(v), _) => Literal::Int(v),
                (l @ (Literal::Float | Literal::Complex), _) => l,
                _ => return None,
            }
        }
        Expr::List(_)
        | Expr::ListComp(_)
        | Expr::Tuple(_)
        | Expr::Dict(_)
        | Expr::DictComp(_)
        | Expr::Set(_)
        | Expr::SetComp(_) => Literal::Container,
        Expr::Name(n) if &*n.id == "None" => Literal::None,
        _ => return None,
    })
}

/// Why `%` conversion `c` fails on the literal `e` (verified on Jython):
/// the numeric conversions need a number (`int`, `long`, `float`, `bool`),
/// `%c` an int in range or one character. `None` when it may succeed.
fn percent_type_error(c: char, e: &Expr) -> Option<String> {
    let lit = literal(e)?;
    match c {
        'd' | 'i' | 'o' | 'u' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G' => matches!(
            lit,
            Literal::Str | Literal::Complex | Literal::Container | Literal::None
        )
        .then(|| {
            format!(
                "`%{c}` needs a number, not {} (Jython raises TypeError)",
                describe(lit)
            )
        }),
        'c' => match lit {
            Literal::Str => bad_char(e).then(|| BAD_CHAR.to_owned()),
            Literal::Complex | Literal::Container | Literal::None => Some(BAD_CHAR.to_owned()),
            // Negative or beyond Unicode: out of range for any format
            // string (byte strings take 0 to 255, unicode strings more).
            Literal::Int(Some(v)) if !(0..=0x10FFFF).contains(&v) => Some(
                "`%c` argument is not in the range of characters (Jython raises OverflowError)"
                    .to_owned(),
            ),
            _ => None,
        },
        _ => None,
    }
}

/// Why a `*` width or precision fails on the literal `e`: it must be an
/// integer (verified on Jython: `TypeError: * wants int`).
fn star_type_error(e: &Expr) -> Option<String> {
    let lit = literal(e)?;
    matches!(
        lit,
        Literal::Str | Literal::Complex | Literal::Container | Literal::None
    )
    .then(|| {
        format!(
            "`*` needs an integer, not {} (Jython raises TypeError)",
            describe(lit)
        )
    })
}

fn describe(lit: Literal) -> &'static str {
    match lit {
        Literal::Str => "a string",
        Literal::Int(_) => "an integer",
        Literal::Float => "a float",
        Literal::Complex => "a complex number",
        Literal::Container => "a list, tuple, dict or set",
        Literal::None => "`None`",
    }
}

/// Why the format spec `spec` fails for the literal `e` in `str.format`,
/// with Jython's message. `!r` and `!s` make every value a string, and
/// containers and `None` are formatted as their `str` (`object.__format__`).
/// Checked against Jython on 5,414,400 generated specs and literals (every
/// combination of fills and alignments, signs, `#`, `0`, widths, `,`,
/// precisions, every presentation type and a few invalid ones, with and
/// without `!r` / `!s`, and on a smaller grid the same followed by
/// characters after the type): this reports exactly the ones that raise.
///
/// * every literal: a `.` without digits ("Format specifier missing
///   precision") and characters after the type ("Invalid conversion
///   specification", `{0:dd}`, `{0:%Y}`, `{0:.2.2}`): the parser rejects
///   them before it looks at the value. Only literals: other objects may
///   have a `__format__` of their own (`datetime` takes `strftime` codes);
/// * strings: only `s` or no type; no sign, `#`, `=` alignment (also
///   implied by the `0` flag) or `,`;
/// * integers: with `b c d o x X n` or no type no precision, with `c` no
///   sign or `#` and a value in range, `,` only with `d` or no type; the
///   float types and `%` format the value as a float;
/// * floats: `e E f F g G n %` or no type; no `#`; `,` not with `n`;
/// * complex numbers: `e E f F g G n` or no type; no `#`, zero padding,
///   `=` alignment or `,` with `n`.
fn format_spec_error(spec: &StandardSpec, converted: bool, e: &Expr) -> Option<String> {
    let lit = if converted { Literal::Str } else { literal(e)? };
    let m = format_spec_problem(spec, lit)?;
    Some(if m.contains("OverflowError") {
        m
    } else {
        format!("{m} (Jython raises ValueError)")
    })
}

/// [`format_spec_error`] for a literal of kind `lit`, without the
/// exception.
fn format_spec_problem(spec: &StandardSpec, lit: Literal) -> Option<String> {
    if let Some(invalid) = spec.invalid {
        return Some(invalid.to_owned());
    }
    let kind = spec.kind;
    let ty = kind.map_or_else(String::new, |t| format!(" '{t}'"));
    let unknown = |object: &str| {
        Some(format!(
            "Unknown format code '{}' for object of type '{object}'",
            kind.unwrap_or(' ')
        ))
    };
    let in_float = |kind: Option<char>, object: &str| -> Option<String> {
        if spec.alternate {
            return Some(format!(
                "Alternate form (#) not allowed in {object} format specifier"
            ));
        }
        if spec.grouping && kind == Some('n') {
            return Some(format!(
                "Grouping not allowed with {object} format specifier 'n'"
            ));
        }
        None
    };
    match lit {
        Literal::Str | Literal::Container | Literal::None => {
            if !matches!(kind, None | Some('s')) {
                return unknown("string");
            }
            if spec.sign {
                return Some("Sign not allowed in string format specifier".to_owned());
            }
            if spec.alternate {
                return Some(
                    "Alternate form (#) not allowed in string format specifier".to_owned(),
                );
            }
            if spec.alignment() == Some('=') {
                return Some(
                    "'=' alignment flag not allowed in string format specifier".to_owned(),
                );
            }
            if spec.grouping {
                return Some(format!(
                    "Grouping not allowed with string format specifier{ty}"
                ));
            }
            None
        }
        Literal::Int(v) => match kind {
            None | Some('b' | 'c' | 'd' | 'o' | 'x' | 'X' | 'n') => {
                if spec.precision {
                    return Some("Precision not allowed in integer format specifier".to_owned());
                }
                if kind == Some('c') {
                    if spec.sign {
                        return Some(
                            "Sign not allowed with integer format specifier 'c'".to_owned(),
                        );
                    }
                    if spec.alternate {
                        return Some(
                            "Alternate form (#) not allowed with integer format specifier 'c'"
                                .to_owned(),
                        );
                    }
                    if v.is_some_and(|v| !(0..=0x10FFFF).contains(&v)) {
                        return Some(
                            "`c` argument is not in the range of characters (Jython raises OverflowError)"
                                .to_owned(),
                        );
                    }
                }
                if spec.grouping && !matches!(kind, None | Some('d')) {
                    return Some(format!(
                        "Grouping not allowed with integer format specifier{ty}"
                    ));
                }
                None
            }
            Some('e' | 'E' | 'f' | 'F' | 'g' | 'G' | '%') => in_float(kind, "float"),
            Some(_) => unknown("integer"),
        },
        Literal::Float => match kind {
            None | Some('e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'n' | '%') => in_float(kind, "float"),
            Some(_) => unknown("float"),
        },
        Literal::Complex => match kind {
            None | Some('e' | 'E' | 'f' | 'F' | 'g' | 'G' | 'n') => {
                if let Some(e) = in_float(kind, "complex") {
                    return Some(e);
                }
                if spec.zero_fill() {
                    return Some("Zero padding not allowed in complex format specifier".to_owned());
                }
                if spec.alignment() == Some('=') {
                    return Some(
                        "'=' alignment flag not allowed in complex format specifier".to_owned(),
                    );
                }
                None
            }
            Some(_) => unknown("complex"),
        },
    }
}

/// Whether a format spec (`[[fill]align][sign][#][0][width][,][.precision]
/// [type]`) has a width or precision beyond `i32::MAX`, which
/// `Integer.parseInt` rejects (verified on Jython, leading zeros
/// included). `None` when it holds a non-ASCII digit (not modelled).
fn spec_number_too_large(spec: &str) -> Option<bool> {
    if spec.chars().any(maybe_java_digit) {
        return None;
    }
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    let align = |c: Option<&char>| matches!(c, Some('<' | '>' | '=' | '^'));
    // As in `standard_spec`: an alignment character is never a fill.
    if align(chars.first()) {
        i = 1;
    } else if align(chars.get(1)) {
        i = 2;
    }
    for flag in [&['+', '-', ' '][..], &['#'], &['0']] {
        if chars.get(i).is_some_and(|c| flag.contains(c)) {
            i += 1;
        }
    }
    let number = |i: &mut usize| -> bool {
        let start = *i;
        while chars.get(*i).is_some_and(char::is_ascii_digit) {
            *i += 1;
        }
        let digits: String = chars[start..*i].iter().collect();
        let digits = digits.trim_start_matches('0');
        digits.len() > 10 || (digits.len() == 10 && digits > "2147483647")
    };
    if number(&mut i) {
        return Some(true);
    }
    if chars.get(i) == Some(&',') {
        i += 1;
    }
    if chars.get(i) == Some(&'.') {
        i += 1;
        return Some(number(&mut i));
    }
    Some(false)
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
    let mut argument = None;
    if head.is_empty() {
        if *auto == Some(false) {
            return Err(
                "cannot switch from manual field specification to automatic field numbering".into(),
            );
        }
        *auto = Some(true);
        *next_auto += 1;
        spec.positional = spec.positional.max(*next_auto);
        argument = Some(FieldArg::Index(*next_auto - 1));
    } else if head.starts_with(|c: char| c.is_ascii_digit()) {
        if *auto == Some(true) {
            return Err(
                "cannot switch from automatic field numbering to manual field specification".into(),
            );
        }
        *auto = Some(false);
    }
    // `Integer.parseInt`: an index when it fits in an int, else a keyword
    // (`{2147483648}` and `{+2147483648}` are looked up in the keyword
    // arguments, still with manual numbering; verified on Jython).
    if !head.is_empty() {
        let digits = head.strip_prefix(['+', '-']).unwrap_or(head);
        let numeric = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
        match head.parse::<i32>() {
            Ok(index) if numeric => match usize::try_from(index) {
                Ok(index) => {
                    spec.positional = spec.positional.max(index.saturating_add(1));
                    argument = Some(FieldArg::Index(index));
                }
                // Jython indexes the argument array with it, whatever the
                // arguments are (also with `**kwargs` that have the key).
                Err(_) => {
                    return Err(format!(
                        "field `{{{head}}}` is a negative index (ArrayIndexOutOfBoundsException)"
                    )
                    .into());
                }
            },
            _ => {
                if !spec.keywords.iter().any(|k| k == head) {
                    spec.keywords.push(head.to_owned());
                }
                argument = Some(FieldArg::Keyword(head.to_owned()));
            }
        }
    }
    field_rest(&name[first_end..])?;
    if first_end < name.len() {
        argument = None;
    }
    let mut rest = &field[name_end..];
    let converted = rest.starts_with('!');
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
    if let Some(format_spec) = rest.strip_prefix(':') {
        if format_spec.contains('{') {
            if depth >= 1 {
                return Err("Max string recursion exceeded".into());
            }
            parse_braces_into(format_spec, spec, auto, next_auto, depth + 1)?;
        } else {
            if spec_number_too_large(format_spec) == Some(true) {
                spec.oversized.push(Oversized {
                    argument: argument.as_ref().map(FieldArg::clone_arg),
                    converted,
                    spec: format_spec.to_owned(),
                });
            }
            if let (Some(argument), Some(parsed)) = (argument, standard_spec(format_spec)) {
                spec.typed.push(TypedField {
                    argument,
                    converted,
                    parsed,
                    spec: format_spec.to_owned(),
                });
            }
        }
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

/// A string literal, or a concatenation of string literals with `+`
/// (`('%s' + '%s')`), which Jython evaluates before formatting.
fn folded_string(e: &Expr) -> Option<String> {
    match e {
        Expr::BinOp(b) if b.op == Operator::Add => {
            Some(folded_string(&b.left)? + &folded_string(&b.right)?)
        }
        _ => string_of(e),
    }
}

/// Whether `%c` fails on the literal `e`: a string literal that is not
/// exactly one character (`TypeError: %c requires int or char`, verified
/// on Jython). Byte strings with non-ASCII bytes are not judged (their
/// length depends on the source encoding).
fn bad_char(e: &Expr) -> bool {
    match e {
        Expr::Str(s) => match &s.s {
            StringValue::Unicode(u) => u.chars().count() != 1,
            StringValue::Bytes(b) => b.is_ascii() && b.len() != 1,
        },
        _ => false,
    }
}

const BAD_CHAR: &str = "`%c` requires an int or a single character (Jython raises TypeError)";

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
                let Some(format) = folded_string(&b.left) else {
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
                let Some(format) = folded_string(&a.value) else {
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
                spec.conversions
                    .iter()
                    .zip(&t.elts)
                    .find_map(|(c, e)| match c {
                        Some(c) => percent_type_error(*c, e),
                        None => star_type_error(e),
                    })
            }
        }
        Expr::Dict(d) => {
            if spec.keys.is_empty() {
                // The dict is the one value of a single placeholder.
                return match spec.conversions.as_slice() {
                    [Some(c)] => percent_type_error(*c, right),
                    _ => None,
                };
            }
            let mut given = Vec::new();
            for k in &d.keys {
                given.push(string_of(k)?);
            }
            let missing: Vec<&String> = spec.keys.iter().filter(|k| !given.contains(k)).collect();
            if let Some(k) = missing.first() {
                return Some(format!("Format key `{k}` is missing from the dict"));
            }
            // The value of a key is the last one written for it.
            let value = |key: &str| given.iter().rposition(|k| k == key).map(|i| &d.values[i]);
            spec.key_conversions
                .iter()
                .find_map(|(k, c)| percent_type_error(*c, value(k)?))
        }
        _ if is_single_value(right) => {
            if !spec.keys.is_empty() {
                return Some("`%(key)` format requires a mapping".into());
            }
            match spec.positional {
                1 => spec.conversions[0].and_then(|c| percent_type_error(c, right)),
                0 => Some("Not all arguments converted during string formatting: the format string has no placeholder".into()),
                n => Some(format!("Not enough arguments for format string: it needs {n}, 1 given")),
            }
        }
        _ => None,
    }
}

/// Whether `e`, the right operand of `%`, is certainly one value that is
/// not a tuple or a mapping: a string, number, list or set literal (or
/// comprehension), a signed number literal, `None`.
fn is_single_value(e: &Expr) -> bool {
    match e {
        Expr::Str(_)
        | Expr::Num(_)
        | Expr::List(_)
        | Expr::ListComp(_)
        | Expr::Set(_)
        | Expr::SetComp(_) => true,
        Expr::UnaryOp(_) => literal(e).is_some(),
        Expr::Name(n) => &*n.id == "None",
        _ => false,
    }
}

fn check_format_call(format: &str, call: &iglint_python::ast::ExprCall) -> Option<String> {
    let spec = match parse_braces(format) {
        Ok(s) => s,
        Err(Bad::Error(e)) => return Some(format!("Bad `format` string: {e}")),
        Err(Bad::Unknown) => return None,
    };
    // The positional arguments, with a literal `*(...)` / `*[...]`.
    let positional: Option<Vec<&Expr>> = match call.starargs.as_deref() {
        None => Some(call.args.iter().collect()),
        Some(Expr::Tuple(t)) => Some(call.args.iter().chain(&t.elts).collect()),
        Some(Expr::List(l)) => Some(call.args.iter().chain(&l.elts).collect()),
        Some(_) => None,
    };
    if let Some(positional) = &positional
        && spec.positional > positional.len()
    {
        return Some(format!(
            "Format string uses {} but only {} given",
            plural(spec.positional, "positional argument"),
            positional.len()
        ));
    }
    // The keyword arguments, with a literal `**{...}` whose keys are
    // string literals.
    let keywords: Option<Vec<(String, &Expr)>> = match call.kwargs.as_deref() {
        None => Some(Vec::new()),
        Some(Expr::Dict(d)) => d
            .keys
            .iter()
            .zip(&d.values)
            .map(|(k, v)| string_of(k).map(|k| (k, v)))
            .collect(),
        Some(_) => None,
    };
    let keywords = keywords.map(|mut all| {
        all.extend(
            call.keywords
                .iter()
                .map(|k| (k.arg.id.to_string(), &k.value)),
        );
        all
    });
    if let Some(keywords) = &keywords {
        for k in &spec.keywords {
            if !keywords.iter().any(|(name, _)| name == k) {
                let digits = k.strip_prefix(['+', '-']).unwrap_or(k);
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    // No keyword argument written in a call can have this
                    // name; only `**kwargs` can pass it.
                    return Some(format!(
                        "Format field `{{{k}}}` is too large for an index, so it is looked up as a keyword argument, which is not given (KeyError)"
                    ));
                }
                return Some(format!(
                    "Format string uses keyword argument `{k}`, which is not given"
                ));
            }
        }
    }
    for o in &spec.oversized {
        let value = match &o.argument {
            Some(FieldArg::Index(i)) => positional.as_ref().and_then(|p| p.get(*i).copied()),
            Some(FieldArg::Keyword(k)) => keywords
                .as_ref()
                .and_then(|kw| kw.iter().find(|(name, _)| name == k).map(|(_, v)| *v)),
            None => None,
        };
        // Literals format with the standard parser; other objects may
        // have their own `__format__` (`datetime` takes `strftime` codes).
        let standard = value.is_some_and(|v| {
            matches!(
                v,
                Expr::Num(_)
                    | Expr::Str(_)
                    | Expr::List(_)
                    | Expr::Tuple(_)
                    | Expr::Dict(_)
                    | Expr::Set(_)
            )
        });
        if o.converted || standard {
            return Some(format!(
                "Format spec `{}` has a width or precision too large for a Java `int` (ValueError)",
                o.spec
            ));
        }
    }
    for t in &spec.typed {
        let value = match &t.argument {
            FieldArg::Index(i) => positional.as_ref().and_then(|p| p.get(*i).copied()),
            FieldArg::Keyword(k) => keywords
                .as_ref()
                .and_then(|kw| kw.iter().rev().find(|(name, _)| name == k).map(|(_, v)| *v)),
        };
        if let Some(value) = value
            && let Some(problem) = format_spec_error(&t.parsed, t.converted, value)
        {
            return Some(format!("{problem} in `{{:{}}}`", t.spec));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::script::testing::*;

    /// Replays Jython's verdicts on `format<TAB>argument<TAB>result` lines
    /// (`oracle/format-spec.sh`): the check must report exactly the cases
    /// that raise. Returns the number of cases checked.
    fn replay_format_specs(text: &str) -> usize {
        let mut checked = 0;
        for line in text.lines() {
            let mut parts = line.splitn(3, '\t');
            let (fmt, arg, result) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let field = fmt.strip_prefix("{0").unwrap().strip_suffix('}').unwrap();
            let (converted, spec) = match field.split_once(':') {
                Some((conversion, spec)) => (!conversion.is_empty(), spec),
                None => (false, ""),
            };
            let Some(parsed) = standard_spec(spec) else {
                continue;
            };
            let module = iglint_python::parse_module(arg);
            let Some(iglint_python::ast::Stmt::Expr(e)) = module.module.body.first() else {
                panic!("{arg}");
            };
            let found = format_spec_error(&parsed, converted, &e.value);
            assert_eq!(
                found.is_some(),
                result != "ok",
                "{fmt}.format({arg}): Jython {result}, iglint {found:?}"
            );
            checked += 1;
        }
        checked
    }

    /// Reviewer round 10 (Codex): definite format spec errors with literal
    /// arguments (`'{0:z}'.format(1)`, `'{0:.2}'.format(1)`,
    /// `'{0:+s}'.format('ab')`), and round 11's characters after the type
    /// (`'{0:dd}'.format(1)`). A stratified sample of 4,000 of the
    /// 5,414,400 generated cases, half of which Jython accepts.
    #[test]
    fn format_specs_match_jython() {
        let checked = replay_format_specs(include_str!(
            "../../../tests/fixtures/format-spec/sample.tsv"
        ));
        assert_eq!(checked, 4000);
    }

    /// Every generated case (`oracle/format-spec.sh all OUT`, then
    /// `IGLINT_FORMAT_SPEC_ORACLE=OUT cargo test -p iglint_linter
    /// format_specs_match_jython_everywhere -- --ignored`).
    #[test]
    #[ignore]
    fn format_specs_match_jython_everywhere() {
        let path = std::env::var("IGLINT_FORMAT_SPEC_ORACLE").expect("IGLINT_FORMAT_SPEC_ORACLE");
        let text = std::fs::read_to_string(path).unwrap();
        let checked = replay_format_specs(&text);
        println!("{checked} cases match Jython");
    }

    #[test]
    fn format_spec_errors() {
        for (code, message) in [
            (
                "x = '{0:z}'.format(1)",
                "Unknown format code 'z' for object of type 'integer'",
            ),
            (
                "x = '{0:.2}'.format(1)",
                "Precision not allowed in integer format specifier",
            ),
            (
                "x = '{0:+s}'.format('ab')",
                "Sign not allowed in string format specifier",
            ),
            (
                "x = '{0:=5}'.format('ab')",
                "'=' alignment flag not allowed in string format specifier",
            ),
            (
                "x = '{0:05}'.format('ab')",
                "'=' alignment flag not allowed in string format specifier",
            ),
            (
                "x = '{0:,}'.format('ab')",
                "Grouping not allowed with string format specifier",
            ),
            (
                "x = '{0:#}'.format(1.5)",
                "Alternate form (#) not allowed in float format specifier",
            ),
            (
                "x = '{0:,x}'.format(5)",
                "Grouping not allowed with integer format specifier 'x'",
            ),
            (
                "x = '{0:+c}'.format(65)",
                "Sign not allowed with integer format specifier 'c'",
            ),
            (
                "x = '{0:05}'.format(1j)",
                "Zero padding not allowed in complex format specifier",
            ),
            (
                "x = '{0!r:>+5}'.format(1)",
                "Sign not allowed in string format specifier",
            ),
            // Reviewer round 11: characters after the type, and a `.`
            // without digits, whatever the literal.
            ("x = '{0:dd}'.format(1)", "Invalid conversion specification"),
            ("x = '{0:%Y}'.format(1)", "Invalid conversion specification"),
            (
                "x = '{0:.2.2}'.format(1.5)",
                "Invalid conversion specification",
            ),
            (
                "x = '{0:s.}'.format('ab')",
                "Invalid conversion specification",
            ),
            (
                "x = '{0:#x#}'.format(5)",
                "Invalid conversion specification",
            ),
            (
                "x = '{0:dd}'.format(None)",
                "Invalid conversion specification",
            ),
            (
                "x = '{0!r:dd}'.format(x)",
                "Invalid conversion specification",
            ),
            (
                "x = '{0:5.}'.format(1)",
                "Format specifier missing precision",
            ),
            // Jython never reads an alignment character as a fill.
            ("x = '{0:<<}'.format('ab')", "Unknown format code '<'"),
            ("x = '{0:^<}'.format(5)", "Unknown format code '<'"),
            (
                "x = '{0:>>10}'.format(5)",
                "Invalid conversion specification",
            ),
        ] {
            let d = lint_module("SCR112", code);
            assert_eq!(d.len(), 1, "{code}");
            assert!(d[0].message.contains(message), "{code}: {}", d[0].message);
        }
        for code in [
            "x = '{0:.2f}'.format(1)",
            "x = '{0:,}'.format(1)",
            "x = '{0:,.2f}'.format(1.5)",
            "x = '{0:x<05}'.format('ab')",
            "x = '{0:>5}'.format([1])",
            "x = '{0:05}'.format(5)",
            "x = '{0:+.1%}'.format(5)",
            "x = '{0:z}'.format(value)",
            // `datetime` and other objects have a `__format__` of their own.
            "x = '{0:%Y-%m-%d}'.format(day)",
            "x = '{0:dd}'.format(d)",
        ] {
            assert!(lint_module("SCR112", code).is_empty(), "{code}");
        }
    }

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
        assert!(brace_error("{-1}").contains("negative index"));
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

    /// Reviewer round 9 (Codex) finding C2: literal arguments of the wrong
    /// type for a numeric conversion or presentation type. Each case was
    /// run on Jython 2.7 (Ignition 8.3.3): every line of `failing` raises
    /// (`TypeError`, `ValueError` or `OverflowError`), every line of
    /// `passing` formats. 3,000 generated cases (formats, flags, widths,
    /// `*`, keys, `!r`/`!s`, specs with fills and literal arguments of every
    /// kind) gave no finding on code that formats.
    #[test]
    fn literal_arguments_of_the_wrong_type() {
        let failing = r#"a = '%d' % 'ab'
b = '%d' % ('1',)
c = '%.2f %s' % (u'x', 1)
d = '%x' % None
e = '%e' % [1]
f = '%(k)i' % {'k': 'v'}
g = '%g' % (1j,)
h = '%c' % (-1,)
i = '%c' % 1114112
j = '%*d' % ('x', 1)
k = '{:d}'.format('a')
l = '{0:.2f}'.format(u'x')
m = '{k:x}'.format(k=[1])
n = '{0!r:d}'.format(1)
o = '{:s}'.format(1)
p = '{:d}'.format(1.5)
q = '{:%}'.format(1j)
r = '{:c}'.format(-1)
s = '%d' % {}
t = '%o' % ((1,),)
"#;
        let d = lint_module("SCR112", failing);
        assert_eq!(lines(&d).len(), failing.lines().count(), "{d:?}");
        let passing = r#"a = '%d' % 1.5
b = '%x %o %e' % (1, 2L, 3)
c = '%s %r' % ('a', None)
d = '%c' % 255
e = '%f' % -1
f = '{:d}'.format(1)
g = '{:.2f}'.format(1)
h = '{:s}'.format('a')
i = '{0!s:>5}'.format(1)
j = '{:d>5}'.format('a')
k = '{:e}'.format(1j)
l = '{:n}'.format(1.5)
m = '{:%}'.format(2)
n = '%d' % True
o = '{:d}'.format(True)
p = '%d' % x
q = '{:d}'.format(x)
r = u'%c' % 300
"#;
        let d = lint_module("SCR112", passing);
        assert!(d.is_empty(), "{d:?}");
    }

    /// Each case was run on Jython 2.7 (Ignition 8.3.3): the first group
    /// raises `TypeError`, the second formats.
    #[test]
    fn char_conversions_and_concatenated_formats() {
        let failing = r#"a = '%c' % 'ab'
b = '%c' % ''
c = '%5c' % 'ab'
d = '%c%s' % ('ab', 1)
e = '%(a)c' % {'a': 'ab'}
f = u'%c' % u'ab'
g = '%*c' % (3, 'ab')
h = ('%s' + '%s') % (1,)
i = ('%s' + '%s' + '%s') % (1, 2)
j = ('{}' + '{}').format(1)
"#;
        let d = lint_module("SCR112", failing);
        assert_eq!(
            lines(&d).into_iter().map(|(l, _)| l).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
        assert!(d[0].message.contains("`%c`"), "{}", d[0].message);
        let fine = r#"a = '%c' % 'a'
b = '%c' % u'\xe9'
c = '%c' % 65
d = '%c' % '\n'
e = ('%s' + u'%s') % (1, 2)
f = '%%c' % ()
g = '%c' % u'\U0001F600'
h = '%c' % x
i = ('%s' + s) % (1,)
"#;
        let d = lint_module("SCR112", fine);
        assert!(d.is_empty(), "{d:?}");
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
        // Indexes that do not fit in an int are keywords.
        assert_eq!(
            parse_braces("{18446744073709551615}").unwrap().keywords,
            ["18446744073709551615"]
        );
        assert_eq!(
            parse_braces("{2147483648}").unwrap().keywords,
            ["2147483648"]
        );
        assert_eq!(
            parse_braces("{02147483648}").unwrap().keywords,
            ["02147483648"]
        );
        assert_eq!(parse_braces("{2147483647}").unwrap().positional, 2147483648);
        assert_eq!(
            parse_braces("{0000000002147483647}").unwrap().positional,
            2147483648
        );
        // Also with attributes and indexes after it, and in a nested
        // format spec; an index inside brackets is not a field.
        let spec = parse_braces("{2147483648.x} {0[2147483649]} {0:{2147483650}}").unwrap();
        assert_eq!(spec.keywords, ["2147483648", "2147483650"]);
        assert_eq!(spec.positional, 1);
        // Still manual numbering.
        assert!(brace_error("{:{2147483648}}").contains("cannot switch"));
        assert!(brace_error("{2147483648}{}").contains("cannot switch"));
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

    /// Verified on Jython: an index too large for an int is a keyword,
    /// which only `**kwargs` can supply.
    #[test]
    fn huge_indexes_are_keywords() {
        let d = lint_module("SCR112", "x = \"{18446744073709551615}\".format(1)\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("KeyError"), "{}", d[0].message);
        for ok in [
            "x = '{2147483648}'.format(**{'2147483648': 1})\n",
            "x = '{2147483648.x}'.format(**kw)\n",
            "x = '{0:{2147483648}}'.format(1, **kw)\n",
            "x = '{+2147483648}'.format(**kw)\n",
            "x = '{0[2147483648]}'.format(d)\n",
            "x = '{2147483647}'.format(*args)\n",
        ] {
            assert!(lint_module("SCR112", ok).is_empty(), "{ok}");
        }
        for bad in [
            "x = '{2147483648.x}'.format(o)\n",
            "x = '{0:{2147483648}}'.format(1, w=5)\n",
            "x = u'{02147483648}'.format()\n",
        ] {
            assert_eq!(lint_module("SCR112", bad).len(), 1, "{bad}");
        }
        assert!(lint_module("SCR112", "x = '%lls' % 'x'\n").len() == 1);
    }

    /// Verified on Jython: a negative index raises
    /// `ArrayIndexOutOfBoundsException` whatever the arguments (`{-0}` and
    /// `{+1}` are indexes 0 and 1); a width or precision too large for an
    /// int raises `ValueError` for literals and with `!r` / `!s` (not for a
    /// `datetime`, whose `__format__` takes `strftime` codes); literal
    /// `*args` and `**kwargs` are counted. Not modelled, so not reported
    /// although Jython raises: a spec applied to an attribute or item of
    /// the argument (`{0.real:2147483648}`) or built from another field.
    #[test]
    fn negative_indexes_large_specs_and_literal_star_arguments() {
        for bad in [
            "x = '{-1}'.format(1, 2)\n",
            "x = u'{-1}'.format(**{'-1': 5})\n",
            "x = '{-2147483648.real}'.format(*args)\n",
            "x = '{0:2147483648}'.format(1)\n",
            "x = '{0:.2147483648}'.format(1.0)\n",
            "x = '{:x<02147483648}'.format('a')\n",
            "x = '{a:-2147483648}'.format(a=[1])\n",
            "x = '{0:,.0000000000002147483648}'.format(1)\n",
            "x = '{0!r:2147483648}'.format(d)\n",
            "x = '{0:2147483648}'.format(*(1,))\n",
            "x = '{a:2147483648}'.format(**{'a': 'x'})\n",
            "x = '{0}{1}'.format(*(1,))\n",
            "x = '{0}{1}'.format(*[])\n",
            "x = '{a}'.format(1, **{})\n",
            "x = '{a}'.format(**{'b': 1})\n",
            "x = '{2147483648}'.format(**{})\n",
        ] {
            assert_eq!(lint_module("SCR112", bad).len(), 1, "{bad}");
        }
        for ok in [
            "x = '{-0}{+1}'.format(1, 2)\n",
            "x = '{0:2147483647}'.format(d)\n",
            "x = '{0:2147483648}'.format(d)\n",
            "x = '{0.real:2147483648}'.format(1)\n",
            "x = '{0:{1}}'.format(1, 2147483648)\n",
            "x = '{0:2147483647.2147483647}'.format(d)\n",
            "x = '{0}{1}'.format(1, *(2,))\n",
            "x = '{a}'.format(**{'a': 1})\n",
            "x = '{a}'.format(**{k: 1})\n",
            "x = '{0}{1}'.format(*xs)\n",
        ] {
            assert!(lint_module("SCR112", ok).is_empty(), "{ok}");
        }
        assert_eq!(spec_number_too_large("2147483647"), Some(false));
        assert_eq!(spec_number_too_large("02147483648"), Some(true));
        assert_eq!(spec_number_too_large("<2147483648"), Some(true));
        assert_eq!(spec_number_too_large("%Y-%m-%d"), Some(false));
        assert_eq!(spec_number_too_large("\u{663}"), None);
        assert!(lint_module("SCR112", "x = '%*%' % (1,)\n").is_empty());
    }
}
