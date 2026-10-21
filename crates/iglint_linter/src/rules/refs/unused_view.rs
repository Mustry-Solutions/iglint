//! REF011 unused-view.

use iglint_project::model::ModelData;
use iglint_text::{TextRange, TextSize};

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
navigate or dock action, page or dock in the page configuration, and (once
the script layer reports them) no literal path in a script.

## Why is this bad?

Unused views are dead weight: they are still loaded, searched and
maintained, and they confuse readers about what the application shows.

This rule is not in the default set: views are also opened through paths
built at run time (bound `props.path`, string concatenation in scripts,
paths stored in tags or databases), which no static analysis sees. Views
of inheritable projects are not reported, because projects outside the
workspace may use them.

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
    let len = cx.file(file).map_or(0, |f| f.text().len().min(1));
    let range = TextRange::at(TextSize::new(0), TextSize::new(len as u32));
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
}
