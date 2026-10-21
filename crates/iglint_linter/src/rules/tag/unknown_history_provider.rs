//! TAG004 unknown-history-provider.

use iglint_project::Existence;
use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG004",
    name: "unknown-history-provider",
    category: Category::Tag,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`historyProvider` is not configured on the gateway",
    explanation: r#"## What it does

Checks the `historyProvider` of every tag with history enabled against the
historian providers and database connections (which serve as SQL
historians) configured in the gateway collections of the workspace. Names
are compared case-insensitively. When the workspace configures no
historian provider at all, nothing is reported.

## Why is this bad?

Tag history for a provider that does not exist is never stored: trends and
reports silently have no data for the tag.

Values with UDT parameter placeholders (`{Historian}`) are not checked.

## Example

```json
{"name": "Speed", "historyEnabled": true, "historyProvider": "Core Historain"}
```

Use instead:

```json
{"name": "Speed", "historyEnabled": true, "historyProvider": "Core Historian"}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnknownHistoryProvider)],
    options: None,
};

struct UnknownHistoryProvider;

impl ResourceRule for UnknownHistoryProvider {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Tags(folder) = data else {
            return;
        };
        let Some(index) = cx.index() else {
            return;
        };
        for tag in &folder.tags {
            if tag.history_enabled != Some(true) {
                continue;
            }
            let Some(provider) = tag.history_provider else {
                continue;
            };
            let name = provider.value();
            if name.trim().is_empty() || name.contains('{') {
                continue;
            }
            if index.history_provider(name) == Existence::Missing {
                cx.report_in(
                    tag.file,
                    provider.range(),
                    format!("History provider `{name}` is not configured on the gateway"),
                )
                .node_path(&tag.full_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::tag::fixtures::*;
    use crate::rules::test_support::*;

    fn historian(f: Fixture) -> Fixture {
        f.resource(
            "gw/core/com.inductiveautomation.historian/historian-provider/Core Historian",
            &[(
                "config.json",
                r#"{"profile": {"type": "CoreHistorian"}, "settings": {}}"#,
            )],
        )
    }

    const TAGS: &str = r#"[
        {"name": "A", "tagType": "AtomicTag", "historyEnabled": true, "historyProvider": "core historian"},
        {"name": "B", "tagType": "AtomicTag", "historyEnabled": true, "historyProvider": "Gone"},
        {"name": "C", "tagType": "AtomicTag", "historyEnabled": false, "historyProvider": "Gone"},
        {"name": "D", "tagType": "AtomicTag", "historyEnabled": true, "historyProvider": "{Hist}"}]"#;

    #[test]
    fn unknown_providers() {
        let d = tags(historian(gateway()), TAGS, "[]").lint(&["TAG004"]);
        assert_eq!(flagged(&d), vec!["\"Gone\""]);
    }

    #[test]
    fn no_historians_in_workspace_is_unknown() {
        assert!(tags(gateway(), TAGS, "[]").lint(&["TAG004"]).is_empty());
    }
}
