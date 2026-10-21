//! TAG006 tag-change-script-paths.

use iglint_project::event_resource::EventSettings;
use iglint_project::model::ModelData;
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG006",
    name: "tag-change-script-paths",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Tag change script without tag paths or with an invalid tag path",
    explanation: r#"## What it does

Checks the `paths` of gateway (and Vision client) tag change scripts: the
list must exist and not be empty, and every entry must be a tag path the
gateway can parse.

## Why is this bad?

A tag change script without paths never runs; an unparsable path is
dropped, so the script does not run for that tag.

## Example

```json
{"attributes": {"paths": ["Line1/[default]Speed"], "changeTypes": ["value"]}}
```

Use instead:

```json
{"attributes": {"paths": ["[default]Line1/Speed"], "changeTypes": ["value"]}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&TagChangePaths)],
    options: None,
};

struct TagChangePaths;

impl ResourceRule for TagChangePaths {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::EventScript(e) = data else {
            return;
        };
        let EventSettings::TagChange {
            paths, paths_value, ..
        } = &e.settings
        else {
            return;
        };
        let Some(resource) = cx.resource() else {
            return;
        };
        let manifest = resource.manifest.file;
        let node = resource.path.to_string();
        let empty = paths_value.is_none_or(|v| v.as_array().is_none_or(|a| a.is_empty()));
        if empty {
            let range: TextRange = match paths_value {
                Some(v) => v.range,
                None => resource
                    .manifest
                    .doc
                    .root()
                    .and_then(|r| r.get("attributes"))
                    .map_or_else(TextRange::default, |a| a.range),
            };
            cx.report_in(
                manifest,
                range,
                "Tag change script has no tag paths; it never runs",
            )
            .node_path(node.clone());
            return;
        }
        for (s, parsed) in paths {
            if let Some(error) = &parsed.error {
                cx.report_in(
                    manifest,
                    s.range(),
                    format!("Invalid tag path `{}`: {}", s.value(), error.kind),
                )
                .node_path(node.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn tag_change(attributes: &str) -> Fixture {
        Fixture::new()
            .file(
                "projects/p/ignition/tag-change/watch/resource.json",
                &format!(
                    r#"{{"scope": "G", "version": 1, "restricted": false, "overridable": true,
                        "files": ["onTagChange.py"], "attributes": {attributes}}}"#
                ),
            )
            .file(
                "projects/p/ignition/tag-change/watch/onTagChange.py",
                "def onTagChange(initialChange, newValue, previousValue, event, executionCount):\n\tpass\n",
            )
    }

    #[test]
    fn empty_missing_and_invalid_paths() {
        let d = tag_change(r#"{"enabled": true, "paths": []}"#).lint(&["TAG006"]);
        assert_eq!(flagged(&d), vec!["[]"]);
        let d = tag_change(r#"{"enabled": true}"#).lint(&["TAG006"]);
        assert_eq!(d.len(), 1);
        let d = tag_change(r#"{"paths": ["[default]A/B", "A/[default]B"]}"#).lint(&["TAG006"]);
        assert_eq!(flagged(&d), vec!["\"A/[default]B\""]);
        assert!(d[0].message.starts_with("Invalid tag path `A/[default]B`"));
    }
}
