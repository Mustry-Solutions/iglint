//! REF011 unused-view.

use iglint_project::model::ModelData;
use iglint_project::summary::covered_by_prefix;
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF011",
    name: "unused-view",
    category: Category::Ref,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "View is not referenced anywhere",
    explanation: r#"## What it does

Reports views that no literal reference in the project (or in a project
inheriting from it) points at: no embedded view, repeater, table, popup,
navigate or dock action, page or dock in the page configuration, and no
script call (`system.perspective.openPopup`, `togglePopup`,
`navigate(view=...)`) with a literal path.

## Why is this bad?

Unused views are dead weight: they are still loaded, searched and
maintained, and they confuse readers about what the application shows.

Paths built at run time are taken into account by their literal prefix:
a bound `props.path` whose expression is `'Cards/' + {view.params.kind}`,
or a script call with `'Popups/' + name`, makes every view under that
prefix count as used (a path with no literal prefix makes every view
count). Views of inheritable projects are not reported, because projects
outside the workspace may use them.

This rule is not in the default set: paths can also come from tags,
databases or session properties, which no static analysis sees.

## Example

A view `Old/MotorPanel` that nothing embeds, opens or navigates to. Delete
it, or reference it where it is meant to be used.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnusedView)],
    options: None,
};

struct UnusedView;

impl ResourceRule for UnusedView {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::View { file, .. } = data else {
            return;
        };
        if let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource())
            && covered_by_prefix(&index.dynamic_references(project).views, &resource.path)
        {
            return;
        }
        report_unused(cx, *file, "View");
    }
}

/// Reports the unit's resource when nothing references it.
pub(crate) fn report_unused(cx: &mut Checker<'_, '_>, file: iglint_project::FileId, what: &str) {
    let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
        return;
    };
    let ws = cx.workspace();
    if ws.project(project).inheritable() || index.usage_count(resource.id) > 0 {
        return;
    }
    // The first character, which may be longer than a byte (a byte
    // order mark).
    let range = cx
        .file(file)
        .map_or(TextRange::default(), |f| TextRange::first_char(f.text()));
    cx.report_in(
        file,
        range,
        format!("{what} `{}` is not referenced anywhere", resource.path),
    )
    .node_path(resource.path.to_string());
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn unreferenced_views() {
        let main = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.view", "meta": {"name": "E"}, "props": {"path": "Used"}}]}}"#;
        let f = Fixture::view(main)
            .with_view("Used", "{}")
            .with_view("Unused", "{}")
            .resource(
                "projects/p/com.inductiveautomation.perspective/page-config",
                &[("config.json", r#"{"pages": {"/": {"viewPath": "Main"}}}"#)],
            );
        let d = f.lint(&["REF011"]);
        assert_eq!(
            messages(&d)[0].1,
            "View `Unused` is not referenced anywhere"
        );
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn inheritable_projects_are_skipped() {
        let f = Fixture::new()
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "enabled": true, "inheritable": true}"#,
            )
            .with_view("Lonely", "{}");
        assert!(f.lint(&["REF011"]).is_empty());
    }

    #[test]
    fn script_references_and_dynamic_prefixes() {
        use crate::rules::script::testing::{ViewScript, view_json};
        let main = view_json(
            ViewScript::Action,
            "\tsystem.perspective.openPopup('p', 'Popups/A')\n\tsystem.perspective.openPopup('p', 'Cards/' + kind)",
        );
        let bound = r#"{"root": {"type": "ia.display.view", "meta": {"name": "root"}, "props": {"path": "x"},
            "propConfig": {"props.path": {"binding": {"type": "expr", "config": {"expression": "'Tiles/' + {view.params.k}"}}}}}}"#;
        let f = Fixture::new()
            .with_view("Main", &main)
            .with_view("Holder", bound)
            .with_view("Popups/A", "{}")
            .with_view("Popups/B", "{}")
            .with_view("Cards/X", "{}")
            .with_view("Tiles/Y", "{}")
            .resource(
                "projects/p/com.inductiveautomation.perspective/page-config",
                &[(
                    "config.json",
                    r#"{"pages": {"/": {"viewPath": "Main"}, "/h": {"viewPath": "Holder"}}}"#,
                )],
            );
        let d = f.lint(&["REF011"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec!["View `Popups/B` is not referenced anywhere".to_owned()]
        );
    }
}
