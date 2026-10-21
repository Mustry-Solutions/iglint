//! VIEW011 duplicate-json-key.

use iglint_json::{Value, ValueKind};
use iglint_project::model::ModelData;
use iglint_project::{FileId, ProjectId};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ProjectRule, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW011",
    name: "duplicate-json-key",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "JSON object has the same key twice",
    explanation: r#"## What it does

Reports a key that occurs more than once in the same JSON object, in any
JSON file of a resource (`view.json`, `style.json`, `tags.json`,
`config.json`, `resource.json`, `project.json`, ...). Every occurrence but
the last is reported.

## Why is this bad?

Ignition reads the file with Gson, which keeps only the last value of a
repeated key. The earlier values are silently ignored, and the next save
from the Designer drops them. Duplicates come from merge conflicts resolved
by hand or from generated files.

## Example

```json
"props": {"text": "Start", "text": "Stop"}
```

Use instead:

```json
"props": {"text": "Stop"}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[
        Hook::Resource(&DuplicateJsonKey),
        Hook::Project(&DuplicateJsonKey),
    ],
    options: None,
};

struct DuplicateJsonKey;

fn check_tree(cx: &mut Checker<'_, '_>, file: FileId, root: &Value<'_>) {
    root.walk(|_, value| {
        if let ValueKind::Object(object) = &value.kind
            && object.members.len() > 1
        {
            for group in object.duplicate_keys() {
                let last = *group.last().expect("group has members");
                for &i in &group[..group.len() - 1] {
                    let member = &object.members[i];
                    cx.report_in(
                        file,
                        member.key.range(),
                        format!(
                            "Duplicate key `{}`; Ignition keeps only the last value",
                            member.key()
                        ),
                    )
                    .label(
                        file,
                        object.members[last].key.range(),
                        "value that is used",
                    );
                }
            }
        }
        true
    });
}

impl ResourceRule for DuplicateJsonKey {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        match data {
            ModelData::View { file, view } => check_tree(cx, *file, view.root),
            ModelData::StyleClass(s) => check_tree(cx, s.file, s.root),
            ModelData::PageConfig(p) => check_tree(cx, p.file, p.root),
            ModelData::SessionProps { file, props } => check_tree(cx, *file, props.root),
            ModelData::Gateway(g) => {
                if let (Some(file), Some(config)) = (g.config_file, g.config) {
                    check_tree(cx, file, config);
                }
            }
            ModelData::Tags(t) => {
                // Tag files are modelled per tag; parse each file's text
                // to see its whole tree.
                for (file, _) in &t.files {
                    let Some(source) = cx.file(*file) else {
                        continue;
                    };
                    if let Ok(root) = iglint_json::parse(source.text()) {
                        check_tree(cx, *file, &root);
                    }
                }
            }
            ModelData::WebDev(w) => {
                if let (Some(config), Some(resource)) = (w.config, cx.resource()) {
                    let ws = cx.workspace();
                    if let Some(f) = ws.resource_file(resource.id, "config.json") {
                        check_tree(cx, f.id, config);
                    }
                }
            }
            _ => {}
        }
        if let Some(resource) = cx.resource()
            && let Some(root) = resource.manifest.doc.root()
        {
            check_tree(cx, resource.manifest.file, root);
        }
    }
}

impl ProjectRule for DuplicateJsonKey {
    fn check_project(&self, cx: &mut Checker<'_, '_>, project: ProjectId) {
        let p = cx.workspace().project(project);
        if let Some(root) = p.manifest.doc.root() {
            check_tree(cx, p.manifest.file, root);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_all_but_last_occurrence() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.display.label", "meta": {"name": "root"},
                "props": {"text": "a", "x": 1, "text": "b", "text": "c"}}}"#,
        );
        let d = f.lint(&["VIEW011"]);
        assert_eq!(d.len(), 2);
        assert_eq!(flagged(&d), vec!["\"text\"", "\"text\""]);
        assert_eq!(d[0].labels[0].range.slice(d[0].file.text()), "\"text\"");
        assert_eq!(
            d[0].message,
            "Duplicate key `text`; Ignition keeps only the last value"
        );
    }

    #[test]
    fn manifests_and_other_resources() {
        let f = Fixture::view("{}")
            .file("projects/p/project.json", r#"{"title": "a", "title": "b"}"#)
            .resource(
                "projects/p/com.inductiveautomation.perspective/style-classes/S",
                &[(
                    "style.json",
                    r#"{"base": {"style": {"color": "red", "color": "blue"}}}"#,
                )],
            );
        let d = f.lint(&["VIEW011"]);
        let files: Vec<String> = d
            .iter()
            .map(|d| {
                d.file
                    .path()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files.contains(&"project.json".to_owned()));
        assert!(files.contains(&"style.json".to_owned()));
    }

    #[test]
    fn same_keys_in_different_objects_are_fine() {
        let f = Fixture::view(r#"{"a": {"x": 1}, "b": {"x": 2}}"#);
        assert!(f.lint(&["VIEW011"]).is_empty());
    }
}
