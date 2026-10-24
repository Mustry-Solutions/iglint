//! SCR110 comparison-with-literal-identity.

use iglint_python::ast::{CmpOp, Expr};
use iglint_text::{Ranged, TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{fix, report, source, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR110",
    name: "comparison-with-literal-identity",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    fix_safety: crate::registry::FixSafety::Unsafe,
    summary: "`is` / `is not` with a string or number literal",
    explanation: r#"## What it does

Reports `x is 'text'`, `x is 1`, `x is not ''` and the like: identity
comparisons where one side is a string or number literal. Comparisons with
`None`, `True` and `False` are fine.

## Why is this bad?

`is` compares object identity, not value. Whether two equal strings or
numbers are the same object is an implementation detail; in Jython it
differs between interned literals, values read from tags or databases
(Java strings converted to Python), and small and large integers. The
comparison works in testing and fails in production.

## Example

```python
if event.source.props.text is "Start":
    start()
```

Use instead:

```python
if event.source.props.text == "Start":
    start()
```

## Fix

Replaces `is` with `==` and `is not` with `!=`. Unsafe: the result
changes whenever identity and equality differ, which is the bug being
fixed (in Jython `int('1000') is 1000` is `False`, `==` gives `True`),
and `==` calls the operand's `__eq__` (Java `equals`).

## pylint

Corresponds to `R0123 literal-comparison`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&LiteralIdentity)],
    options: None,
};

struct LiteralIdentity;

fn is_literal(e: &Expr) -> bool {
    matches!(e, Expr::Str(_) | Expr::Num(_))
}

/// The range of the `is` / `is not` operator between two operands.
fn operator_range(text: &str, from: TextSize, to: TextSize, negated: bool) -> Option<TextRange> {
    let between = &text[from.to_usize()..to.to_usize()];
    let bytes = between.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'i' if between[i..].starts_with("is")
                && (i == 0 || !ident(bytes[i - 1]))
                && !bytes.get(i + 2).copied().is_some_and(ident) =>
            {
                let mut end = i + 2;
                if negated {
                    let rest = &between[end..];
                    let skipped =
                        rest.len() - rest.trim_start_matches([' ', '\t', '\\', '\n', '\r']).len();
                    if !rest[skipped..].starts_with("not") {
                        return None;
                    }
                    end += skipped + 3;
                }
                return Some(TextRange::new(
                    from + TextSize::from(i),
                    from + TextSize::from(end),
                ));
            }
            _ => i += 1,
        }
    }
    None
}

impl ScriptRule for LiteralIdentity {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let text = source(unit);
        let mut found = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Compare(c) = e else { return };
            let mut left: &Expr = &c.left;
            for (op, right) in c.ops.iter().zip(&c.comparators) {
                if matches!(op, CmpOp::Is | CmpOp::IsNot) && (is_literal(left) || is_literal(right))
                {
                    let negated = *op == CmpOp::IsNot;
                    let op_range =
                        operator_range(text, left.range().end(), right.range().start(), negated);
                    found.push((left.range().cover(right.range()), negated, op_range));
                }
                left = right;
            }
        });
        for (range, negated, op_range) in found {
            let (op, replacement) = if negated {
                ("is not", "!=")
            } else {
                ("is", "==")
            };
            let fix = op_range.and_then(|r| {
                fix(
                    cx,
                    unit,
                    format!("Replace `{op}` with `{replacement}`"),
                    Applicability::Unsafe,
                    vec![(r, replacement.to_owned())],
                )
                .map(|f| {
                    f.reason_if_unsafe(
                        "`==` calls `__eq__` and can give a different result than the identity \
                         test",
                    )
                })
            });
            let _ = report(
                cx,
                unit,
                range,
                format!("`{op}` compares identity with a literal; use `{replacement}`"),
            )
            .fix_opt(fix);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_and_fixes() {
        let d = lint_module(
            "SCR110",
            "if x is 'a' or 1 is not y or x is None:\n    pass\n",
        );
        assert_eq!(flagged(&d), ["x is 'a'", "1 is not y"]);
        let f = library("m", "if (x) is ('a'):\n    pass\nz = y is  not  -1\n");
        let fixed = f.fixed(&[("SCR110", "")], true, &library_path(&f, "m"));
        assert_eq!(fixed, "if (x) == ('a'):\n    pass\nz = y !=  -1\n");
    }

    #[test]
    fn fix_inside_view_json() {
        let f = view(ViewScript::Transform, "\treturn value is 'on'");
        let fixed = f.fixed(&[("SCR110", "")], true, &view_path(&f));
        assert!(
            fixed.contains("\"code\": \"\\treturn value \\u003d\\u003d \\u0027on\\u0027\""),
            "{fixed}"
        );
    }

    #[test]
    fn identity_with_singletons_is_fine() {
        assert!(lint_module("SCR110", "a = x is None or x is not True or x is y\n").is_empty());
    }
}
