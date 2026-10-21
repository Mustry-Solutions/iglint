//! VIEW008 empty-script.

use iglint_project::ScriptKind;
use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW008",
    name: "empty-script",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Perspective script with an empty body",
    explanation: r#"## What it does

Reports a Perspective script whose body is empty or only whitespace: script
actions, script transforms, property change scripts, message handlers,
custom methods and extension functions, in views and session properties.
Disabled change scripts, disabled actions and disabled extension functions
are not compiled and are not reported.

## Why is this bad?

Ignition compiles the body inside a generated `def` line. An empty body is
an `IndentationError` ("expected an indented block"): the action does
nothing, the transform makes its binding fail, the handler never runs, and
the gateway logs an error each time.

## Example

```json
"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": ""}}
```

Use instead: remove the action, or give it a body (`pass` if it is a
placeholder):

```json
"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tpass"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&EmptyScript)],
    options: None,
};

struct EmptyScript;

fn describe(kind: ScriptKind) -> Option<&'static str> {
    Some(match kind {
        ScriptKind::EventAction => "Script action",
        ScriptKind::Transform => "Script transform",
        ScriptKind::PropertyChange => "Property change script",
        ScriptKind::MessageHandler => "Message handler",
        ScriptKind::CustomMethod => "Custom method",
        ScriptKind::ExtensionFunction { .. } => "Extension function",
        _ => return None,
    })
}

impl ResourceRule for EmptyScript {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        if !matches!(
            data,
            ModelData::View { .. } | ModelData::SessionProps { .. }
        ) {
            return;
        }
        for (file, script) in data.scripts() {
            let Some(what) = describe(script.kind) else {
                continue;
            };
            if !script.enabled || !script.is_blank() {
                continue;
            }
            cx.report_in(
                file,
                script.anchor(),
                format!("{what} has an empty body; Ignition cannot compile it"),
            )
            .node_path(script.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_blank_scripts_of_every_kind() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
              "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "  \n\t"}}}},
              "scripts": {"messageHandlers": [{"messageType": "m", "pageScope": true, "sessionScope": false, "viewScope": false, "script": ""}],
                          "customMethods": [{"name": "f", "params": [], "script": "\treturn 1"}]},
              "propConfig": {"custom.a": {"onChange": {"enabled": true, "script": ""},
                 "binding": {"type": "expr", "config": {"expression": "1"}, "transforms": [{"type": "script", "code": ""}]}},
                 "custom.b": {"onChange": {"enabled": false, "script": ""}}},
              "custom": {"a": 1, "b": 2}}}"#,
        );
        let d = f.lint(&["VIEW008"]);
        let mut msgs: Vec<String> = d.iter().map(|d| d.message.clone()).collect();
        msgs.sort();
        assert_eq!(
            msgs,
            vec![
                "Message handler has an empty body; Ignition cannot compile it",
                "Property change script has an empty body; Ignition cannot compile it",
                "Script action has an empty body; Ignition cannot compile it",
                "Script transform has an empty body; Ignition cannot compile it",
            ]
        );
    }

    #[test]
    fn disabled_actions_are_not_compiled() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
              "events": {"dom": {"onClick": [{"type": "script", "enabled": false, "scope": "G", "config": {"script": ""}},
                                             {"type": "script", "scope": "G", "config": {"script": "\tpass"}}]}}}}"#,
        );
        assert!(f.lint(&["VIEW008"]).is_empty());
    }
}
