//! SEC005 querystring-parameter.

use iglint_project::model::ModelData;
use iglint_project::named_query::NamedQueryParamKind;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "SEC005",
    name: "querystring-parameter",
    category: Category::Sec,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Named query parameter of type `QueryString`",
    explanation: r#"## What it does

Reports named query parameters declared with type `QueryString` in the
query's `resource.json` (`attributes.parameters[].type`). The finding
points at the parameter's type.

## Why is this bad?

A `QueryString` parameter is pasted into the SQL text (`{name}`) before
the query runs; it is not bound like a `Parameter` (`:name`). Any caller
that can run the query (a binding, a script, a client with the right
permissions) controls part of the SQL statement: SQL injection, and
broken statements for values containing quotes.

Use a `Parameter` for values. For a table or column name, keep a fixed
set of queries, or check the value against a fixed list in a gateway
script before running the query, and suppress the finding for that query
with a per-file `exclude` for this rule.

## Example

```sql
SELECT * FROM orders WHERE line = '{line}'
```

with `line` declared as `QueryString`. Use instead:

```sql
SELECT * FROM orders WHERE line = :line
```

with `line` declared as `Parameter` (type `String`).
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&QueryStringParameter)],
    options: None,
};

struct QueryStringParameter;

impl ResourceRule for QueryStringParameter {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::NamedQuery(query) = data else {
            return;
        };
        let Some(resource) = cx.resource() else {
            return;
        };
        let file = resource.manifest.file;
        for param in &query.parameters {
            if param.kind != NamedQueryParamKind::QueryString {
                continue;
            }
            let range = param
                .value
                .get("type")
                .map_or(param.value.range, |t| t.range);
            let message = match param.identifier {
                Some(name) => format!(
                    "Named query parameter `{}` is a `QueryString`: its value is pasted into the SQL text",
                    name.value()
                ),
                None => "Named query parameter is a `QueryString`: its value is pasted into the SQL text"
                    .to_owned(),
            };
            cx.report_in(file, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn with_params(params: &str) -> Fixture {
        let manifest = format!(
            r#"{{"scope": "DG", "version": 2, "restricted": false, "overridable": true,
  "files": ["query.sql"], "attributes": {{"type": "Query", "database": "", "enabled": true,
  "parameters": [{params}]}}}}"#
        );
        Fixture::new()
            .file(
                "projects/p/ignition/named-query/Orders/ByLine/resource.json",
                &manifest,
            )
            .file(
                "projects/p/ignition/named-query/Orders/ByLine/query.sql",
                "SELECT * FROM orders WHERE line = '{line}' AND id = :id",
            )
    }

    #[test]
    fn reports_querystring_parameters() {
        let f = with_params(
            r#"{"type": "QueryString", "identifier": "line", "sqlType": 7},
               {"type": "Parameter", "identifier": "id", "sqlType": 2},
               {"type": "Database", "identifier": "db", "sqlType": 7}"#,
        );
        let d = f.lint(&["SEC005"]);
        assert_eq!(flagged(&d), ["\"QueryString\""]);
        assert!(d[0].message.contains("`line`"));
        assert!(d[0].file.path().ends_with("resource.json"));
    }

    #[test]
    fn bound_parameters_are_fine() {
        let f = with_params(r#"{"type": "Parameter", "identifier": "id", "sqlType": 2}"#);
        assert!(f.lint(&["SEC005"]).is_empty());
    }
}
