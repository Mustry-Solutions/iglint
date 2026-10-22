//! REF009 resource-manifest-mismatch.

use iglint_json::Value;
use iglint_project::kinds::FileKind;
use iglint_project::model::ModelData;
use iglint_text::{TextRange, TextSize};

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
    summary: "`resource.json` and the files of the resource disagree, or it is missing",
    explanation: r#"## What it does

Compares the `files` list of a resource's `resource.json` with the files
in the resource directory: a listed file that does not exist, and a file
that exists but is not listed, are reported. Manifests without a `files`
list are not checked, and unlisted files are only reported for resource
types of Inductive Automation modules (third-party modules may keep
bookkeeping files next to their resources).

It also reports code the gateway never loads because it is not in a
resource:

* a resource directory without its manifest: a directory that holds the
  main file of a resource type (a `view.json` below `views/`, a `code.py`
  below `ignition/script-python/`, a timer script, a style class, ...) but
  no `resource.json` (`unary-resource.json` for tag folders). The files
  are still linted as that resource;
* a `view.json` or Python file in a project or gateway configuration
  collection that is not inside any resource directory, and a Python file
  in a resource directory that is not the resource's script (a
  `helper.py` next to a library `code.py`). They are linted on their own,
  like files outside a project.

## Why is this bad?

The gateway loads only the listed files. An unlisted file (for example a
`view.json` copied in by hand) is silently ignored, and a listed file that
is missing makes the resource incomplete or invalid on the gateway. A
directory without a manifest is a plain folder to the gateway: the view,
script or query in it does not exist at run time.

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
            report_outside_resource(cx);
            return;
        };
        let ws = cx.workspace();
        if resource.manifest.missing {
            let manifest = ws.file(resource.manifest.file).name().to_owned();
            for f in &resource.files {
                let entry = ws.file(*f);
                if !entry.kind.implies_resource() {
                    continue;
                }
                let Some(range) = cx.file(*f).map(|s| start_range(s.text())) else {
                    continue;
                };
                cx.report_in(
                    *f,
                    range,
                    format!(
                        "`{manifest}` is missing in this {} directory; the gateway does not load `{}`",
                        resource.kind.name(),
                        entry.name()
                    ),
                )
                .node_path(resource.qualified_path())
                .fingerprint_key(format!("missing manifest {}", entry.name()));
            }
            return;
        }
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

/// The first character of a file (empty for an empty file): where findings
/// about the whole file point.
fn start_range(text: &str) -> TextRange {
    let end = text.chars().next().map_or(0, char::len_utf8);
    TextRange::new(TextSize::new(0), TextSize::new(end as u32))
}

/// A `view.json` or Python file of a project or collection that is not in
/// any resource directory, or a Python file in a resource directory that is
/// not the resource's script (both linted standalone).
fn report_outside_resource(cx: &mut Checker<'_, '_>) {
    let primary = cx.primary_file();
    let Some(source) = cx.file(primary) else {
        return;
    };
    let Some(entry) = cx.workspace().file_by_path(source.path()) else {
        return;
    };
    let message = match (entry.kind, entry.resource) {
        (FileKind::Stray, _) if entry.container.is_some() => format!(
            "`{}` is not inside a resource directory (`<moduleId>/<typeId>/<path>/` with a `resource.json`); the gateway does not load it",
            entry.name()
        ),
        (FileKind::OtherResourceData, Some(r)) if entry.name().ends_with(".py") => {
            let resource = cx.workspace().resource(r);
            format!(
                "`{}` is not a file of this {} resource; the gateway does not load it",
                entry.name(),
                resource.kind.name()
            )
        }
        _ => return,
    };
    let range = start_range(source.text());
    cx.report(range, message)
        .fingerprint_key("outside resource");
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
    const PERSPECTIVE: &str = "projects/p/com.inductiveautomation.perspective";

    #[test]
    fn resources_without_manifest_are_linted_and_reported() {
        let view = r#"{"custom": {"unused": 1}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#;
        let f = Fixture::new()
            .file(&format!("{PERSPECTIVE}/views/V/view.json"), view)
            .file(
                "projects/p/ignition/script-python/lib/code.py",
                "def f():\n    return undefined_x\n",
            )
            .file(
                "projects/p/ignition/timer/t/handleTimerEvent.py",
                "def handleTimerEvent():\n    print undefined_y\n",
            )
            .file("config/c/config-mode.json", "{}")
            .file("config/c/ignition/tag-definition/default/tags.json", "{}");
        let d = f.lint(&["REF009", "VIEW015", "SCR101"]);
        let got: Vec<(String, String)> = d
            .iter()
            .map(|d| (d.rule.code().to_owned(), d.message.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "REF009".into(),
                    "`unary-resource.json` is missing in this tag-definition directory; the gateway does not load `tags.json`".into()
                ),
                (
                    "REF009".into(),
                    "`resource.json` is missing in this view directory; the gateway does not load `view.json`".into()
                ),
                ("VIEW015".into(), "Custom property `unused` is never read".into()),
                (
                    "REF009".into(),
                    "`resource.json` is missing in this script-library directory; the gateway does not load `code.py`".into()
                ),
                ("SCR101".into(), "Undefined name `undefined_x`".into()),
                (
                    "REF009".into(),
                    "`resource.json` is missing in this gateway-timer directory; the gateway does not load `handleTimerEvent.py`".into()
                ),
                ("SCR101".into(), "Undefined name `undefined_y`".into()),
            ]
        );
        assert_eq!(flagged(&d)[1], "{");
    }

    #[test]
    fn files_outside_resources_are_linted_and_reported() {
        let f = Fixture::new()
            .file(
                "projects/p/views/W/view.json",
                r#"{"custom": {"unused": 1}}"#,
            )
            .file("projects/p/loose.py", "def f():\n    return undefined_z\n")
            .file(
                "projects/p/ignition/script-python/lib/resource.json",
                r#"{"scope": "A", "version": 1}"#,
            )
            .file("projects/p/ignition/script-python/lib/code.py", "")
            .file(
                "projects/p/ignition/script-python/lib/helper.py",
                "def f():\n    return undefined_w\n",
            )
            // Folders and other files are not resources.
            .file(&format!("{PERSPECTIVE}/views/Folder/notes.txt"), "")
            .file("projects/p/ignition/script-python/pkg/readme.txt", "");
        let d = f.lint(&["REF009", "VIEW015", "SCR101"]);
        let got: Vec<(String, String)> = d
            .iter()
            .map(|d| (d.rule.code().to_owned(), d.message.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (
                    "REF009".into(),
                    "`helper.py` is not a file of this script-library resource; the gateway does not load it".into()
                ),
                ("SCR101".into(), "Undefined name `undefined_w`".into()),
                (
                    "REF009".into(),
                    "`loose.py` is not inside a resource directory (`<moduleId>/<typeId>/<path>/` with a `resource.json`); the gateway does not load it".into()
                ),
                ("SCR101".into(), "Undefined name `undefined_z`".into()),
                (
                    "REF009".into(),
                    "`view.json` is not inside a resource directory (`<moduleId>/<typeId>/<path>/` with a `resource.json`); the gateway does not load it".into()
                ),
                ("VIEW015".into(), "Custom property `unused` is never read".into()),
            ]
        );
    }

    #[test]
    fn files_outside_projects_are_not_reported() {
        let f = Fixture::new().file("views/W/view.json", r#"{"custom": {}}"#);
        assert!(f.lint(&["REF009"]).is_empty());
    }
}
