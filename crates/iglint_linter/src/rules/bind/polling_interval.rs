//! BIND004 polling-interval.

use iglint_expression::{Call, ExprKind};
use iglint_project::{EmbeddedExpression, ExpressionKind};

use super::{expression_file, ign_lint_expression_path};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{ExpressionRule, Hook, ParsedOptions, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND004",
    name: "polling-interval",
    category: Category::Bind,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Expression polls faster than the minimum interval",
    explanation: r#"## What it does

Finds calls of polling expression functions in bindings and reports those
that poll faster than `minimum-interval` milliseconds:

* `now(n)` with a literal `0 < n < minimum-interval`;
* `now()` without an argument, which polls every 1000 ms;
* `runScript(script, n, ...)` with a literal poll rate `0 < n < minimum-interval`.

A rate of `0` (or less) means "do not poll" and is fine. A rate that is not
a literal (`now({view.custom.rate})`) cannot be checked and is only
reported with `strict`.

Calls are found by parsing the expression, so `snow(5)` and `'now()'`
inside a string literal are not calls of `now`. Every expression of a
binding is checked: expression and expression-structure bindings, tag
bindings in expression mode, query binding parameters, expression
transforms and map transform expressions, in views and session
properties.

## Why is this bad?

Every polling expression re-evaluates on its own timer in the gateway, for
every open session. Fast polling of many bindings is a common cause of
gateway load.

## Example

```text
dateFormat(now(), "HH:mm:ss")
```

Use instead:

```text
dateFormat(now(10000), "HH:mm:ss")
```

## Options

* `minimum-interval`: the smallest allowed poll rate in milliseconds.

## ign-lint

Equivalent of `PollingIntervalRule` (kwarg `minimum_interval`), with the
same message in `iglint ign-lint`. Differences: ign-lint matches text, so it
also reports `snow(5)`, `now()` inside string literals and `now(-1)`, and
ignores a dynamic `now({...})` when the same expression also has a literal
`now(n)`; it reads indirect tag binding references (property paths, not
expressions) as if they could poll. It reports one finding per string,
iglint one per expression at the first offending call.
"#,
    options: &[OptionMeta {
        name: "minimum-interval",
        ty: OptionType::Integer,
        default: "10000",
        description: "Smallest allowed poll rate in milliseconds.",
    }],
    ign_lint_names: &["PollingIntervalRule"],
    ign_lint_option_map: &[("minimum_interval", "minimum-interval")],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Expression(&PollingInterval)],
    options: Some(parse_options),
};

struct Options {
    minimum: i64,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        minimum: r.int("minimum-interval")?,
    }))
}

/// The poll rate of a call, when it polls.
enum Rate {
    /// A literal rate in milliseconds (`now()` is 1000).
    Literal(f64),
    /// A rate that is not a literal.
    Dynamic,
}

fn literal_number(e: &iglint_expression::Expr) -> Option<f64> {
    match &e.unparenthesized().kind {
        ExprKind::Integer(i) => Some(*i as f64),
        ExprKind::Float(f) => Some(*f),
        ExprKind::Unary(u) if u.op == iglint_expression::UnaryOp::Neg => {
            literal_number(&u.operand).map(|n| -n)
        }
        _ => None,
    }
}

fn rate(call: &Call) -> Option<Rate> {
    let arg = if call.is("now") {
        match call.args.first() {
            None => return Some(Rate::Literal(1000.0)),
            Some(a) => a,
        }
    } else if call.is("runScript") {
        call.args.get(1)?
    } else {
        return None;
    };
    Some(match literal_number(arg) {
        Some(n) => Rate::Literal(n),
        None => Rate::Dynamic,
    })
}

struct PollingInterval;

impl ExpressionRule for PollingInterval {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        // Bindings only (views and session properties).
        if e.binding.is_none() || e.kind == ExpressionKind::PollingRate {
            return;
        }
        let minimum = cx.options::<Options>().minimum as f64;
        let strict = cx.strict();
        let offending = e.parsed.expr.calls().into_iter().find_map(|call| {
            let message = match rate(call)? {
                Rate::Literal(n) if n > 0.0 && n < minimum => {
                    if call.is("now") && call.args.is_empty() {
                        format!(
                            "`{}()` without an interval polls every 1000 ms, faster than the minimum of {} ms",
                            call.name, minimum
                        )
                    } else {
                        format!(
                            "`{}` polls every {} ms, faster than the minimum of {} ms",
                            call.name, n, minimum
                        )
                    }
                }
                Rate::Dynamic if strict => format!(
                    "`{}` polls at a rate that is not a literal; it cannot be checked against the minimum of {} ms",
                    call.name, minimum
                ),
                _ => return None,
            };
            Some((call, message))
        });
        let Some((call, message)) = offending else {
            return;
        };
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        let ign_path = ign_lint_expression_path(cx.data(), e);
        let ign_message = format!("'{}'", e.text());
        cx.report_expression(file, e, call.name_range, message)
            .node_path(e.node_path.clone())
            .ign_lint_path(ign_path)
            .ign_lint_message(ign_message);
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

    fn found(expr: &str, options: &str) -> Vec<String> {
        flagged(&view(expr).lint_with(&[("BIND004", options)]))
    }

    #[test]
    fn fast_and_default_polling() {
        assert_eq!(found("now()", ""), vec!["now"]);
        assert_eq!(found("dateFormat(NOW(500), 'x')", ""), vec!["NOW"]);
        assert_eq!(found("runScript('a.b', 100)", ""), vec!["runScript"]);
        assert_eq!(found("now ( 2000 )", ""), vec!["now"]);
    }

    #[test]
    fn slow_disabled_and_non_calls() {
        assert!(found("now(10000) + now(0) + now(-1)", "").is_empty());
        assert!(found("snow(5)", "").is_empty());
        assert!(found("'call now() later'", "").is_empty());
        assert!(found("runScript('a.b')", "").is_empty());
    }

    #[test]
    fn mixed_calls_report_once_at_the_first_offender() {
        let d = view("now(20000) + now(500) + now(1)").lint(&["BIND004"]);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("500"));
        assert_eq!(
            d[0].ign_lint_message.as_deref(),
            Some("'now(20000) + now(500) + now(1)'")
        );
        assert_eq!(d[0].ign_lint_path.as_deref(), Some("propConfig.custom.x"));
    }

    #[test]
    fn dynamic_rates_only_in_strict_mode() {
        assert!(found("now({view.custom.rate})", "").is_empty());
    }

    #[test]
    fn minimum_interval_option() {
        assert!(found("now(5000)", "minimum-interval = 1000").is_empty());
        assert_eq!(found("now(5000)", "minimum-interval = 30000"), vec!["now"]);
        assert_eq!(found("now(999)", "minimum-interval = 1000"), vec!["now"]);
        assert!(found("now(1000)", "minimum-interval = 1000").is_empty());
    }

    #[test]
    fn struct_leaves_query_parameters_and_transforms() {
        let v = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.label", "meta": {"name": "L"}, "propConfig": {
              "props.text": {"binding": {"type": "expr-struct", "config": {"struct": {"a": "now()", "b": "1"}}}},
              "props.data": {"binding": {"type": "query", "config": {"queryPath": "q", "parameters": {"t": "now(10)"}}}},
              "custom.c": {"binding": {"type": "property", "config": {"path": "this.props.text"},
                  "transforms": [{"type": "expression", "expression": "now(1)"}]}}}}]}}"#;
        let d = Fixture::view(v).lint(&["BIND004"]);
        let mut paths: Vec<String> = d.iter().map(|d| d.ign_lint_path.clone().unwrap()).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "root.root.children[0].L.propConfig.custom.c.binding.transforms[0]",
                "root.root.children[0].L.propConfig.props.data.t",
                "root.root.children[0].L.propConfig.props.text.a",
            ]
        );
    }
}
