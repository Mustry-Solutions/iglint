//! REF005 named-query-parameter-mismatch.

use iglint_project::Lookup;
use iglint_project::view::{Binding, BindingConfig};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF005",
    name: "named-query-parameter-mismatch",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Query binding parameters do not match the named query",
    explanation: r#"## What it does

Compares the `parameters` of every query binding with the parameters the
named query declares: a parameter the query declares but the binding does
not pass, and a parameter the binding passes but the query does not
declare, are both reported. Calls to `system.db.runNamedQuery` with
literal parameter dictionaries are checked too, as soon as the script
layer reports them.

## Why is this bad?

The Designer fills a query binding's parameters from the named query when
the binding is created; a mismatch means the query changed afterwards. A
missing parameter is passed as null, so the query silently returns the
wrong rows (or fails on `NOT NULL` comparisons); an unknown one is
ignored, which usually hides a renamed parameter.

## Example

Named query `Machines/ByLine` declares `line`:

```json
{"type": "query", "config": {"queryPath": "Machines/ByLine", "parameters": {"lineId": "{view.params.line}"}}}
```

Use instead:

```json
{"type": "query", "config": {"queryPath": "Machines/ByLine", "parameters": {"line": "{view.params.line}"}}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Binding(&ParamMismatch)],
    options: None,
};

struct ParamMismatch;

impl BindingRule for ParamMismatch {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Query(q) = &binding.typed else {
            return;
        };
        let (Some(path), Some((index, project))) = (q.query_path, cx.project_index()) else {
            return;
        };
        let Lookup::Found {
            facts: Some(facts), ..
        } = index.named_query(project, path.value())
        else {
            return;
        };
        let file = cx.primary_file();
        let query = path.value();
        for p in &q.parameters {
            let name = p.name.value();
            if !facts.parameters.iter().any(|(n, _)| n == name) {
                cx.report_in(
                    file,
                    p.name.range(),
                    format!("Named query `{query}` has no parameter `{name}`"),
                )
                .node_path(format!("{}.parameters.{name}", binding.node_path));
            }
        }
        let missing: Vec<&str> = facts
            .parameters
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !q.parameters.iter().any(|p| p.name.value() == *n))
            .collect();
        if !missing.is_empty() {
            let list = missing
                .iter()
                .map(|n| format!("`{n}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let noun = if missing.len() == 1 {
                "parameter"
            } else {
                "parameters"
            };
            cx.report_in(
                file,
                path.range(),
                format!("Query binding does not pass {noun} {list} of named query `{query}`"),
            )
            .node_path(&binding.node_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::refs::unknown_named_query::tests::{query_view, with_query};
    use crate::rules::test_support::*;

    #[test]
    fn unknown_and_missing_parameters() {
        let view = query_view("Q", r#"{"lineId": "1", "shift": "2"}"#);
        let f = with_query(Fixture::view(&view), "Q", &["line", "shift", "day"]);
        let d = f.lint(&["REF005"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "Query binding does not pass parameters `line`, `day` of named query `Q`"
                    .to_owned(),
                "Named query `Q` has no parameter `lineId`".to_owned(),
            ]
        );
        assert_eq!(flagged(&d), vec!["\"Q\"", "\"lineId\""]);
    }

    #[test]
    fn matching_parameters_and_unknown_queries_are_silent() {
        let view = query_view("Q", r#"{"line": "1"}"#);
        assert!(
            with_query(Fixture::view(&view), "Q", &["line"])
                .lint(&["REF005"])
                .is_empty()
        );
        let view = query_view("Other", r#"{"x": "1"}"#);
        assert!(
            with_query(Fixture::view(&view), "Q", &["line"])
                .lint(&["REF005"])
                .is_empty()
        );
    }
}
