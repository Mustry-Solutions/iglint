//! REF004 unknown-named-query.

use iglint_project::Lookup;
use iglint_project::kinds::ResourceKind;
use iglint_project::view::{Binding, BindingConfig};

use super::case_insensitive_match;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::string_values;

pub static META: RuleMeta = RuleMeta {
    code: "REF004",
    name: "unknown-named-query",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Named query does not exist",
    explanation: r#"## What it does

Checks the `queryPath` of every query binding (views and session
properties) against the named queries the project sees, including
inherited ones. Scripts are checked too: literal paths passed to
`system.db.runNamedQuery`, `runSFNamedQuery`, `clearNamedQueryCache`,
`execQuery` and `execUpdate` (through aliases and local names holding a
literal). The project-scoped forms (`runNamedQuery(project, path, ...)`,
`project=` keywords) are checked against the named project when it is in
the workspace (`system.util.getProjectName()` is the script's project). A
call `runNamedQuery(a, b)` whose second argument is neither a string nor a
dict is read as `(path, parameters)` only when `a` cannot be a project
name (it contains `/`).

## Why is this bad?

The binding fails at run time with an error quality and the component
shows no data. A missing named query is usually one that was renamed or
moved to another folder.

Paths with `{...}` placeholders and projects that inherit from a project
outside the workspace are not reported.

## Example

```json
{"type": "query", "config": {"queryPath": "Machines/ByLin"}}
```

Use instead:

```json
{"type": "query", "config": {"queryPath": "Machines/ByLine"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[
        Hook::Binding(&UnknownNamedQuery),
        Hook::Script(&UnknownNamedQuery),
    ],
    options: None,
};

struct UnknownNamedQuery;

impl BindingRule for UnknownNamedQuery {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Query(q) = &binding.typed else {
            return;
        };
        let Some(path) = q.query_path else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        let value = path.value();
        if value.trim().is_empty() || value.contains('{') {
            return;
        }
        if !matches!(index.named_query(project, value), Lookup::Missing) {
            return;
        }
        let message = missing_message(cx, project, value);
        let file = cx.primary_file();
        cx.report_in(file, path.range(), message)
            .node_path(&binding.node_path);
    }
}

fn missing_message(
    cx: &Checker<'_, '_>,
    project: iglint_project::ProjectId,
    value: &str,
) -> String {
    let mut message = format!("Named query `{value}` does not exist");
    if let Some(similar) =
        case_insensitive_match(cx.workspace(), project, ResourceKind::NamedQuery, value)
    {
        message.push_str(&format!(" (did you mean `{similar}`?)"));
    }
    message
}

impl ScriptRule for UnknownNamedQuery {
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
            let Some((project, path)) =
                super::script_calls::named_query_lookup(cx, model, index, &call)
            else {
                continue;
            };
            for literal in string_values(model, path).unwrap_or_default() {
                let value = literal.value.as_str();
                if value.trim().is_empty() || value.contains('{') {
                    continue;
                }
                if matches!(index.named_query(project, value), Lookup::Missing) {
                    let message = missing_message(cx, project, value);
                    crate::rules::script::report(cx, unit, literal.range, message);
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::rules::test_support::*;

    pub(crate) fn query_view(path: &str, params: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "custom": {{"rows": null}},
                "propConfig": {{"custom.rows": {{"binding": {{"type": "query", "config":
                    {{"queryPath": "{path}", "returnFormat": "dataset", "parameters": {params},
                      "polling": {{"enabled": false, "rate": "0"}}}}}}}}}}}}}}"#
        )
    }

    pub(crate) fn with_query(f: Fixture, path: &str, params: &[&str]) -> Fixture {
        let params: Vec<String> = params
            .iter()
            .map(|p| format!(r#"{{"type": "Parameter", "sqlType": 7, "identifier": "{p}"}}"#))
            .collect();
        let manifest = format!(
            r#"{{"scope": "DG", "version": 2, "restricted": false, "overridable": true,
                "files": ["query.sql"], "attributes": {{"useMaxReturnSize": false, "autoBatchEnabled": false,
                "fallbackValue": "", "maxReturnSize": 100, "cacheUnit": "SEC", "type": "Query",
                "enabled": true, "cacheAmount": 1, "cacheEnabled": false, "database": "",
                "fallbackEnabled": false, "lastModificationSignature": "", "permissions": [],
                "parameters": [{}]}}}}"#,
            params.join(", ")
        );
        let dir = format!("projects/p/ignition/named-query/{path}");
        f.file(&format!("{dir}/resource.json"), &manifest)
            .file(&format!("{dir}/query.sql"), "SELECT 1")
    }

    #[test]
    fn reports_missing_query() {
        let f = with_query(
            Fixture::view(&query_view("Machines/Gone", "{}")),
            "Machines/ByLine",
            &[],
        );
        let d = f.lint(&["REF004"]);
        assert_eq!(flagged(&d), vec!["\"Machines/Gone\""]);
        assert_eq!(d[0].node_path.as_deref(), Some("root.custom.rows@binding"));
    }

    #[test]
    fn existing_query_is_fine() {
        let f = with_query(
            Fixture::view(&query_view("Machines/ByLine", "{}")),
            "Machines/ByLine",
            &[],
        );
        assert!(f.lint(&["REF004"]).is_empty());
    }

    #[test]
    fn case_hint() {
        let f = with_query(
            Fixture::view(&query_view("machines/byline", "{}")),
            "Machines/ByLine",
            &[],
        );
        assert!(
            f.lint(&["REF004"])[0]
                .message
                .contains("did you mean `Machines/ByLine`")
        );
    }

    #[test]
    fn script_call_sites() {
        use crate::rules::script::testing::{ViewScript, decoded_flagged, view};
        let body = "\tsystem.db.runNamedQuery('Machines/Gone', {})\n\tsystem.db.runNamedQuery('Machines/ByLine', {})\n\tsystem.db.runNamedQuery(system.util.getProjectName(), 'Machines/Gone2', {})\n\tsystem.db.runNamedQuery('elsewhere', 'Machines/Gone3', {})\n\tsystem.db.runNamedQuery('Machines/Gone4', params)\n\tsystem.db.runNamedQuery('p', params)\n\tsystem.db.runNamedQuery('Top', params)\n\tsystem.db.execQuery(path='Machines/Gone5')\n\tq = system.db.runNamedQuery\n\tq(path='machines/byline')";
        let f = with_query(view(ViewScript::Action, body), "Machines/ByLine", &[]);
        let d = f.lint(&["REF004"]);
        assert_eq!(
            decoded_flagged(&d),
            vec![
                "'Machines/Gone'",
                "'Machines/Gone2'",
                "'Machines/Gone4'",
                "'Machines/Gone5'",
                "'machines/byline'"
            ]
        );
        assert!(d[4].message.contains("did you mean `Machines/ByLine`"));
    }
}
