//! VIEW003 unknown-component-type.

use std::any::Any;

use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{ComponentRule, Hook, ParsedOptions, Phase, RuleImpl};
use crate::rules::helpers::wildcard_match;

pub static META: RuleMeta = RuleMeta {
    code: "VIEW003",
    name: "unknown-component-type",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Component type is not a registered Perspective component",
    explanation: r#"## What it does

Reports a component whose `type` is not one of the Perspective components
of Ignition 8.3.

Types outside Ignition's own `ia.` namespace come from third-party or
in-house modules, which iglint cannot know. They are only checked once you
list the module types your gateways have in `allowed-types`; then any
other non-`ia.` type is reported too.

## Why is this bad?

A view with an unregistered type renders a "component not found"
placeholder: usually a typo after a hand edit, or a module that is not
installed on the gateway the project is deployed to.

## Example

```json
{"type": "ia.display.lable", "meta": {"name": "Title"}}
```

Use instead:

```json
{"type": "ia.display.label", "meta": {"name": "Title"}}
```

## Options

* `allowed-types` (default `[]`): component types from other modules that
  exist on your gateways. `*` matches any characters, `?` one character, for
  example `["embr.*", "acme.widgets.gauge"]`. Setting it (even to `[]`)
  turns on checking of non-`ia.` types.
"#,
    options: &[OptionMeta {
        name: "allowed-types",
        ty: OptionType::StringList,
        default: "[]",
        description: "Component types (wildcards `*`, `?`) from modules outside Ignition's own.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&UnknownComponentType)],
    options: Some(parse_options),
};

struct Options {
    allowed: Vec<String>,
    /// Whether `allowed-types` was configured: only then are module types
    /// checked.
    check_modules: bool,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        allowed: r.string_list("allowed-types")?,
        check_modules: r.is_set("allowed-types"),
    }) as Box<dyn Any + Send + Sync>)
}

struct UnknownComponentType;

impl ComponentRule for UnknownComponentType {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        _view: &View<'_>,
        component: &Component<'_>,
    ) {
        let (Some(type_id), Some(value)) = (component.type_id, component.type_value) else {
            return;
        };
        if iglint_stubs::component_type(type_id).is_some() {
            return;
        }
        let options = cx.options::<Options>();
        if !type_id.starts_with("ia.") && !options.check_modules {
            return;
        }
        if options.allowed.iter().any(|p| wildcard_match(p, type_id)) {
            return;
        }
        let message = format!("Unknown component type `{type_id}`");
        cx.report(value.range, message)
            .node_path(component.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const VIEW: &str = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
        {"type": "ia.display.lable", "meta": {"name": "A"}},
        {"type": "embr.chart.thing", "meta": {"name": "B"}},
        {"type": "ia.display.label", "meta": {"name": "C"}}]}}"#;

    #[test]
    fn reports_unknown_ignition_types_only_by_default() {
        let d = Fixture::view(VIEW).lint(&["VIEW003"]);
        assert_eq!(flagged(&d), vec!["\"ia.display.lable\""]);
        assert_eq!(d[0].message, "Unknown component type `ia.display.lable`");
    }

    #[test]
    fn allowed_types_option_enables_module_checks() {
        let d = Fixture::view(VIEW).lint_with(&[("VIEW003", r#"allowed-types = ["embr.*"]"#)]);
        assert_eq!(flagged(&d), vec!["\"ia.display.lable\""]);
        let d = Fixture::view(VIEW).lint_with(&[("VIEW003", r#"allowed-types = ["acme.*"]"#)]);
        assert_eq!(
            flagged(&d),
            vec!["\"ia.display.lable\"", "\"embr.chart.thing\""]
        );
    }
}
