//! VIEW014 param-without-direction.

use iglint_project::view::View;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW014",
    name: "param-without-direction",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "View parameter has no `paramDirection`",
    explanation: r#"## What it does

Reports a view parameter (`params.<name>`) without a valid
`propConfig["params.<name>"].paramDirection` (`input`, `output` or
`inout`). A parameter whose nested entries (`params.<name>.<key>`) carry a
direction counts as having one.

## Why is this bad?

The direction decides how the parameter is bound when the view is embedded
or opened as a popup: `input` flows into the view, `output` flows back to
the parent's binding, `inout` both. The Designer always writes it; a
parameter without one was added by hand or by a tool, and its behaviour in
embedding views is not what the parameter list in the Designer shows.

## Example

```json
"params": {"motorId": 0}
```

Use instead:

```json
"params": {"motorId": 0},
"propConfig": {"params.motorId": {"paramDirection": "input", "persistent": true}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&ParamWithoutDirection)],
    options: None,
};

struct ParamWithoutDirection;

impl ViewRule for ParamWithoutDirection {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        for param in view.params() {
            if param.direction.is_some() {
                continue;
            }
            // Object parameters may carry their directions on nested
            // entries (`params.door.open`); the parameter then has one.
            let nested = view.prop_configs.iter().any(|c| {
                c.owner == param.owner
                    && c.param_direction.is_some()
                    && c.path.category == "params"
                    && c.path.name == param.name
                    && c.key.value().len() > "params.".len() + param.name.len()
            });
            if nested {
                continue;
            }
            let config = param.config.map(|id| view.prop_config_entry(id));
            let (range, message) = match config.and_then(|c| c.param_direction_value) {
                Some(v) => (
                    v.range,
                    format!(
                        "View parameter `{}` has an invalid `paramDirection`; use `input`, `output` or `inout`",
                        param.name
                    ),
                ),
                None => (
                    param.range(),
                    format!("View parameter `{}` has no `paramDirection`", param.name),
                ),
            };
            let range = if range.is_empty() {
                config.map_or(range, |c| c.key.range())
            } else {
                range
            };
            cx.report(range, message).node_path(param.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_missing_and_invalid_directions() {
        let f = Fixture::view(
            r#"{"params": {"a": 1, "b": 2, "c": 3},
               "propConfig": {"params.b": {"paramDirection": "in"}, "params.c": {"paramDirection": "output"},
                              "params.d": {"persistent": true}},
               "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#,
        );
        let d = f.lint(&["VIEW014"]);
        assert_eq!(flagged(&d), vec!["\"a\"", "\"in\"", "\"params.d\""]);
        assert_eq!(d[0].message, "View parameter `a` has no `paramDirection`");
    }

    #[test]
    fn nested_directions_count() {
        let f = Fixture::view(
            r#"{"params": {"door": {"open": true}},
               "propConfig": {"params.door.open": {"paramDirection": "input"}},
               "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#,
        );
        assert!(f.lint(&["VIEW014"]).is_empty());
    }
}
