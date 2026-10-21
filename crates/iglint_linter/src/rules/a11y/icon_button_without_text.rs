//! A11Y002 icon-button-without-text.

use iglint_project::Owner;
use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};
use crate::rules::helpers::is_blank_text;

pub static META: RuleMeta = RuleMeta {
    code: "A11Y002",
    name: "icon-button-without-text",
    category: Category::A11y,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Clickable button or icon without text or tooltip",
    explanation: r#"## What it does

Reports a Button (`ia.input.button`) or Icon (`ia.display.icon`) that has
an enabled event action but neither a visible text (`props.text`, buttons
only) nor a tooltip text (`meta.tooltip.text`). Bound texts count as
present.

## Why is this bad?

A control that shows only an icon has no accessible name: screen readers
announce "button", and operators unfamiliar with the icon have nothing to
go by. A tooltip text or a short label fixes both.

## Example

```json
{"type": "ia.input.button", "meta": {"name": "Reset"},
 "props": {"text": "", "image": {"icon": {"path": "material/refresh"}}},
 "events": {"component": {"onActionPerformed": {"type": "script", "config": {"script": "\treset()"}}}}}
```

Use instead:

```json
{"type": "ia.input.button", "meta": {"name": "Reset", "tooltip": {"enabled": true, "text": "Reset counters"}},
 "props": {"text": "", "image": {"icon": {"path": "material/refresh"}}},
 "events": {"component": {"onActionPerformed": {"type": "script", "config": {"script": "\treset()"}}}}}
```

Not in the default set: many HMIs use well-known icon-only controls on
purpose.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&IconButtonWithoutText)],
    options: None,
};

struct IconButtonWithoutText;

impl ComponentRule for IconButtonWithoutText {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        let is_button = match component.type_id {
            Some("ia.input.button") => true,
            Some("ia.display.icon") => false,
            _ => return,
        };
        let owner = Owner::Component(component.id);
        let clickable = view
            .events
            .iter()
            .any(|e| e.owner == owner && e.actions.iter().any(|a| a.enabled != Some(false)));
        if !clickable {
            return;
        }
        if is_button
            && (!is_blank_text(component.props.and_then(|p| p.get("text")))
                || view.is_bound(owner, "props.text"))
        {
            return;
        }
        let tooltip = component
            .meta
            .and_then(|m| m.get("tooltip"))
            .and_then(|t| t.get("text"));
        if !is_blank_text(tooltip) || view.is_bound(owner, "meta.tooltip.text") {
            return;
        }
        let what = if is_button {
            "Button"
        } else {
            "Clickable icon"
        };
        cx.report(
            component.name_range(),
            format!(
                "{what} `{}` has no text or tooltip; give it `meta.tooltip.text`",
                component.name
            ),
        )
        .node_path(component.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const CLICK: &str =
        r#""events": {"dom": {"onClick": {"type": "script", "config": {"script": "\tpass"}}}}"#;

    #[test]
    fn reports_unlabelled_controls() {
        let f = Fixture::view(&format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
              {{"type": "ia.input.button", "meta": {{"name": "B1"}}, "props": {{"text": ""}}, {CLICK}}},
              {{"type": "ia.input.button", "meta": {{"name": "B2"}}, "props": {{"text": "Go"}}, {CLICK}}},
              {{"type": "ia.display.icon", "meta": {{"name": "I1"}}, {CLICK}}},
              {{"type": "ia.display.icon", "meta": {{"name": "I2", "tooltip": {{"text": "Help"}}}}, {CLICK}}},
              {{"type": "ia.display.icon", "meta": {{"name": "I3"}}}}]}}}}"#
        ));
        let d = f.lint(&["A11Y002"]);
        assert_eq!(flagged(&d), vec!["\"B1\"", "\"I1\""]);
    }
}
