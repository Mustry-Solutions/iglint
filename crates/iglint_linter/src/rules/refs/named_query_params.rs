//! REF005 named-query-parameter-mismatch.

use iglint_project::Lookup;
use iglint_project::view::{Binding, BindingConfig};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::{Arg, dict_keys, string_value};

pub static META: RuleMeta = RuleMeta {
    code: "REF005",
    name: "named-query-parameter-mismatch",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Named query parameters passed do not match the named query",
    explanation: r#"## What it does

Compares the `parameters` of every query binding, and the parameters dict
literal (`{'line': 1}` or `dict(line=1)`, written in the call) of every
script call of `system.db.runNamedQuery`, `runSFNamedQuery`, `execQuery`
and `execUpdate`, with the parameters the named query declares: a
parameter the query declares but the binding or call does not pass, and a
parameter it passes but the query does not declare, are both reported.
Calls whose parameters are a variable or are unpacked (`**kw`) are not
checked: their keys are not known.

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
    hooks: &[Hook::Binding(&ParamMismatch), Hook::Script(&ParamMismatch)],
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

fn list(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl ScriptRule for ParamMismatch {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some((model, targets)) = super::script_calls::api_targets(cx, unit) else {
            return;
        };
        let Some(index) = cx.index() else {
            return;
        };
        for (_, target) in targets {
            let ApiTarget::NamedQuery(call) = target else {
                continue;
            };
            let Some(params) = call.params else {
                continue;
            };
            let Some((project, path)) =
                super::script_calls::named_query_lookup(cx, model, index, &call)
            else {
                continue;
            };
            let Some(literal) = string_value(model, path) else {
                continue;
            };
            let query = literal.value.as_str();
            let Lookup::Found {
                facts: Some(facts), ..
            } = index.named_query(project, query)
            else {
                continue;
            };
            let keys = match params {
                Arg::Given(e) => match dict_keys(model, e) {
                    Some(keys) => keys,
                    None => continue,
                },
                Arg::Absent => Vec::new(),
                Arg::Unknown => continue,
            };
            for k in &keys {
                if !facts.parameters.iter().any(|(n, _)| *n == k.value) {
                    let message = format!("Named query `{query}` has no parameter `{}`", k.value);
                    crate::rules::script::report(cx, unit, k.range, message);
                }
            }
            let missing: Vec<&str> = facts
                .parameters
                .iter()
                .map(|(n, _)| n.as_str())
                .filter(|n| !keys.iter().any(|k| k.value == *n))
                .collect();
            if !missing.is_empty() {
                let noun = if missing.len() == 1 {
                    "parameter"
                } else {
                    "parameters"
                };
                let message = format!(
                    "Call does not pass {noun} {} of named query `{query}`",
                    list(&missing)
                );
                crate::rules::script::report(cx, unit, literal.range, message);
            }
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

    #[test]
    fn script_call_sites() {
        use crate::rules::script::testing::{ViewScript, decoded_flagged, view};
        let body = "\tsystem.db.runNamedQuery('Q', {'line': 1, 'lineId': 2})\n\tsystem.db.runNamedQuery('Q', dict(line=1, shift=2))\n\tsystem.db.runNamedQuery('Q')\n\tsystem.db.runNamedQuery('Q', params)\n\tsystem.db.runNamedQuery('Q', **kw)\n\tsystem.db.clearNamedQueryCache('Q')";
        let f = with_query(view(ViewScript::Action, body), "Q", &["line", "shift"]);
        let d = f.lint(&["REF005"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "Call does not pass parameter `shift` of named query `Q`".to_owned(),
                "Named query `Q` has no parameter `lineId`".to_owned(),
                "Call does not pass parameters `line`, `shift` of named query `Q`".to_owned(),
            ]
        );
        assert_eq!(decoded_flagged(&d), vec!["'Q'", "'lineId'", "'Q'"]);
    }
}
