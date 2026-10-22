//! REF001 unknown-view.

use iglint_json::{JsonString, Value};
use iglint_project::kinds::ResourceKind;
use iglint_project::model::ModelData;
use iglint_project::view::{View, ViewRef, ViewRefKind};
use iglint_project::{FileId, Lookup};

use super::{case_insensitive_match, owner_value, value_at};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl, ScriptRule, ScriptUnit};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::string_values;

pub static META: RuleMeta = RuleMeta {
    code: "REF001",
    name: "unknown-view",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Referenced view does not exist",
    explanation: r#"## What it does

Checks every literal view path in the project against the views the
project sees (its own and inherited ones):

* `props.path` of embedded views and flex repeaters, view canvas
  instances, carousel views, accordion bodies and view headers, dashboard
  widgets, table subviews (when enabled) and table columns rendered as
  views;
* popup, navigate and alter-dock actions;
* page and dock view paths in the page configuration.

Scripts are checked too: literal view paths passed to
`system.perspective.openPopup`, `togglePopup` and `navigate(view=...)`,
found through aliases (`nav = system.perspective.navigate`) and local
names whose every assignment is a literal (`path = 'Popups/A'`). The
finding points at the literal.

## Why is this bad?

Perspective shows an empty frame or an error placeholder instead of the
view, and popups or navigation silently do nothing. A missing view is
almost always a view that was renamed or moved.

Paths that are bound (the literal is only the Designer's cached value),
table columns that do not render views, disabled table subviews and paths
whose project inherits from a project outside the workspace are not
reported.

## Example

```json
{"type": "ia.display.view", "props": {"path": "Popups/MotorDetial"}}
```

Use instead:

```json
{"type": "ia.display.view", "props": {"path": "Popups/MotorDetail"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnknownView), Hook::Script(&UnknownView)],
    options: None,
};

struct UnknownView;

impl ResourceRule for UnknownView {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        match data {
            ModelData::View { file, view } => {
                for r in &view.view_refs {
                    if r.bound || !is_active(view, r) {
                        continue;
                    }
                    check(cx, *file, r.path, &r.node_path);
                }
            }
            ModelData::PageConfig(pc) => {
                for page in &pc.pages {
                    if let Some(v) = page.view_path {
                        check(cx, pc.file, v, &format!("pages.{}", page.url.value()));
                    }
                    for d in &page.docks {
                        if let Some(v) = d.view_path {
                            check(
                                cx,
                                pc.file,
                                v,
                                &format!("pages.{}.docks.{}", page.url.value(), d.side),
                            );
                        }
                    }
                }
                for d in &pc.shared_docks {
                    if let Some(v) = d.view_path {
                        check(cx, pc.file, v, &format!("sharedDocks.{}", d.side));
                    }
                }
            }
            _ => {}
        }
    }
}

impl ScriptRule for UnknownView {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some((model, targets)) = super::script_calls::api_targets(cx, unit) else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        for (_, target) in targets {
            let ApiTarget::View { path, .. } = target else {
                continue;
            };
            for literal in string_values(model, path).unwrap_or_default() {
                let value = literal.value.as_str();
                if value.trim().is_empty() || value.contains('{') {
                    continue;
                }
                if !matches!(index.view(project, value), Lookup::Missing) {
                    continue;
                }
                let message = missing_message(cx, project, value);
                crate::rules::script::report(cx, unit, literal.range, message);
            }
        }
    }
}

fn missing_message(
    cx: &Checker<'_, '_>,
    project: iglint_project::ProjectId,
    value: &str,
) -> String {
    let mut message = format!("View `{value}` does not exist");
    if let Some(similar) =
        case_insensitive_match(cx.workspace(), project, ResourceKind::View, value)
    {
        message.push_str(&format!(
            " (view paths are case-sensitive; did you mean `{similar}`?)"
        ));
    }
    message
}

/// Whether the referencing property is actually used by Perspective.
pub(crate) fn is_active(view: &View<'_>, r: &ViewRef<'_>) -> bool {
    let Some(owner) = owner_value(view, r.owner) else {
        return true;
    };
    let sibling = |name: &str| {
        let base = r.property.strip_suffix(".viewPath")?;
        value_at(owner, &format!("{base}.{name}"))
    };
    match r.kind {
        // Columns use `viewPath` only with `render: "view"`.
        ViewRefKind::TableColumn => sibling("render").and_then(Value::as_str) == Some("view"),
        // Subviews are off unless `rows.subview.enable` is true (schema
        // default false).
        ViewRefKind::TableSubview => sibling("enable").and_then(Value::as_bool) == Some(true),
        // Accordion headers use `viewPath` only with `type: "view"`.
        ViewRefKind::Accordion if r.property.ends_with("header.content.viewPath") => {
            sibling("type").and_then(Value::as_str) == Some("view")
        }
        _ => true,
    }
}

fn check(cx: &mut Checker<'_, '_>, file: FileId, path: &JsonString<'_>, node_path: &str) {
    let Some((index, project)) = cx.project_index() else {
        return;
    };
    let value = path.value();
    if value.trim().is_empty() || value.contains('{') {
        return;
    }
    if !matches!(index.view(project, value), Lookup::Missing) {
        return;
    }
    let message = missing_message(cx, project, value);
    cx.report_in(file, path.range(), message)
        .node_path(node_path);
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn embed(path: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
                {{"type": "ia.display.view", "meta": {{"name": "Embed"}}, "props": {{"path": "{path}"}}}}
            ]}}}}"#
        )
    }

    #[test]
    fn reports_missing_embedded_view() {
        let f = Fixture::view(&embed("Popups/Missing")).with_view("Popups/Detail", "{}");
        let d = f.lint(&["REF001"]);
        assert_eq!(flagged(&d), vec!["\"Popups/Missing\""]);
        assert_eq!(d[0].node_path.as_deref(), Some("root/Embed.props.path"));
    }

    #[test]
    fn existing_views_and_leading_slash_are_fine() {
        let f = Fixture::view(&embed("/Popups/Detail")).with_view("Popups/Detail", "{}");
        assert!(f.lint(&["REF001"]).is_empty());
    }

    #[test]
    fn case_mismatch_gets_a_hint() {
        let f = Fixture::view(&embed("popups/detail")).with_view("Popups/Detail", "{}");
        let d = f.lint(&["REF001"]);
        assert!(
            d[0].message.contains("did you mean `Popups/Detail`"),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn bound_paths_are_skipped() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.view", "meta": {"name": "Embed"}, "props": {"path": "Gone"},
             "propConfig": {"props.path": {"binding": {"type": "expr", "config": {"expression": "'A'"}}}}}
        ]}}"#;
        assert!(Fixture::view(view).lint(&["REF001"]).is_empty());
    }

    #[test]
    fn inactive_table_columns_and_subviews_are_skipped() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.table", "meta": {"name": "T"}, "props": {
                "columns": [{"field": "a", "render": "auto", "viewPath": "Gone/A"},
                            {"field": "b", "render": "view", "viewPath": "Gone/B"}],
                "rows": {"subview": {"enable": false, "viewPath": "Gone/C"}}}}
        ]}}"#;
        let d = Fixture::view(view).lint(&["REF001"]);
        assert_eq!(flagged(&d), vec!["\"Gone/B\""]);
    }

    #[test]
    fn actions_and_page_config() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.input.button", "meta": {"name": "B"}, "events": {"component": {"onActionPerformed": [
                {"type": "popup", "scope": "C", "config": {"type": "open", "id": "p", "viewPath": "Gone/Popup"}},
                {"type": "popup", "scope": "C", "config": {"type": "close", "id": "p", "viewPath": "Gone/Closed"}},
                {"type": "nav", "scope": "C", "config": {"view": "Gone/Nav"}}
            ]}}}
        ]}}"#;
        let f = Fixture::view(view).resource(
            "projects/p/com.inductiveautomation.perspective/page-config",
            &[(
                "config.json",
                r#"{"pages": {"/": {"viewPath": "Main"}, "/x": {"viewPath": "Gone/Page",
                    "docks": {"left": [{"id": "d", "viewPath": "Gone/Dock"}]}}}}"#,
            )],
        );
        let mut found = flagged(&f.lint(&["REF001"]));
        found.sort();
        assert_eq!(
            found,
            vec![
                "\"Gone/Dock\"",
                "\"Gone/Nav\"",
                "\"Gone/Page\"",
                "\"Gone/Popup\""
            ]
        );
    }

    #[test]
    fn inherited_views_resolve_and_unknown_parents_stay_silent() {
        let base = Fixture::new()
            .file(
                "projects/base/project.json",
                r#"{"title": "base", "enabled": true, "inheritable": true}"#,
            )
            .resource(
                "projects/base/com.inductiveautomation.perspective/views/Shared",
                &[("view.json", "{}")],
            )
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "parent": "base", "enabled": true, "inheritable": false}"#,
            )
            .with_view("Main", &embed("Shared"));
        assert!(base.lint(&["REF001"]).is_empty());

        let orphan = Fixture::new()
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "parent": "elsewhere", "enabled": true, "inheritable": false}"#,
            )
            .with_view("Main", &embed("Anything"));
        assert!(orphan.lint(&["REF001"]).is_empty());
    }

    #[test]
    fn script_call_sites() {
        use crate::rules::script::testing::{
            ViewScript, decoded_flagged, gateway_message, library, view,
        };
        let body = "\tP = 'Popups/Gone'\n\tsystem.perspective.openPopup('id', P)\n\tnav = system.perspective.navigate\n\tnav(view='Popups/Detail')\n\tnav(view='popups/detail')\n\tsystem.perspective.openPopup('id', 'Dyn/' + x)\n\tsystem.perspective.togglePopup(id='t', view=\"Also\\\"Gone\")";
        let f = view(ViewScript::Action, body).with_view("Popups/Detail", "{}");
        let d = f.lint(&["REF001"]);
        assert_eq!(
            decoded_flagged(&d),
            vec!["'Popups/Gone'", "'popups/detail'", "\"Also\\\"Gone\""]
        );
        assert!(d[1].message.contains("did you mean `Popups/Detail`"));
        let lib = library(
            "core/nav",
            "def go(view='Lib/Gone'):\n    system.perspective.navigate(view=view)\n    system.perspective.navigate(view='Lib/Gone2')\n",
        );
        assert_eq!(flagged(&lib.lint(&["REF001"])), vec!["'Lib/Gone2'"]);
        let gw = gateway_message(
            "def handleMessage(payload):\n\tsystem.perspective.openPopup('i', 'X', sessionId=payload['s'], pageId=payload['p'])\n",
        );
        assert_eq!(flagged(&gw.lint(&["REF001"])), vec!["'X'"]);
    }

    #[test]
    fn standalone_views_have_no_project() {
        let f = Fixture::new().file("loose/view.json", &embed("Gone"));
        assert!(f.lint(&["REF001"]).is_empty());
    }
}
