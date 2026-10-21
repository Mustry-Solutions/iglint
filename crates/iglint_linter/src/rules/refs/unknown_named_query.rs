//! REF004 unknown-named-query.

use iglint_project::Lookup;
use iglint_project::kinds::ResourceKind;
use iglint_project::view::{Binding, BindingConfig};

use super::case_insensitive_match;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};

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
inherited ones. Literal paths passed to `system.db.runNamedQuery` in
scripts are checked too, as soon as the script layer reports them.

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
    hooks: &[Hook::Binding(&UnknownNamedQuery)],
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
        let mut message = format!("Named query `{value}` does not exist");
        if let Some(similar) =
            case_insensitive_match(cx.workspace(), project, ResourceKind::NamedQuery, value)
        {
            message.push_str(&format!(" (did you mean `{similar}`?)"));
        }
        let file = cx.primary_file();
        cx.report_in(file, path.range(), message)
            .node_path(&binding.node_path);
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
}
