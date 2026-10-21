//! VIEW009 disabled-change-script.

use iglint_project::model::ModelData;
use iglint_project::view::PropConfig;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW009",
    name: "disabled-change-script",
    category: Category::View,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "Property change script is disabled but has code",
    explanation: r#"## What it does

Reports a property change script (`propConfig.<path>.onChange`) with
`"enabled": false` whose script is not empty.

## Why is this bad?

A disabled change script never runs. Its code is dead, is not maintained
along with the rest of the view, and confuses readers who expect it to run
when the property changes. Delete it, or enable it if it should run.

## Example

```json
"custom.speed": {"onChange": {"enabled": false, "script": "\tself.refresh()"}}
```

Use instead:

```json
"custom.speed": {"onChange": {"enabled": true, "script": "\tself.refresh()"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&DisabledChangeScript)],
    options: None,
};

struct DisabledChangeScript;

impl ResourceRule for DisabledChangeScript {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let (file, configs, scripts): (_, &[PropConfig<'_>], _) = match data {
            ModelData::View { file, view } => (*file, &view.prop_configs, &view.scripts),
            ModelData::SessionProps { file, props } => (*file, &props.prop_configs, &props.scripts),
            _ => return,
        };
        for config in configs {
            let Some(on_change) = &config.on_change else {
                continue;
            };
            if on_change.enabled != Some(false) {
                continue;
            }
            let Some(script) = on_change.script.map(|id| &scripts[id.index()]) else {
                continue;
            };
            if script.is_blank() {
                continue;
            }
            let range = on_change
                .value
                .get("enabled")
                .map_or(script.anchor(), |v| v.range);
            cx.report_in(
                file,
                range,
                format!(
                    "Change script of `{}` is disabled but has code; it never runs",
                    config.key.value()
                ),
            )
            .node_path(script.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_disabled_scripts_with_code() {
        let f = Fixture::view(
            r#"{"custom": {"a": 1, "b": 2, "c": 3},
               "propConfig": {"custom.a": {"onChange": {"enabled": false, "script": "\tprint 1"}},
                              "custom.b": {"onChange": {"enabled": false, "script": "  "}},
                              "custom.c": {"onChange": {"enabled": true, "script": "\tprint 1"}}},
               "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#,
        );
        let d = f.lint(&["VIEW009"]);
        assert_eq!(flagged(&d), vec!["false"]);
        assert_eq!(
            d[0].message,
            "Change script of `custom.a` is disabled but has code; it never runs"
        );
    }
}
