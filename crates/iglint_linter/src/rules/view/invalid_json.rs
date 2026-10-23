//! VIEW001 invalid-json.

use iglint_json::ParseErrorKind;
use iglint_project::model::ModelData;
use iglint_project::{MAX_JSON_DEPTH, ProjectId};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ProjectRule, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW001",
    name: "invalid-json",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Resource JSON does not parse",
    explanation: r#"## What it does

Reports resource files (`view.json`, `style.json`, `config.json`,
`tags.json`, `resource.json`, `project.json`, ...) that are not valid JSON,
at the first syntax error.

Arrays and objects nested more than 8,192 levels deep are reported too.
Gson parses any depth, but Perspective cannot open such a view: its view
loader and property trees recurse and overflow a gateway thread's stack at
1,200 to 3,300 levels (measured on Ignition 8.3.3). iglint does not
analyse files nested this deeply.

## Why is this bad?

The gateway cannot load a resource whose JSON does not parse: the view,
style class or tag folder silently disappears. Other files still lint, and
rules that need the broken file's content stay silent about it instead of
guessing.

## Example

```json
{
  "custom": {
    "count": 1,
  }
}
```

Use instead:

```json
{
  "custom": {
    "count": 1
  }
}
```

## ign-lint

ign-lint aborts the whole run on the first invalid file. iglint reports the
file and keeps going.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&InvalidJson), Hook::Project(&InvalidJson)],
    options: None,
};

struct InvalidJson;

/// `unexpected character, expected a value at 10..11` without the range.
fn strip_range(message: &str) -> &str {
    match message.rsplit_once(" at ") {
        Some((head, tail)) if tail.contains("..") => head,
        _ => message,
    }
}

/// The finding's message for a parse error message.
fn message(error: &str) -> String {
    if error == ParseErrorKind::NestingTooDeep.to_string() {
        format!(
            "Invalid JSON: nesting too deep (more than {} levels of arrays and objects); Perspective's view loader and property trees overflow the stack long before (measured at 1,200 to 3,300 levels)",
            fmt_thousands(MAX_JSON_DEPTH)
        )
    } else {
        format!("Invalid JSON: {error}")
    }
}

/// `8192` as `8,192`.
fn fmt_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl ResourceRule for InvalidJson {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, _data: &ModelData<'_>) {
        let errors = cx.unit().errors;
        for e in errors {
            // I/O errors are reported by the engine; their files have no
            // text and are not in the file table, so the report is dropped.
            cx.report_in(e.file, e.range, message(strip_range(&e.message)));
        }
        if let Some(resource) = cx.resource()
            && let Some(e) = resource.manifest.doc.error()
        {
            cx.report_in(
                resource.manifest.file,
                e.range,
                message(&e.kind.to_string()),
            );
        }
    }
}

impl ProjectRule for InvalidJson {
    fn check_project(&self, cx: &mut Checker<'_, '_>, project: ProjectId) {
        let p = cx.workspace().project(project);
        if let Some(e) = p.manifest.doc.error() {
            let range: TextRange = e.range;
            cx.report_in(p.manifest.file, range, message(&e.kind.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_syntax_errors_with_position() {
        let f = Fixture::view("{\n  \"custom\": {\n    \"a\": 1,\n  }\n}");
        let d = f.lint(&["VIEW001"]);
        assert_eq!(
            messages(&d),
            vec![("VIEW001".into(), "Invalid JSON: trailing comma".into())]
        );
        assert_eq!(flagged(&d), vec![","]);
    }

    #[test]
    fn nesting_beyond_the_limit() {
        let deep = |n: usize| {
            format!(
                r#"{{"custom": {{"c": {}1{}}}, "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#,
                "[".repeat(n),
                "]".repeat(n)
            )
        };
        assert!(Fixture::view(&deep(8_000)).lint(&["VIEW001"]).is_empty());
        let d = Fixture::view(&deep(9_000)).lint(&["VIEW001"]);
        assert_eq!(d.len(), 1);
        assert!(
            d[0].message
                .contains("nesting too deep (more than 8,192 levels")
        );
        assert_eq!(flagged(&d), vec!["["]);
    }

    #[test]
    fn valid_files_and_other_resources_are_clean() {
        let f =
            Fixture::view(r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#)
                .with_view("Other", "{}");
        assert!(f.lint(&["VIEW001"]).is_empty());
    }

    #[test]
    fn manifests_and_standalone_views() {
        let f = Fixture::view("{}")
            .file("projects/p/project.json", "{\"title\": ")
            .file("loose/view.json", "[1 2]");
        let d = f.lint(&["VIEW001"]);
        let mut found: Vec<String> = d
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
        found.sort();
        assert_eq!(found, vec!["project.json", "view.json"]);
    }
}
