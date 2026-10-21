//! BIND011 expression-type-mismatch.

use iglint_expression::{BinaryOp, Expr, ExprKind};
use iglint_project::EmbeddedExpression;

use super::expression_file;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ExpressionRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND011",
    name: "expression-type-mismatch",
    category: Category::Bind,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Obvious literal type errors in expressions",
    explanation: r#"## What it does

Reports operations whose result is wrong whatever the inputs are, judged
from literals only:

* arithmetic (`-`, `*`, `/`, `%`, `^`) with a string literal that is not a
  number, such as `{x} * "two"`; Ignition fails to coerce the string at run
  time (`+` is string concatenation and is not reported);
* comparison with `null` through `=` or `!=`, such as `{x} = null`; use
  `isNull({x})`, which says what is meant and does not depend on how the
  operand's type coerces.

The rule never guesses the types of references or function results, which
is why it only covers literals. Off by default: it is a style aid more than
a bug finder.

## Why is this bad?

The binding evaluates to an error quality (arithmetic on text) or to a
comparison that does not test for null the way it reads.

## Example

```text
{this.custom.count} * "2x"
if({view.params.item} = null, "none", {view.params.item})
```

Use instead:

```text
{this.custom.count} * 2
if(isNull({view.params.item}), "none", {view.params.item})
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Expression(&ExpressionTypeMismatch)],
    options: None,
};

struct ExpressionTypeMismatch;

fn non_numeric_string(e: &Expr) -> bool {
    match &e.unparenthesized().kind {
        ExprKind::String(s) => s.trim().parse::<f64>().is_err(),
        _ => false,
    }
}

fn is_null(e: &Expr) -> bool {
    matches!(e.unparenthesized().kind, ExprKind::Null)
}

impl ExpressionRule for ExpressionTypeMismatch {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        if !e.parsed.is_valid() {
            return;
        }
        let mut found = Vec::new();
        e.parsed.expr.walk(&mut |x| {
            let ExprKind::Binary(b) = &x.kind else {
                return;
            };
            match b.op {
                BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod | BinaryOp::Pow
                    if non_numeric_string(&b.left) || non_numeric_string(&b.right) =>
                {
                    found.push((
                        b.op_range,
                        format!(
                            "Arithmetic `{}` on a string literal that is not a number",
                            b.op.symbol()
                        ),
                    ));
                }
                BinaryOp::Eq | BinaryOp::NotEq if is_null(&b.left) || is_null(&b.right) => {
                    let negate = if b.op == BinaryOp::NotEq { "!" } else { "" };
                    found.push((
                        b.op_range,
                        format!(
                            "Comparison with `null` through `{}`; use `{negate}isNull(...)`",
                            b.op.symbol()
                        ),
                    ));
                }
                _ => {}
            }
        });
        if found.is_empty() {
            return;
        }
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        for (range, message) in found {
            cx.report_expression(file, e, range, message)
                .node_path(e.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(expr: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"propConfig": {{"custom.x": {{"binding": {{"type": "expr", "config": {{"expression": {}}}}}}}}}}}"#,
            serde_json::to_string(expr).unwrap()
        ))
    }

    #[test]
    fn literal_type_errors() {
        let d = view("{view.custom.a} * 'two' + ({view.custom.b} != null)").lint(&["BIND011"]);
        assert_eq!(flagged(&d), vec!["*", "!="]);
        assert!(d[1].message.contains("!isNull"));
    }

    #[test]
    fn coercible_and_concatenation() {
        assert!(
            view("{view.custom.a} * '2' + 'a' + 'b' + isNull({view.custom.b})")
                .lint(&["BIND011"])
                .is_empty()
        );
    }
}
