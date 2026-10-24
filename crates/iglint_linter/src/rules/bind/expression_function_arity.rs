//! BIND003 expression-function-arity.

use iglint_project::EmbeddedExpression;

use super::expression_file;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ExpressionRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND003",
    name: "expression-function-arity",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Wrong number of arguments for an expression function",
    explanation: r#"## What it does

Checks the argument count of every call to a known expression function
against the counts the gateway accepts when it compiles the expression
(including functions with optional and repeated arguments).

## Why is this bad?

Ignition rejects the expression at compile time ("wrong number of
arguments"), so the binding never evaluates.

## Example

```text
dateFormat(now(0))
```

Use instead:

```text
dateFormat(now(0), "yyyy-MM-dd HH:mm")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Expression(&ExpressionFunctionArity)],
    options: None,
};

struct ExpressionFunctionArity;

impl ExpressionRule for ExpressionFunctionArity {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        if !e.parsed.is_valid() {
            return;
        }
        let mut bad = Vec::new();
        for call in e.parsed.expr.calls() {
            let Some(f) = iglint_stubs::expression_function_in(&call.name, e.function_context())
            else {
                continue;
            };
            if f.arity.is_some() && !f.accepts_arg_count(call.args.len()) {
                bad.push((call, f));
            }
        }
        if bad.is_empty() {
            return;
        }
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        for (call, f) in bad {
            let n = call.args.len();
            cx.report_expression(
                file,
                e,
                call.name_range,
                format!(
                    "`{}` does not accept {} argument{} (parameters: {})",
                    f.name,
                    n,
                    if n == 1 { "" } else { "s" },
                    if f.params.is_empty() {
                        "none"
                    } else {
                        f.params
                    }
                ),
            )
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
    fn wrong_counts() {
        let d = view("dateFormat(now(0)) + if(1, 2)").lint(&["BIND003"]);
        assert_eq!(flagged(&d), vec!["dateFormat", "if"]);
    }

    #[test]
    fn optional_and_variadic_arguments() {
        assert!(
            view("now() + now(1000) + concat('a') + concat('a', 'b', 'c') + coalesce(1, 2, 3)")
                .lint(&["BIND003"])
                .is_empty()
        );
    }

    #[test]
    fn unknown_functions_are_not_this_rules_business() {
        assert!(view("nope(1, 2, 3)").lint(&["BIND003"]).is_empty());
    }
}
