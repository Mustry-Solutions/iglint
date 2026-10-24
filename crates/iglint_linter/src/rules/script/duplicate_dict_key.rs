//! SCR108 duplicate-dict-key.

use iglint_python::ast::{Expr, ExprName, Number, Operator, StringValue, UnaryOp};
use iglint_text::Ranged;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{file_line, report, semantic, source, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR108",
    name: "duplicate-dict-key",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Dict literal has the same constant key twice",
    explanation: r#"## What it does

Reports keys of a dict literal that repeat an earlier constant key, with
Python 2.7 equality:

* numbers by value: `1`, `1L`, `1.0`, `True` and `1+0j` are the same key,
  as are `0`, `-0.0`, `0j` and `False`; `1e16` equals `10000000000000000`
  but not `10000000000000001`;
* strings: a `str` and a `unicode` with the same ASCII text are the same
  key; a `str` with non-ASCII bytes never equals a `unicode` (`'\xe9'` and
  `u'\xe9'` are two keys);
* `None`, and tuples of such constants (`(1, 2)` and `(1.0, 2L)`).

`True`, `False` and `None` count only where they are Jython's builtins: in
Python 2 `True = 2` makes `True` an ordinary variable, and `from x import
*`, `exec` or writes through `globals()` may rebind them.

Number literals with `+`, `-` or `~` in front count as constants; other
expressions do not.

## Why is this bad?

Only the last value is kept; the earlier entry is silently lost. In
Ignition this often hides a mistake in a payload, a column mapping or a
style dict.

## Example

```python
payload = {
    "line": line,
    "machine": machine,
    "line": station,
}
```

Use instead:

```python
payload = {
    "line": line,
    "machine": machine,
    "station": station,
}
```

## pylint

Corresponds to `W0109 duplicate-key`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&DuplicateDictKey)],
    options: None,
};

struct DuplicateDictKey;

/// A constant dict key, canonical under Python 2.7 equality: two keys are
/// the same dict key exactly when their `Key`s are equal (equal values
/// also hash equal in Python, so `==` decides).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    /// An integer, a bool, an integral float or a complex with zero
    /// imaginary part and integral real part, as a decimal string without
    /// leading zeros (`0` has no sign).
    Integer(String),
    /// A finite non-integral float or an infinity (bits; never NaN or -0.0).
    Float(u64),
    /// A complex with a non-zero imaginary part: real and imaginary bits,
    /// zeros without sign.
    Complex(u64, u64),
    /// A `unicode`, or a `str` whose bytes are all ASCII (they compare
    /// equal to the `unicode` with the same text).
    Text(String),
    /// A `str` with non-ASCII bytes: equal only to the same bytes
    /// (`'\xe9' != u'\xe9'`).
    Bytes(Box<[u8]>),
    None,
    Tuple(Vec<Key>),
}

/// A number while folding unary operators and `real + imaginary`.
#[derive(Clone, Copy, Debug)]
enum Num<'a> {
    /// Decimal integer, optional leading minus.
    Integer(&'a str),
    Small(i128),
    Float(f64),
    Complex(f64, f64),
}

impl Num<'_> {
    fn integer(&self) -> Option<i128> {
        match *self {
            Num::Small(i) => Some(i),
            Num::Integer(s) => s.parse().ok(),
            _ => None,
        }
    }

    fn to_f64(self) -> Option<f64> {
        match self {
            Num::Small(i) => Some(i as f64),
            // Correctly rounded, like Python's int-to-float conversion.
            Num::Integer(s) => s.parse().ok(),
            Num::Float(f) => Some(f),
            Num::Complex(..) => None,
        }
    }

    fn negate(self) -> Option<Num<'static>> {
        Some(match self {
            Num::Small(i) => Num::Small(i.checked_neg()?),
            Num::Integer(s) => Num::Small(-s.parse::<i128>().ok()?),
            Num::Float(f) => Num::Float(-f),
            Num::Complex(re, im) => Num::Complex(-re, -im),
        })
    }

    fn key(self) -> Key {
        match self {
            Num::Small(i) => Key::Integer(i.to_string()),
            Num::Integer(s) => Key::Integer(normalize_integer(s)),
            Num::Float(f) => float_key(f),
            Num::Complex(re, im) => {
                if im == 0.0 {
                    float_key(re)
                } else {
                    Key::Complex(zero_bits(re), zero_bits(im))
                }
            }
        }
    }
}

fn zero_bits(f: f64) -> u64 {
    if f == 0.0 { 0 } else { f.to_bits() }
}

fn normalize_integer(s: &str) -> String {
    match s.strip_prefix('-') {
        Some(digits) if digits.trim_start_matches('0').is_empty() => "0".to_owned(),
        _ => s.to_owned(),
    }
}

/// An integral float compares equal to the integer with exactly its value
/// (`1e16 == 10000000000000000`, but not `10000000000000001`).
fn float_key(f: f64) -> Key {
    if f.is_finite() && f.fract() == 0.0 {
        Key::Integer(exact_integer(f))
    } else {
        Key::Float(zero_bits(f))
    }
}

/// The exact decimal value of an integral finite float.
fn exact_integer(f: f64) -> String {
    if f.abs() < 1e38 {
        return (f as i128).to_string();
    }
    // mantissa * 2^exponent, in base 10^9 limbs (little endian).
    let bits = f.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32 - 1075;
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    let mut limbs: Vec<u64> = vec![mantissa % 1_000_000_000, mantissa / 1_000_000_000];
    for _ in 0..exponent {
        let mut carry = 0;
        for limb in &mut limbs {
            let v = *limb * 2 + carry;
            *limb = v % 1_000_000_000;
            carry = v / 1_000_000_000;
        }
        if carry > 0 {
            limbs.push(carry);
        }
    }
    while limbs.len() > 1 && limbs.last() == Some(&0) {
        limbs.pop();
    }
    let mut out = if f < 0.0 {
        "-".to_owned()
    } else {
        String::new()
    };
    let mut rev = limbs.iter().rev();
    if let Some(top) = rev.next() {
        out.push_str(&top.to_string());
    }
    for limb in rev {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

/// A numeric constant: a number literal, `True`/`False`, unary `+`, `-`
/// and `~` on one, and `real + imaginary` / `real - imaginary`.
/// `builtin` tells whether a name reads Jython's builtin (`True = 2` makes
/// `True` an ordinary variable).
fn number<'e>(e: &'e Expr, builtin: &dyn Fn(&ExprName) -> bool) -> Option<Num<'e>> {
    match e {
        Expr::Num(n) => Some(match &n.n {
            Number::Int(i) => Num::Small(i128::from(*i)),
            Number::Long(l) => Num::Integer(l.trim_start_matches('+')),
            Number::Float(f) => Num::Float(*f),
            Number::Complex(im) => Num::Complex(0.0, *im),
        }),
        Expr::Name(n) if &*n.id == "True" && builtin(n) => Some(Num::Small(1)),
        Expr::Name(n) if &*n.id == "False" && builtin(n) => Some(Num::Small(0)),
        Expr::UnaryOp(u) => {
            let operand = number(&u.operand, builtin)?;
            match u.op {
                UnaryOp::UAdd => Some(operand),
                UnaryOp::USub => operand.negate(),
                UnaryOp::Invert => Some(Num::Small(-operand.integer()?.checked_add(1)?)),
                UnaryOp::Not => None,
            }
        }
        Expr::BinOp(b) if matches!(b.op, Operator::Add | Operator::Sub) => {
            let Expr::Num(right) = &*b.right else {
                return None;
            };
            let Number::Complex(im) = right.n else {
                return None;
            };
            let re = number(&b.left, builtin)?.to_f64()?;
            Some(match b.op {
                Operator::Add => Num::Complex(re, im),
                _ => Num::Complex(re, -im),
            })
        }
        _ => None,
    }
}

/// The canonical form of a constant key.
fn key(e: &Expr, builtin: &dyn Fn(&ExprName) -> bool) -> Option<Key> {
    match e {
        Expr::Str(s) => Some(match &s.s {
            StringValue::Unicode(u) => Key::Text(u.to_string()),
            StringValue::Bytes(b) if b.is_ascii() => {
                Key::Text(String::from_utf8_lossy(b).into_owned())
            }
            StringValue::Bytes(b) => Key::Bytes(b.clone()),
        }),
        Expr::Name(n) if &*n.id == "None" && builtin(n) => Some(Key::None),
        Expr::Tuple(t) => t
            .elts
            .iter()
            .map(|e| key(e, builtin))
            .collect::<Option<_>>()
            .map(Key::Tuple),
        _ => number(e, builtin).map(Num::key),
    }
}

impl ScriptRule for DuplicateDictKey {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let builtin = |n: &ExprName| super::reads_builtin(model, n);
        let text = source(unit);
        let mut found = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Dict(d) = e else { return };
            let mut seen: FxHashMap<Key, &Expr> = FxHashMap::default();
            for k in &d.keys {
                let Some(canonical) = key(k, &builtin) else {
                    continue;
                };
                match seen.get(&canonical) {
                    Some(first) => found.push((first.range(), k.range())),
                    None => {
                        seen.insert(canonical, k);
                    }
                }
            }
        });
        for (first, dup) in found {
            let shown = dup.slice(text).to_owned();
            let first_shown = first.slice(text);
            // `True` and `1` are the same key: say which one it repeats.
            let equal = (first_shown != shown).then(|| format!("equal to {first_shown}"));
            let first_line =
                file_line(cx, unit, first.start()).map(|l| format!("first on line {l}"));
            let note = match (equal, first_line) {
                (Some(e), Some(l)) => format!(" ({e}, {l})"),
                (Some(x), None) | (None, Some(x)) => format!(" ({x})"),
                (None, None) => String::new(),
            };
            let message =
                format!("Duplicate key {shown} in dict literal{note}; only the last value is kept");
            let label = unit.script.file_range(first);
            let file = unit.file;
            let _ = report(cx, unit, dup, message)
                .label(file, label, "first occurrence")
                .fingerprint_key(format!("Duplicate key {shown}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_duplicates() {
        let d = lint_module(
            "SCR108",
            "d = {'a': 1, 'b': 2, u'a': 3, 1: 0, 1.0: 1, None: 1, None: 2}\n",
        );
        assert_eq!(flagged(&d), ["u'a'", "1.0", "None"]);
        assert!(d[0].message.contains("first on line 1"));
    }

    #[test]
    fn named_escapes_are_resolved() {
        let d = lint_module(
            "SCR108",
            "d = {u'\\N{PILE OF POO}': 1, u'\\U0001f4a9': 2}\n",
        );
        assert_eq!(flagged(&d), ["u'\\U0001f4a9'"]);
    }

    #[test]
    fn distinct_keys_are_fine() {
        assert!(
            lint_module(
                "SCR108",
                "d = {'a': 1, 'A': 2, x: 1, x: 2, 1: 0, '1': 1, 2: 2, None: 3, (): 4}\n"
            )
            .is_empty()
        );
    }

    /// Each case was checked with Jython 2.7 in the Ignition 8.3.3 image
    /// (`len()` of the dict, compiled like Ignition compiles scripts).
    #[test]
    fn python_equality() {
        let duplicates = [
            "{True: 1, 1: 2}",
            "{False: 1, 0: 2}",
            "{True: 1, 1.0: 2}",
            "{0: 1, 0j: 2}",
            "{0: 1, -0.0: 2}",
            "{-0j: 1, 0: 2}",
            "{1: 1, 1+0j: 2}",
            "{1-0j: 1, 1: 2}",
            "{1j: 1, 1j: 2}",
            "{1+2j: 1, 1.0+2j: 2}",
            "{(1, 2): 1, (1, 2): 2}",
            "{(1, 2): 1, (1.0, 2L): 2}",
            "{(True, 2): 1, (1, 2): 2}",
            "{(1, (2, 3)): 1, (1, (2.0, 3)): 2}",
            "{(): 1, (): 2}",
            "{(1, 'a'): 1, (1, u'a'): 2}",
            "{'\\xe9': 1, '\\xe9': 2}",
            "{u'\\xe9': 1, u'\\xe9': 2}",
            "{'\u{e9}': 1, '\\xc3\\xa9': 2}",
            "{u'\u{e9}': 1, u'\\xe9': 2}",
            "{-1: 1, -1L: 2}",
            "{-1: 1, -1.0: 2}",
            "{-0: 1, 0: 2}",
            "{+1: 1, 1: 2}",
            "{-True: 1, -1: 2}",
            "{~1: 1, -2: 2}",
            "{1e16: 1, 10000000000000000: 2}",
            "{1e16: 1, 10000000000000000L: 2}",
            "{123456789012345678901234567890: 1, 123456789012345678901234567890L: 2}",
            "{1e300: 1, 1e300: 2}",
            "{1e999: 1, 2e999: 2}",
            "{-1e999: 1, -1e999: 2}",
            "{0.1: 1, 0.1000000000000000055511151231257827: 2}",
            "{1: 1, 1.0000000000000001: 2}",
            "{0x10: 1, 16: 2}",
            "{010: 1, 8: 2}",
            "{0b11: 1, 3: 2}",
            "{0xffffffffffffffff: 1, 18446744073709551615: 2}",
            "{'a' 'b': 1, 'ab': 2}",
            "{r'\\n': 1, '\\\\n': 2}",
            "{99999999999: 1, 99999999999.0: 2}",
        ];
        for case in duplicates {
            let d = lint_module("SCR108", &format!("d = {case}\n"));
            assert_eq!(d.len(), 1, "{case}");
        }
        let distinct = [
            "{1j: 1, 2j: 2}",
            "{'\\xe9': 1, u'\\xe9': 2}",
            "{'\u{e9}': 1, u'\u{e9}': 2}",
            "{(1, '\\xe9'): 1, (1, u'\\xe9'): 2}",
            "{1e16: 1, 10000000000000001: 2}",
            "{9007199254740993: 1, 9007199254740992.0: 2}",
            "{1e999: 1, -1e999: 2}",
            "{0.5: 1, 1: 2}",
            "{(1, 2): 1, (1, 2, 3): 2}",
            "{(1, x): 1, (1, x): 2}",
            "{1: 1, 1 + 0: 2}",
            "{2**53: 1, 9007199254740992.0: 2}",
        ];
        for case in distinct {
            assert!(
                lint_module("SCR108", &format!("d = {case}\n")).is_empty(),
                "{case}"
            );
        }
    }

    #[test]
    fn exact_integers_of_large_floats() {
        use super::exact_integer;
        assert_eq!(exact_integer(-0.0), "0");
        assert_eq!(exact_integer(1e16), "10000000000000000");
        assert_eq!(
            exact_integer(2f64.powi(200)),
            "1606938044258990275541962092341162602522202993782792835301376"
        );
        assert_eq!(
            exact_integer(-1.5e308),
            "-150000000000000001646859544416068312610738464515967769505216024354736378107367237305744993467742033348591874504581758773417385425614710132492460513797219069046907496754399540778546234549382838565007576699247501767640346439319417766442136793680061187184175955257676622657937704339811809909462328573145334677504"
        );
    }

    #[test]
    fn message_names_the_equal_key() {
        let d = lint_module("SCR108", "d = {True: 1,\n     1: 2}\n");
        assert_eq!(flagged(&d), ["1"]);
        assert!(
            d[0].message
                .contains("Duplicate key 1 in dict literal (equal to True, first on line 1)"),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn rebound_true_is_an_ordinary_name() {
        assert!(lint_module("SCR108", "True = 2\nd = {True: 'two', 1: 'one'}\n").is_empty());
        assert!(lint_module("SCR108", "def f(False):\n    return {False: 1, 0: 2}\n").is_empty());
        assert!(lint_module("SCR108", "from m import *\nd = {True: 1, 1: 2}\n").is_empty());
        assert_eq!(lint_module("SCR108", "d = {True: 'a', 1: 'b'}\n").len(), 1);
    }
}
