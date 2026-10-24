//! BIND002 unknown-expression-function.

use iglint_project::EmbeddedExpression;

use super::expression_file;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{ExpressionRule, Hook, ParsedOptions, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND002",
    name: "unknown-expression-function",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Function not registered on the gateway",
    explanation: r#"## What it does

Reports calls to expression functions that Ignition 8.3 does not register.
Function names are matched case-insensitively, as Ignition does. The list
of functions is generated from a gateway with the standard modules; add
functions of third-party modules with `extra-functions`.

The functions depend on where the expression runs: Perspective bindings,
transforms and the other expressions of a view can also call
`isAuthorized` and `property`, which tag expressions, derived tags and
alarm and tag property bindings cannot.

Expressions with a syntax error are left to `BIND001`.

## Why is this bad?

The expression fails to compile with "function not found" and the binding
never produces a value.

## Example

```text
dateFormatt(now(0), "yyyy-MM-dd")
```

Use instead:

```text
dateFormat(now(0), "yyyy-MM-dd")
```

## Options

* `extra-functions`: names of additional functions provided by modules
  (case-insensitive). Listing a function here only silences this rule: an
  expression function that Ignition 8.3 does not register may run code
  (Vision's `objectScript` runs Python), so it still keeps the safe fixes
  of other rules from being safe until the project lists it under
  `[fix] vetted-expression-functions`.
"#,
    options: &[OptionMeta {
        name: "extra-functions",
        ty: OptionType::StringList,
        default: "[]",
        description: "Additional function names provided by modules.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Expression(&UnknownExpressionFunction)],
    options: Some(parse_options),
};

struct Options {
    extra: Vec<String>,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        extra: r
            .string_list("extra-functions")?
            .into_iter()
            .map(|s| s.to_lowercase())
            .collect(),
    }))
}

struct UnknownExpressionFunction;

impl ExpressionRule for UnknownExpressionFunction {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        if !e.parsed.is_valid() {
            return;
        }
        let calls = e.parsed.expr.calls();
        let unknown: Vec<_> = calls
            .into_iter()
            .filter(|c| {
                iglint_stubs::expression_function_in(&c.name, e.function_context()).is_none()
            })
            .filter(|c| {
                !cx.options::<Options>()
                    .extra
                    .contains(&c.name.to_lowercase())
            })
            .collect();
        if unknown.is_empty() {
            return;
        }
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        for call in unknown {
            let suggestion = crate::suggest::closest(
                &call.name,
                iglint_stubs::expression_functions_in(e.function_context()).map(|f| f.name),
            );
            let message = match suggestion {
                Some(s) => format!(
                    "Unknown expression function `{}` (did you mean `{s}`?)",
                    call.name
                ),
                None => format!("Unknown expression function `{}`", call.name),
            };
            cx.report_expression(file, e, call.name_range, message)
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
    fn unknown_functions_with_suggestion() {
        let d = view("dateFormatt(now(0), 'yyyy') + nope(1)").lint(&["BIND002"]);
        assert_eq!(flagged(&d), vec!["dateFormatt", "nope"]);
        assert!(
            d[0].message.contains("did you mean `dateFormat`"),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn case_insensitive_and_known_functions() {
        assert!(
            view("DATEFORMAT(NOW(0), 'y') + toInt('1') + if(true, 1, 2)")
                .lint(&["BIND002"])
                .is_empty()
        );
    }

    /// Reviewer round 19 (Opus) minor: Perspective's expressions (its
    /// `ExpressionBindingParseContext`) add `isAuthorized` and `property`
    /// to the gateway's functions; tag and alarm expressions have neither.
    /// Measured on an 8.3.3 gateway: in a live session
    /// `property('this.custom.x') + 1` evaluates to 43 for `custom.x = 42`
    /// in an expression binding and in an expression transform, while an
    /// expression tag calling `property` or `isAuthorized` has quality
    /// `Error_Configuration`.
    #[test]
    fn functions_depend_on_the_context() {
        assert!(
            view("property('this.custom.x') + toInt(isAuthorized(false))")
                .lint(&["BIND002"])
                .is_empty()
        );
        let f = crate::rules::tag::fixtures::tags(
            crate::rules::tag::fixtures::gateway(),
            r#"[{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "expression": "property('x') + isAuthorized(true) + now(0)"}]"#,
            "[]",
        );
        assert_eq!(flagged(&f.lint(&["BIND002"])), ["property", "isAuthorized"]);
    }

    #[test]
    fn extra_functions_option_and_invalid_expressions() {
        assert!(
            view("myModuleFn(1)")
                .lint_with(&[("BIND002", "extra-functions = ['MyModuleFn']")])
                .is_empty()
        );
        assert!(view("nope(1) +").lint(&["BIND002"]).is_empty());
    }
}
