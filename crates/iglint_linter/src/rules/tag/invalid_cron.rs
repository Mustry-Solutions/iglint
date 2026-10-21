//! TAG007 invalid-cron-expression.

use iglint_project::event_resource::EventSettings;
use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG007",
    name: "invalid-cron-expression",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Scheduled script with an invalid cron expression",
    explanation: r#"## What it does

Checks the `cronExpression` of gateway scheduled scripts. The gateway uses
five-field cron patterns (minute, hour, day of month, month, day of week),
optionally several joined with `|`. Each field is `*` or a comma-separated
list of values and ranges (`a-b`), each optionally with a step (`/n`).
Months may be written `jan`..`dec`, days of the week `sun`..`sat` (0 or 7
is Sunday), and the day of month may be `L` (last day).

Only clear errors are reported: a wrong number of fields, characters that
are not part of the syntax, values out of range and zero steps.

## Why is this bad?

The gateway cannot schedule a script with an invalid pattern, so it never
runs.

## Example

```json
{"attributes": {"cronExpression": "0 2 * *"}}
```

Use instead:

```json
{"attributes": {"cronExpression": "0 2 * * *"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&InvalidCron)],
    options: None,
};

struct InvalidCron;

impl ResourceRule for InvalidCron {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::EventScript(e) = data else {
            return;
        };
        let EventSettings::Scheduled { cron_expression } = &e.settings else {
            return;
        };
        let Some(resource) = cx.resource() else {
            return;
        };
        let manifest = resource.manifest.file;
        let node = resource.path.to_string();
        match cron_expression {
            None => {
                let range = resource
                    .manifest
                    .doc
                    .root()
                    .and_then(|r| r.get("attributes"))
                    .map(|a| a.range)
                    .unwrap_or_default();
                cx.report_in(manifest, range, "Scheduled script has no `cronExpression`")
                    .node_path(node);
            }
            Some(expr) => {
                if let Err(problem) = validate(expr.value()) {
                    cx.report_in(
                        manifest,
                        expr.range(),
                        format!("Invalid cron expression `{}`: {problem}", expr.value()),
                    )
                    .node_path(node);
                }
            }
        }
    }
}

const FIELDS: [(&str, u32, u32); 5] = [
    ("minute", 0, 59),
    ("hour", 0, 23),
    ("day of month", 1, 31),
    ("month", 1, 12),
    ("day of week", 0, 7),
];

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// Validates a cron pattern (see the rule documentation).
pub(crate) fn validate(pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err("the pattern is empty".to_owned());
    }
    for part in pattern.split('|') {
        let fields: Vec<&str> = part.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!("expected 5 fields, found {}", fields.len()));
        }
        for (field, (name, min, max)) in fields.iter().zip(FIELDS) {
            for element in field.split(',') {
                validate_element(element, name, min, max)?;
            }
        }
    }
    Ok(())
}

fn validate_element(element: &str, name: &str, min: u32, max: u32) -> Result<(), String> {
    let (range, step) = match element.split_once('/') {
        Some((r, s)) => (r, Some(s)),
        None => (element, None),
    };
    if let Some(step) = step {
        match step.parse::<u32>() {
            Ok(0) => return Err(format!("step 0 in the {name} field")),
            Ok(_) => {}
            Err(_) => return Err(format!("invalid step `{step}` in the {name} field")),
        }
    }
    if range == "*" {
        return Ok(());
    }
    if name == "day of month" && range.eq_ignore_ascii_case("L") && step.is_none() {
        return Ok(());
    }
    let bounds: Vec<&str> = range.split('-').collect();
    if bounds.len() > 2 || bounds.iter().any(|b| b.is_empty()) {
        return Err(format!("invalid value `{element}` in the {name} field"));
    }
    for b in bounds {
        value(b, name, min, max)?;
    }
    Ok(())
}

fn value(text: &str, name: &str, min: u32, max: u32) -> Result<u32, String> {
    let lower = text.to_ascii_lowercase();
    let named = match name {
        "month" => MONTHS
            .iter()
            .position(|m| *m == lower)
            .map(|i| i as u32 + 1),
        "day of week" => DAYS.iter().position(|d| *d == lower).map(|i| i as u32),
        _ => None,
    };
    let n = match named {
        Some(n) => n,
        None => text
            .parse::<u32>()
            .map_err(|_| format!("invalid value `{text}` in the {name} field"))?,
    };
    if n < min || n > max {
        return Err(format!(
            "{n} is out of range for the {name} field ({min}-{max})"
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::rules::test_support::*;

    #[test]
    fn patterns() {
        for ok in [
            "0 2 * * *",
            "*/5 * * * *",
            "0 8-17/2 * * mon-fri",
            "0 0 L * *",
            "0 0 1 jan,jul *",
            "0 0 * * 7",
            "0 2 * * *|30 14 * * sat",
        ] {
            assert_eq!(validate(ok), Ok(()), "{ok}");
        }
        for (bad, why) in [
            ("0 2 * *", "expected 5 fields, found 4"),
            (
                "60 * * * *",
                "60 is out of range for the minute field (0-59)",
            ),
            ("*/0 * * * *", "step 0 in the minute field"),
            ("0 0 * foo *", "invalid value `foo` in the month field"),
            ("", "the pattern is empty"),
            (
                "0 0 0 * *",
                "0 is out of range for the day of month field (1-31)",
            ),
        ] {
            assert_eq!(validate(bad), Err(why.to_owned()), "{bad}");
        }
    }

    fn scheduled(attributes: &str) -> Fixture {
        Fixture::new()
            .file(
                "projects/p/ignition/scheduled/nightly/resource.json",
                &format!(
                    r#"{{"scope": "G", "version": 1, "restricted": false, "overridable": true,
                        "files": ["handleScheduleEvent.py"], "attributes": {attributes}}}"#
                ),
            )
            .file(
                "projects/p/ignition/scheduled/nightly/handleScheduleEvent.py",
                "def handleScheduleEvent():\n\tpass\n",
            )
    }

    #[test]
    fn reports_on_the_manifest() {
        let d = scheduled(r#"{"cronExpression": "0 25 * * *", "enabled": true}"#).lint(&["TAG007"]);
        assert_eq!(flagged(&d), vec!["\"0 25 * * *\""]);
        assert!(
            scheduled(r#"{"cronExpression": "0 2 * * *"}"#)
                .lint(&["TAG007"])
                .is_empty()
        );
        assert_eq!(scheduled(r#"{"enabled": true}"#).lint(&["TAG007"]).len(), 1);
    }
}
