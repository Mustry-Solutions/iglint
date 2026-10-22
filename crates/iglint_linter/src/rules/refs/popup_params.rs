//! REF007 popup-param-mismatch.

use iglint_json::{JsonString, Value};
use iglint_project::model::ModelData;
use iglint_project::view::ViewRefKind;
use iglint_project::{FileId, Lookup};

use super::unknown_view::is_active;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl, ScriptRule, ScriptUnit};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::{dict_keys, string_value};

pub static META: RuleMeta = RuleMeta {
    code: "REF007",
    name: "popup-param-mismatch",
    category: Category::Ref,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Parameter passed to a view that the view does not declare",
    explanation: r#"## What it does

Checks the parameter names passed to a view against the parameters the
target view declares: `props.params` of embedded views, flex repeater
`instances`, view canvas, carousel, accordion, dashboard and table
`viewParams`, popup, navigate and alter-dock action `viewParams`, dock
`viewParams` in the page configuration, and the parameters dict literal of
script calls `system.perspective.openPopup`, `togglePopup` and
`navigate(view=..., params=...)` with a literal view path.

## Why is this bad?

Perspective ignores a parameter the view does not declare, so the value
never arrives. This is usually a typo or a parameter that was renamed in
the target view.

Bound parameter objects (the literal is only a cached value), references
to views that do not exist (see `REF001`) and the flex repeater's own
`instanceStyle` and `instancePosition` keys are not reported.

## Example

The view `Popups/Motor` declares the parameter `motorId`:

```json
{"type": "popup", "config": {"type": "open", "viewPath": "Popups/Motor", "viewParams": {"motor": 3}}}
```

Use instead:

```json
{"type": "popup", "config": {"type": "open", "viewPath": "Popups/Motor", "viewParams": {"motorId": 3}}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&PopupParams), Hook::Script(&PopupParams)],
    options: None,
};

struct PopupParams;

/// Keys a flex repeater instance may have besides view parameters.
const REPEATER_KEYS: &[&str] = &["instanceStyle", "instancePosition"];

impl ResourceRule for PopupParams {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        match data {
            ModelData::View { file, view } => {
                for r in &view.view_refs {
                    let Some(params) = r.params else {
                        continue;
                    };
                    if r.bound || r.params_bound || !is_active(view, r) {
                        continue;
                    }
                    let objects: Vec<&Value<'_>> = if r.kind == ViewRefKind::FlexRepeater {
                        params
                            .as_array()
                            .map(|a| a.iter().collect())
                            .unwrap_or_default()
                    } else {
                        vec![params]
                    };
                    let skip: &[&str] = if r.kind == ViewRefKind::FlexRepeater {
                        REPEATER_KEYS
                    } else {
                        &[]
                    };
                    for object in objects {
                        check(cx, *file, r.path, object, skip, &r.node_path);
                    }
                }
            }
            ModelData::PageConfig(pc) => {
                let docks = pc
                    .pages
                    .iter()
                    .flat_map(|p| p.docks.iter())
                    .chain(pc.shared_docks.iter());
                for d in docks {
                    if let (Some(path), Some(params)) = (d.view_path, d.view_params) {
                        let node = format!("docks.{}", d.id.map_or("", |i| i.value()));
                        check(cx, pc.file, path, params, &[], &node);
                    }
                }
            }
            _ => {}
        }
    }
}

impl ScriptRule for PopupParams {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some((model, targets)) = super::script_calls::api_targets(cx, unit) else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        for (_, target) in targets {
            let ApiTarget::View { path, params } = target else {
                continue;
            };
            let (Some(path), Some(params)) = (string_value(model, path), params.given()) else {
                continue;
            };
            let Some(keys) = dict_keys(model, params) else {
                continue;
            };
            let Lookup::Found {
                facts: Some(facts), ..
            } = index.view(project, &path.value)
            else {
                continue;
            };
            for k in keys {
                if !facts.params.iter().any(|p| p.name == k.value) {
                    let message =
                        format!("`{}` is not a parameter of view `{}`", k.value, path.value);
                    crate::rules::script::report(cx, unit, k.range, message);
                }
            }
        }
    }
}

fn check(
    cx: &mut Checker<'_, '_>,
    file: FileId,
    path: &JsonString<'_>,
    params: &Value<'_>,
    skip: &[&str],
    node_path: &str,
) {
    let Some(object) = params.as_object() else {
        return;
    };
    let Some((index, project)) = cx.project_index() else {
        return;
    };
    let Lookup::Found {
        facts: Some(facts), ..
    } = index.view(project, path.value())
    else {
        return;
    };
    for m in object.iter() {
        let name = m.key();
        if skip.contains(&name) || facts.params.iter().any(|p| p.name == name) {
            continue;
        }
        cx.report_in(
            file,
            m.key.range(),
            format!("`{name}` is not a parameter of view `{}`", path.value()),
        )
        .node_path(node_path);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const TARGET: &str = r#"{"params": {"motorId": 0, "mode": ""},
        "propConfig": {"params.motorId": {"paramDirection": "input"}, "params.mode": {"paramDirection": "input"}},
        "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#;

    #[test]
    fn popup_embedded_and_repeater_params() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.view", "meta": {"name": "E"}, "props": {"path": "Popups/Motor", "params": {"motorId": 1, "motor": 2}}},
            {"type": "ia.display.flex-repeater", "meta": {"name": "R"}, "props": {"path": "Popups/Motor",
                "instances": [{"motorId": 1, "instanceStyle": {}, "instancePosition": {}}, {"mod": "x"}]}},
            {"type": "ia.input.button", "meta": {"name": "B"}, "events": {"component": {"onActionPerformed":
                {"type": "popup", "scope": "C", "config": {"type": "open", "id": "p", "viewPath": "Popups/Motor", "viewParams": {"id": 1}}}}}}
        ]}}"#;
        let f = Fixture::view(view).with_view("Popups/Motor", TARGET);
        let d = f.lint(&["REF007"]);
        assert_eq!(flagged(&d), vec!["\"motor\"", "\"mod\"", "\"id\""]);
        assert_eq!(
            d[0].message,
            "`motor` is not a parameter of view `Popups/Motor`"
        );
    }

    #[test]
    fn declared_params_bound_params_and_missing_views_are_silent() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.view", "meta": {"name": "E"}, "props": {"path": "Popups/Motor", "params": {"motorId": 1}}},
            {"type": "ia.display.view", "meta": {"name": "G"}, "props": {"path": "Gone", "params": {"x": 1}}},
            {"type": "ia.display.view", "meta": {"name": "H"}, "props": {"path": "Popups/Motor", "params": {"x": 1}},
             "propConfig": {"props.params": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}
        ]}}"#;
        let f = Fixture::view(view).with_view("Popups/Motor", TARGET);
        assert!(f.lint(&["REF007"]).is_empty());
    }

    #[test]
    fn script_call_sites() {
        use crate::rules::script::testing::{ViewScript, decoded_flagged, view};
        let body = "\tsystem.perspective.openPopup('id', 'Popups/Motor', {'motorId': 1, 'motor': 2})\n\tsystem.perspective.navigate(view='Popups/Motor', params=dict(mod=1))\n\tsystem.perspective.openPopup('id', 'Popups/Motor', p)";
        let f = view(ViewScript::Action, body).with_view("Popups/Motor", TARGET);
        assert_eq!(
            decoded_flagged(&f.lint(&["REF007"])),
            vec!["'motor'", "mod"]
        );
    }
}
