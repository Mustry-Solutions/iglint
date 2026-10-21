//! TAG005 invalid-alarm-bind-type.

use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG005",
    name: "invalid-alarm-bind-type",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Alarm property bound with a bind type alarms do not accept",
    explanation: r#"## What it does

Checks the `bindType` of bound alarm properties. Alarms accept exactly
`Tag`, `Expression` and `UDTParameter` (with the bound value in `value`);
tag properties use `parameter` with `binding`, which alarms reject.

## Why is this bad?

The gateway cannot read the alarm configuration: the property keeps its
default (a setpoint of 0, an empty label) or the alarm fails to load.

## Example

```json
{"name": "High", "setpointA": {"bindType": "parameter", "binding": "{MaxSpeed}"}}
```

Use instead:

```json
{"name": "High", "setpointA": {"bindType": "UDTParameter", "value": "MaxSpeed"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&AlarmBindType)],
    options: None,
};

struct AlarmBindType;

const ACCEPTED: &[&str] = &["Tag", "Expression", "UDTParameter"];

impl ResourceRule for AlarmBindType {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Tags(folder) = data else {
            return;
        };
        for tag in &folder.tags {
            for alarm in &tag.alarms {
                for b in &alarm.bindings {
                    let (range, found) = match b.bind_type {
                        Some(t) if ACCEPTED.contains(&t.value()) => continue,
                        Some(t) => (t.range(), format!("`{}`", t.value())),
                        None => (b.property.range(), "a non-string `bindType`".to_owned()),
                    };
                    cx.report_in(
                        tag.file,
                        range,
                        format!(
                            "Alarm property `{}` is bound with {found}; alarms accept `Tag`, `Expression` or `UDTParameter`",
                            b.property.value()
                        ),
                    )
                    .node_path(format!("{}.{}", alarm.node_path, b.property.value()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::tag::fixtures::*;
    use crate::rules::test_support::*;

    #[test]
    fn rejected_and_accepted_bind_types() {
        let f = tags(
            gateway(),
            r#"[{"name": "S", "tagType": "AtomicTag", "alarms": [{"name": "High", "mode": "AboveValue",
                "setpointA": {"bindType": "parameter", "binding": "{Max}"},
                "label": {"bindType": "Expression", "value": "'x'"},
                "setpointB": {"bindType": "Tag", "value": "[.]B"},
                "displayPath": {"bindType": "UDTParameter", "value": "Name"},
                "notes": {"bindType": "tag", "value": "[.]N"}}]}]"#,
            "[]",
        );
        let d = f.lint(&["TAG005"]);
        assert_eq!(flagged(&d), vec!["\"parameter\"", "\"tag\""]);
        assert_eq!(
            d[0].node_path.as_deref(),
            Some("[default]S@alarms.High.setpointA")
        );
    }
}
