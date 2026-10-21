//! VIEW005 invalid-position-key.

use iglint_project::Owner;
use iglint_project::view::{Component, View};
use iglint_stubs::Additional;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW005",
    name: "invalid-position-key",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`position` key not valid for the parent container",
    explanation: r#"## What it does

Reports a key of a component's `position` (or a `propConfig` entry
`position.<key>`) that the parent container does not use. Each container
type defines its children's position: `x`, `y`, `width`, `height`, `rotate`
in a coordinate container; `grow`, `shrink`, `basis`, `align`, `display` in
a flex container; `size` in a breakpoint container; `tabIndex` in a tab
container; `height`, `breakpoints` in a column container. Containers whose
child position is free-form (drawing, split, group) are not checked.

## Why is this bad?

The key has no effect. It is usually left over from moving the component
out of another container type by hand, and the layout the author expects
(`x`/`y` inside a flex container) does not happen.

## Example

A child of an `ia.container.flex`:

```json
"position": {"x": 10, "y": 20, "basis": "auto"}
```

Use instead:

```json
"position": {"basis": "auto"}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&InvalidPositionKey)],
    options: None,
};

struct InvalidPositionKey;

impl ComponentRule for InvalidPositionKey {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        let Some(parent) = component.parent else {
            return;
        };
        let Some(ct) = view.component(parent).component_type() else {
            return;
        };
        let Some(schema) = ct.child_position else {
            return;
        };
        if !matches!(schema.additional_properties, Additional::Forbidden) {
            return;
        }
        let container = ct.id;
        let mut seen: Vec<&str> = Vec::new();
        if let Some(position) = component.position.and_then(|p| p.as_object()) {
            for member in &position.members {
                let key = member.key();
                if ct.child_position_prop(key).is_none() {
                    seen.push(key);
                    cx.report(
                        member.key.range(),
                        format!("`position.{key}` has no effect in a `{container}` parent"),
                    )
                    .node_path(format!("{}.position.{key}", component.path));
                }
            }
        }
        let owner = Owner::Component(component.id);
        for config in view.prop_configs.iter().filter(|c| c.owner == owner) {
            let name = config.path.name;
            if config.path.category != "position" || name.is_empty() || seen.contains(&name) {
                continue;
            }
            if ct.child_position_prop(name).is_none() {
                seen.push(name);
                cx.report(
                    config.key.range(),
                    format!("`position.{name}` has no effect in a `{container}` parent"),
                )
                .node_path(format!("{}.position.{name}", component.path));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn flex_child_with_coordinates() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
                {"type": "ia.display.label", "meta": {"name": "A"}, "position": {"x": 1, "basis": "10px"}},
                {"type": "ia.display.label", "meta": {"name": "B"}, "position": {"grow": 1},
                 "propConfig": {"position.y": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}]}}"#,
        );
        let d = f.lint(&["VIEW005"]);
        assert_eq!(flagged(&d), vec!["\"x\"", "\"position.y\""]);
        assert_eq!(
            d[0].message,
            "`position.x` has no effect in a `ia.container.flex` parent"
        );
    }

    #[test]
    fn valid_and_unchecked_containers() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.coord", "meta": {"name": "root"}, "children": [
                {"type": "ia.display.label", "meta": {"name": "A"}, "position": {"x": 1, "y": 2, "width": 3, "height": 4, "rotate": {"angle": "0deg"}}},
                {"type": "ia.container.drawing", "meta": {"name": "D"}, "position": {"x": 1}, "children": [
                    {"type": "ia.shapes.rect", "meta": {"name": "R"}, "position": {"anything": 1}}]},
                {"type": "acme.unknown", "meta": {"name": "U"}, "position": {"x": 1}, "children": [
                    {"type": "ia.display.label", "meta": {"name": "L"}, "position": {"zz": 1}}]}]}}"#,
        );
        assert!(f.lint(&["VIEW005"]).is_empty());
    }

    #[test]
    fn root_is_not_checked() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "position": {"x": 1}}}"#,
        );
        assert!(f.lint(&["VIEW005"]).is_empty());
    }
}
