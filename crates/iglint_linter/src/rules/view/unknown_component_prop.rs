//! VIEW004 unknown-component-prop.

use iglint_project::Owner;
use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW004",
    name: "unknown-component-prop",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Top-level `props` key is not in the component's schema",
    explanation: r#"## What it does

Reports a top-level key of a component's `props` (or a `propConfig` entry
`props.<key>`) that the component's schema does not declare, for component
types whose schema forbids undeclared properties
(`additionalProperties: false`, for example `ia.container.flex`,
`ia.chart.pie`, `ia.display.tree`). Components whose schema accepts extra
keys are never reported.

## Why is this bad?

Perspective ignores properties the component does not know. A misspelled
property (`props.dirction`) or one left over from another component type
has no effect, and bindings on it run for nothing.

## Example

```json
{"type": "ia.container.flex", "props": {"dirction": "column"}}
```

Use instead:

```json
{"type": "ia.container.flex", "props": {"direction": "column"}}
```

Not in the default set: component schemas change between Ignition versions
and modules, so a property added in a newer version would be reported on
projects that target it.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&UnknownComponentProp)],
    options: None,
};

struct UnknownComponentProp;

impl ComponentRule for UnknownComponentProp {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        let Some(ct) = component.component_type() else {
            return;
        };
        if !ct.props_closed() {
            return;
        }
        let type_id = ct.id;
        let mut seen: Vec<&str> = Vec::new();
        if let Some(props) = component.props.and_then(|p| p.as_object()) {
            for member in &props.members {
                let key = member.key();
                if ct.prop(key).is_none() {
                    seen.push(key);
                    cx.report(
                        member.key.range(),
                        format!("`{key}` is not a property of `{type_id}`"),
                    )
                    .node_path(format!("{}.props.{key}", component.path));
                }
            }
        }
        let owner = Owner::Component(component.id);
        for config in view.prop_configs_of(owner) {
            if config.path.category != "props" || config.path.name.is_empty() {
                continue;
            }
            let name = config.path.name;
            if seen.contains(&name) || ct.prop(name).is_some() {
                continue;
            }
            seen.push(name);
            cx.report(
                config.key.range(),
                format!("`{name}` is not a property of `{type_id}`"),
            )
            .node_path(format!("{}.props.{name}", component.path));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn closed_schema_reports_unknown_keys() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
                "props": {"direction": "column", "dirction": "row", "style": {"x": 1}},
                "propConfig": {"props.bogus": {"binding": {"type": "expr", "config": {"expression": "1"}}}},
                "children": [{"type": "ia.display.label", "meta": {"name": "L"}, "props": {"whatever": 1}}]}}"#,
        );
        let d = f.lint(&["VIEW004"]);
        assert_eq!(flagged(&d), vec!["\"dirction\"", "\"props.bogus\""]);
    }

    #[test]
    fn known_keys_are_fine() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
                "props": {"direction": "column", "wrap": "nowrap"}}}"#,
        );
        assert!(f.lint(&["VIEW004"]).is_empty());
    }
}
