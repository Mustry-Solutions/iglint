//! PERF003 hidden-component-bindings.

use iglint_project::Owner;
use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "PERF003",
    name: "hidden-component-bindings",
    category: Category::Perf,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Component with `meta.visible: false` has bindings",
    explanation: r#"## What it does

Reports a component whose `meta.visible` is the literal `false` (not bound)
while it, or a component inside it, has bindings.

## Why is this bad?

Hidden components keep running their bindings: tag subscriptions, polling
queries and expressions are evaluated for something nobody sees. A
component that is permanently hidden is usually left over and can be
removed; one that is shown conditionally should bind `meta.visible`.

## Example

```json
{"type": "ia.display.label", "meta": {"name": "Debug", "visible": false},
 "propConfig": {"props.text": {"binding": {"type": "tag", "config": {"tagPath": "[default]Line/Count"}}}}}
```

Use instead: delete the component, or bind `meta.visible` so it is only
shown (and only needed) when relevant.

Not in the default set: hiding a component temporarily during development
is common and harmless.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&HiddenComponentBindings)],
    options: None,
};

struct HiddenComponentBindings;

fn is_within(
    view: &View<'_>,
    mut id: iglint_project::ComponentId,
    ancestor: iglint_project::ComponentId,
) -> bool {
    loop {
        if id == ancestor {
            return true;
        }
        match view.component(id).parent {
            Some(p) => id = p,
            None => return false,
        }
    }
}

impl ComponentRule for HiddenComponentBindings {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        if component.visible() != Some(false) {
            return;
        }
        let owner = Owner::Component(component.id);
        if view.is_bound(owner, "meta.visible") {
            return;
        }
        let count = view
            .bindings
            .iter()
            .filter(|b| match b.owner {
                Owner::Component(c) => {
                    is_within(view, c, component.id)
                        && !(c == component.id && b.property == "meta.visible")
                }
                _ => false,
            })
            .count();
        if count == 0 {
            return;
        }
        let Some(visible) = component.meta.and_then(|m| m.get("visible")) else {
            return;
        };
        cx.report(
            visible.range,
            format!(
                "Component `{}` is hidden but runs {}",
                component.name,
                crate::rules::helpers::plural(count, "binding", "bindings")
            ),
        )
        .node_path(component.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn hidden_with_bindings() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
              {"type": "ia.container.flex", "meta": {"name": "Hidden", "visible": false}, "children": [
                {"type": "ia.display.label", "meta": {"name": "L"},
                 "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}]},
              {"type": "ia.display.label", "meta": {"name": "Shown", "visible": true},
               "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "1"}}}}},
              {"type": "ia.display.label", "meta": {"name": "Toggled", "visible": false},
               "propConfig": {"meta.visible": {"binding": {"type": "expr", "config": {"expression": "true"}}},
                              "props.text": {"binding": {"type": "expr", "config": {"expression": "1"}}}}},
              {"type": "ia.display.label", "meta": {"name": "Static", "visible": false}}]}}"#,
        );
        let d = f.lint(&["PERF003"]);
        assert_eq!(flagged(&d), vec!["false"]);
        assert_eq!(
            d[0].message,
            "Component `Hidden` is hidden but runs 1 binding"
        );
    }
}
