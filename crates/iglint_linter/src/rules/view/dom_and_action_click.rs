//! VIEW020 dom-and-action-click.

use iglint_project::Owner;
use iglint_project::view::{Component, EventHandler, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW020",
    name: "dom-and-action-click",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Button has both `dom.onClick` and `component.onActionPerformed` actions",
    explanation: r#"## What it does

Reports a Button (`ia.input.button`) that has enabled actions on both
`events.dom.onClick` and `events.component.onActionPerformed`.

## Why is this bad?

A mouse click on a button fires both events, so the work runs twice (two
writes, two popups, two navigations), in an order that is not guaranteed.
`onActionPerformed` also fires for keyboard activation, so it is the one to
keep.

## Example

```json
"events": {
  "dom": {"onClick": {"type": "script", "scope": "G", "config": {"script": "\tsave()"}}},
  "component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tsave()"}}}
}
```

Use instead:

```json
"events": {
  "component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tsave()"}}}
}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&DomAndActionClick)],
    options: None,
};

struct DomAndActionClick;

fn active<'v, 'a>(
    view: &'v View<'a>,
    owner: Owner,
    group: &str,
    name: &str,
) -> Option<&'v EventHandler<'a>> {
    view.events_of(owner).find(|e| {
        e.group == group && e.name == name && e.actions.iter().any(|a| a.enabled != Some(false))
    })
}

impl ComponentRule for DomAndActionClick {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        if component.type_id != Some("ia.input.button") {
            return;
        }
        let owner = Owner::Component(component.id);
        let (Some(dom), Some(action)) = (
            active(view, owner, "dom", "onClick"),
            active(view, owner, "component", "onActionPerformed"),
        ) else {
            return;
        };
        let file = cx.primary_file();
        cx.report(
            dom.key.range(),
            "Button has both `dom.onClick` and `component.onActionPerformed` actions; a click runs both",
        )
        .node_path(dom.node_path.clone())
        .label(file, action.key.range(), "also runs on click");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn button(kind: &str, events: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
                {{"type": "{kind}", "meta": {{"name": "B"}}, "events": {events}}}]}}}}"#
        )
    }

    const BOTH: &str = r#"{"dom": {"onClick": {"type": "script", "config": {"script": "\tpass"}}},
        "component": {"onActionPerformed": [{"type": "nav", "config": {"page": "/"}}]}}"#;

    #[test]
    fn reports_buttons_with_both() {
        let d = Fixture::view(&button("ia.input.button", BOTH)).lint(&["VIEW020"]);
        assert_eq!(flagged(&d), vec!["\"onClick\""]);
        assert_eq!(d[0].labels.len(), 1);
    }

    #[test]
    fn other_components_and_disabled_actions() {
        assert!(
            Fixture::view(&button("ia.display.label", BOTH))
                .lint(&["VIEW020"])
                .is_empty()
        );
        let disabled = r#"{"dom": {"onClick": {"type": "script", "enabled": false, "config": {"script": "\tpass"}}},
            "component": {"onActionPerformed": {"type": "script", "config": {"script": "\tpass"}}}}"#;
        assert!(
            Fixture::view(&button("ia.input.button", disabled))
                .lint(&["VIEW020"])
                .is_empty()
        );
        let one = r#"{"component": {"onActionPerformed": {"type": "script", "config": {"script": "\tpass"}}}}"#;
        assert!(
            Fixture::view(&button("ia.input.button", one))
                .lint(&["VIEW020"])
                .is_empty()
        );
    }
}
