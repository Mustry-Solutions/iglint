//! STYLE002 unused-style-class.

use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};
use crate::rules::refs::unused_view::report_unused;

pub static META: RuleMeta = RuleMeta {
    code: "STYLE002",
    name: "unused-style-class",
    category: Category::Style,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Style class is never referenced",
    explanation: r#"## What it does

Reports style classes that no literal `classes` list and no `style-list`
map transform output in the project (or in a project inheriting from it)
names.

## Why is this bad?

Unused style classes are dead styling that still has to be maintained and
makes the class list harder to navigate.

This rule is not in the default set: class names are often built at run
time (expression bindings such as `'state-' + {view.params.state}`,
scripts), which no static analysis sees. Classes of inheritable projects
are not reported.

## Example

A style class `Legacy/Header` that no component uses. Delete it, or use
it where it is meant to be used.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnusedStyleClass)],
    options: None,
};

struct UnusedStyleClass;

impl ResourceRule for UnusedStyleClass {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::StyleClass(s) = data else {
            return;
        };
        report_unused(cx, s.file, "Style class");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn unreferenced_classes() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
            "props": {"style": {"classes": "Used"}}}}"#;
        let f = Fixture::view(view)
            .resource(
                "projects/p/com.inductiveautomation.perspective/style-classes/Used",
                &[("style.json", r#"{"base": {"style": {"color": "red"}}}"#)],
            )
            .resource(
                "projects/p/com.inductiveautomation.perspective/style-classes/Unused",
                &[("style.json", r#"{"base": {"style": {"color": "red"}}}"#)],
            );
        let d = f.lint(&["STYLE002"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec!["Style class `Unused` is not referenced anywhere".to_owned()]
        );
    }
}
