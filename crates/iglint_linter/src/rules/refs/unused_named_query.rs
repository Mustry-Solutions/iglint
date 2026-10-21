//! REF013 unused-named-query.

use iglint_project::model::ModelData;

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
project inheriting from it) uses, and (once the script layer reports them)
no `system.db.runNamedQuery` call with a literal path.

## Why is this bad?

Unused queries are dead code that still has to be kept in sync with the
database schema.

This rule is not in the default set: named queries are also run with
paths built at run time, from other projects through the gateway, and from
scripts the script layer cannot resolve. Queries of inheritable projects
are not reported.

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
}
