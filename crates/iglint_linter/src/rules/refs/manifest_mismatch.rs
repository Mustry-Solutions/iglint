//! REF009 resource-manifest-mismatch.

use iglint_json::Value;
use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF009",
    name: "resource-manifest-mismatch",
    category: Category::Ref,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`resource.json` and the files of the resource disagree",
    explanation: r#"## What it does

Compares the `files` list of a resource's `resource.json` with the files
in the resource directory: a listed file that does not exist, and a file
that exists but is not listed, are reported. Manifests without a `files`
list are not checked, and unlisted files are only reported for resource
types of Inductive Automation modules (third-party modules may keep
bookkeeping files next to their resources).

## Why is this bad?

The gateway loads only the listed files. An unlisted file (for example a
`view.json` copied in by hand) is silently ignored, and a listed file that
is missing makes the resource incomplete or invalid on the gateway.

## Example

```json
{"scope": "G", "version": 1, "files": ["view.json", "thumbnail.png"]}
```

when the directory has no `thumbnail.png`. Use instead:

```json
{"scope": "G", "version": 1, "files": ["view.json"]}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&ManifestMismatch)],
    options: None,
};

struct ManifestMismatch;

impl ResourceRule for ManifestMismatch {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, _data: &ModelData<'_>) {
        let Some(resource) = cx.resource() else {
            return;
        };
        let ws = cx.workspace();
        let manifest = resource.manifest.file;
        let Some(root) = resource.manifest.doc.root() else {
            return;
        };
        let files = root.get("files");
        let anchor = files.map_or(root.range, |f| f.range);
        for name in &resource.missing_files {
            let range = files
                .and_then(Value::as_array)
                .and_then(|a| a.iter().find(|v| v.as_str() == Some(name)))
                .map_or(anchor, |v| v.range);
            cx.report_in(
                manifest,
                range,
                format!("`{name}` is listed in `resource.json` but does not exist"),
            )
            .node_path(resource.qualified_path());
        }
        // Third-party modules may keep their own bookkeeping files next to
        // the listed ones; only resource types of Inductive Automation
        // modules are checked for unlisted files.
        let ia_module = &*resource.module_id == "ignition"
            || resource.module_id.starts_with("com.inductiveautomation.");
        for f in resource.files.iter().filter(|_| ia_module) {
            let entry = ws.file(*f);
            if entry.listed == Some(false) {
                cx.report_in(
                    manifest,
                    anchor,
                    format!(
                        "`{}` is not listed in `resource.json`; the gateway ignores it",
                        entry.name()
                    ),
                )
                .node_path(resource.qualified_path())
                .fingerprint_key(format!("unlisted {}", entry.name()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn missing_and_unlisted_files() {
        let f = Fixture::new()
            .file(
                "projects/p/com.inductiveautomation.perspective/views/Main/resource.json",
                r#"{"scope": "G", "version": 1, "files": ["view.json", "thumbnail.png"]}"#,
            )
            .file(
                "projects/p/com.inductiveautomation.perspective/views/Main/view.json",
                "{}",
            )
            .file(
                "projects/p/com.inductiveautomation.perspective/views/Main/extra.json",
                "{}",
            );
        let d = f.lint(&["REF009"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "`extra.json` is not listed in `resource.json`; the gateway ignores it".to_owned(),
                "`thumbnail.png` is listed in `resource.json` but does not exist".to_owned(),
            ]
        );
        assert_eq!(flagged(&d)[1], "\"thumbnail.png\"");
    }

    #[test]
    fn consistent_and_unlisted_manifests_are_fine() {
        assert!(Fixture::view("{}").lint(&["REF009"]).is_empty());
        let f = Fixture::new()
            .file(
                "projects/p/com.inductiveautomation.perspective/views/Main/resource.json",
                r#"{"scope": "G", "version": 1}"#,
            )
            .file(
                "projects/p/com.inductiveautomation.perspective/views/Main/view.json",
                "{}",
            );
        assert!(f.lint(&["REF009"]).is_empty());
    }
}
