//! BIND001 expression-syntax-error.

use iglint_project::EmbeddedExpression;

use super::expression_file;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ExpressionRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND001",
    name: "expression-syntax-error",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Expression does not parse",
    explanation: r#"## What it does

Parses every Ignition expression with the grammar of Ignition 8.3's own
parser and reports the first syntax error, as the gateway would: expression
bindings, expression-structure leaves, expression transforms, tag bindings
in expression mode, query binding parameters, polling rates, map transform
expressions, tag expressions, derived tag expressions and expression-bound
tag and alarm properties.

Locations that may hold either an expression or a literal (tag history tag
paths, HTTP binding URLs) are only treated as expressions when they parse,
so they are never reported.

## Why is this bad?

An expression that does not compile never evaluates: the binding shows an
error quality, the tag reports `Bad_Config`. Nothing warns at save time.

## Example

```text
if({view.params.mode} == 'edit', "Save", "Edit")
```

Use instead (Ignition has no `==`):

```text
if({view.params.mode} = 'edit', "Save", "Edit")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Expression(&ExpressionSyntaxError)],
    options: None,
};

struct ExpressionSyntaxError;

impl ExpressionRule for ExpressionSyntaxError {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        if e.tentative {
            return;
        }
        let Some(error) = e.parsed.errors.first() else {
            return;
        };
        let Some(file) = expression_file(cx.data(), e) else {
            return;
        };
        cx.report_expression(
            file,
            e,
            error.range,
            format!("Expression syntax error: {}", error.kind),
        )
        .node_path(e.node_path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view_with_expr(expr: &str) -> String {
        format!(
            r#"{{"custom": {{}}, "propConfig": {{"custom.x": {{"binding": {{"type": "expr", "config": {{"expression": {}}}}}}}}}, "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#,
            serde_json::to_string(expr).unwrap()
        )
    }

    #[test]
    fn reports_first_error_at_exact_range() {
        let f = Fixture::view(&view_with_expr("if({view.params.a} == 1, 2, 3)"));
        let d = f.lint(&["BIND001"]);
        assert_eq!(d.len(), 1);
        assert!(
            d[0].message.starts_with("Expression syntax error: "),
            "{}",
            d[0].message
        );
        assert_eq!(flagged(&d), vec!["="]);
        assert_eq!(d[0].node_path.as_deref(), Some("view.custom.x@binding"));
    }

    #[test]
    fn maps_through_json_escapes() {
        // `=` is how Ignition writes `=`; the error is the stray `)`.
        let view = r#"{"propConfig": {"custom.x": {"binding": {"type": "expr", "config": {"expression": "1 = 1)"}}}}}"#;
        let d = Fixture::view(view).lint(&["BIND001"]);
        assert_eq!(flagged(&d), vec![")"]);
    }

    #[test]
    fn valid_expressions_and_tentative_locations_are_clean() {
        let f = Fixture::view(&view_with_expr("concat('a', {this.custom.b})")).with_view(
            "Hist",
            r#"{"propConfig": {"custom.h": {"binding": {"type": "tag-history", "config": {"tags": [{"path": "[default]A/B (c)"}]}}}}"#,
        );
        assert!(f.lint(&["BIND001"]).is_empty());
    }

    /// Reviewer round 10, O1: every field Perspective parses as an
    /// expression (measured in the Ignition image, see
    /// `iglint_project::perspective_fields`) is checked. A literal URL is
    /// a configuration error in Perspective ("Error parsing \"url\":
    /// Invalid expression"), so it is reported too; fields Perspective
    /// does not parse in the configuration given (an authorization value
    /// with type `None`) are not.
    #[test]
    fn measured_binding_fields_are_checked() {
        let view = r#"{"propConfig": {
            "custom.h": {"binding": {"type": "http", "config": {"request": {"url": "http://x/{a", "method": "POST",
                "header": [{"key": "Authorization", "value": "'Bearer ' +"}], "body": "{a} ==",
                "auth": {"type": "Basic", "value": "*"}}}}},
            "custom.n": {"binding": {"type": "http", "config": {"request": {"url": "'u'", "method": "GET",
                "auth": {"type": "None", "value": "*"}}}}},
            "custom.t": {"binding": {"type": "tag-history", "config": {"dateRange": {"startDate": "now(", "endDate": "now())"},
                "tags": [{"path": "[default]A"}], "returnSize": {"type": "fixed", "numRows": "1 +"}}}},
            "custom.r": {"binding": {"type": "tag-history", "config": {"dateRange": {"mostRecent": "*", "mostRecentUnits": "min"}, "tags": "("}}},
            "custom.m": {"binding": {"type": "expr", "config": {"expression": "1"},
                "transforms": [{"type": "map", "inputType": "expression", "outputType": "expression",
                    "mappings": [], "fallback": "2 +"}]}},
            "custom.s": {"binding": {"type": "expr-struct", "config": {"struct": {"list": ["1", "2 +"]}}}}}}"#;
        let d = Fixture::view(view).lint(&["BIND001"]);
        let mut paths: Vec<&str> = d.iter().filter_map(|d| d.node_path.as_deref()).collect();
        paths.sort_unstable();
        assert_eq!(
            paths,
            [
                "view.custom.h@binding.request.auth.value",
                "view.custom.h@binding.request.body",
                "view.custom.h@binding.request.header[0].value",
                "view.custom.h@binding.request.url",
                "view.custom.m@binding.transforms[0].fallback",
                "view.custom.r@binding.dateRange.mostRecent",
                "view.custom.r@binding.tags",
                "view.custom.s@binding.struct.list[1]",
                "view.custom.t@binding.dateRange.endDate",
                "view.custom.t@binding.dateRange.startDate",
                "view.custom.t@binding.returnSize.numRows",
            ],
            "{d:#?}"
        );
    }

    #[test]
    fn transforms_struct_leaves_and_query_parameters() {
        let view = r#"{"propConfig": {
            "custom.a": {"binding": {"type": "expr-struct", "config": {"struct": {"k": "1 +"}}}},
            "custom.b": {"binding": {"type": "query", "config": {"queryPath": "q", "parameters": {"p": "("}}}},
            "custom.c": {"binding": {"type": "property", "config": {"path": "view.custom.a"},
                "transforms": [{"type": "expression", "expression": "{value} +* 2"}]}}}}"#;
        let d = Fixture::view(view).lint(&["BIND001"]);
        assert_eq!(d.len(), 3, "{d:?}");
    }
}
