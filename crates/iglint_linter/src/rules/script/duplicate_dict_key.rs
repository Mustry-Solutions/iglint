//! SCR108 duplicate-dict-key.

use iglint_python::ast::{Expr, Number, StringValue};
use iglint_text::Ranged;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{file_line, report, source, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR108",
    name: "duplicate-dict-key",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Dict literal has the same constant key twice",
    explanation: r#"## What it does

Reports keys of a dict literal that repeat an earlier constant key: equal
strings (a `str` and a `unicode` with the same text are the same key),
equal numbers (`1`, `1L` and `1.0` are the same key) or `None`, `True`,
`False` twice.

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

/// A canonical form of a constant key.
fn key(e: &Expr) -> Option<String> {
    match e {
        Expr::Str(s) => Some(match &s.s {
            StringValue::Unicode(u) => format!("s:{u}"),
            StringValue::Bytes(b) => match std::str::from_utf8(b) {
                // Non-ASCII byte strings differ from unicode strings.
                Ok(t) if t.is_ascii() => format!("s:{t}"),
                _ => format!("b:{b:?}"),
            },
        }),
        Expr::Num(n) => match &n.n {
            Number::Int(i) => Some(format!("n:{i}")),
            Number::Long(l) => Some(format!("n:{}", l.trim_start_matches('+'))),
            Number::Float(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 => {
                Some(format!("n:{}", *f as i64))
            }
            Number::Float(f) if f.is_finite() => Some(format!("f:{f:?}")),
            _ => None,
        },
        Expr::Name(n) if matches!(&*n.id, "None" | "True" | "False") => Some(format!("c:{}", n.id)),
        _ => None,
    }
}

impl ScriptRule for DuplicateDictKey {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let text = source(unit);
        let mut found = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Dict(d) = e else { return };
            let mut seen: FxHashMap<String, &Expr> = FxHashMap::default();
            for k in &d.keys {
                let Some(canonical) = key(k) else { continue };
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
            let message = match file_line(cx, unit, first.start()) {
                Some(line) => format!(
                    "Duplicate key {shown} in dict literal (first on line {line}); only the last value is kept"
                ),
                None => {
                    format!("Duplicate key {shown} in dict literal; only the last value is kept")
                }
            };
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
                "d = {'a': 1, 'A': 2, x: 1, x: 2, 1: 0, '1': 1, True: 1, 2: 2}\n"
            )
            .is_empty()
        );
    }
}
