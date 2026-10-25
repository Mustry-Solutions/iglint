//! REF013 unused-named-query.

use iglint_project::model::ModelData;
use iglint_project::summary::covered_by_prefix;

use super::unused_view::report_unused;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF013",
    name: "unused-named-query",
    category: Category::Ref,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Named query is not referenced anywhere",
    explanation: r#"## What it does

Reports named queries that no query binding in the project (or in a
project inheriting from it) uses, and no script call
(`system.db.runNamedQuery`, `execQuery`, ...) with a literal path.

## Why is this bad?

Unused queries are dead code that still has to be kept in sync with the
database schema.

Paths built at run time count by their literal prefix (`'Reports/' +
name` uses every query under `Reports/`). Queries of inheritable projects,
and of projects with resources whose scripts are not analysed (reports,
alarm pipelines, SFC charts, Vision windows), are not reported.

This rule is not in the default set: named queries are also run from
other projects through the gateway.

## Example

A named query `Reports/Legacy` that no binding or script uses. Delete it,
or use it where it is meant to be used.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnusedNamedQuery)],
    options: None,
};

struct UnusedNamedQuery;

impl ResourceRule for UnusedNamedQuery {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::NamedQuery(_) = data else {
            return;
        };
        // `query.sql` is not a linted file; report on the manifest.
        let Some(resource) = cx.resource() else {
            return;
        };
        if let Some((index, project)) = cx.project_index()
            && (covered_by_prefix(
                &index.dynamic_references(project).named_queries,
                &resource.path,
            ) || index.has_unmodelled_scripts(project))
        {
            return;
        }
        report_unused(cx, resource.manifest.file, "Named query");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::refs::unknown_named_query::tests::{query_view, with_query};
    use crate::rules::test_support::*;

    #[test]
    fn unreferenced_queries() {
        let f = with_query(Fixture::view(&query_view("Used", "{}")), "Used", &[]);
        let f = with_query(f, "Unused", &[]);
        let d = f.lint(&["REF013"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec!["Named query `Unused` is not referenced anywhere".to_owned()]
        );
    }

    #[test]
    fn script_and_dynamic_references() {
        use crate::rules::script::testing::{ViewScript, view};
        let f = view(
            ViewScript::Action,
            "\tsystem.db.runNamedQuery('Used/A', {})\n\tsystem.db.runNamedQuery('Dyn/' + name)",
        );
        let f = with_query(f, "Used/A", &[]);
        let f = with_query(f, "Dyn/B", &[]);
        let f = with_query(f, "Unused/C", &[]);
        let d = f.lint(&["REF013"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec!["Named query `Unused/C` is not referenced anywhere".to_owned()]
        );
    }
}
