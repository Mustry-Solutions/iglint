//! VIEW007 invalid-utf8.

use iglint_project::model::ModelData;
use iglint_project::{FileId, ProjectId};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ProjectRule, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW007",
    name: "invalid-utf8",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "File is not valid UTF-8",
    explanation: r#"## What it does

Reports resource files (`code.py`, `view.json`, `resource.json`,
`project.json`, `query.sql`, ...) that are not valid UTF-8, at the first
invalid byte sequence; the others are labels of the same finding.

## Why is this bad?

Ignition reads project resources as UTF-8 and silently replaces every
invalid byte sequence with U+FFFD (`�`, the replacement character).
A file saved in another encoding, typically Latin-1 or Windows-1252 from an
external editor, therefore loads with its accented characters destroyed:
`"caf\xe9"` becomes `"caf�"` in a string, and an invalid byte in code
is a syntax error. A PEP 263 declaration such as `# -*- coding: latin-1
-*-` does not help: Ignition compiles scripts from text and rejects it
(`SCR001`).

iglint lints the rest of the file as Ignition sees it. Findings of other
rules at an invalid sequence are not reported (they would be about the
replacement character), and the file gets no automatic fixes, so its bytes
are never rewritten.

## Example

A library script saved as Latin-1 (the `é` is the single byte `0xE9`):

```python
LABEL = "Café"
```

Use instead: the same script saved as UTF-8 (the `é` is `0xC3 0xA9`).
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&InvalidUtf8), Hook::Project(&InvalidUtf8)],
    options: None,
};

struct InvalidUtf8;

fn report_file(cx: &mut Checker<'_, '_>, file: FileId) {
    let Some(source) = cx.file(file) else {
        return;
    };
    let Some((first, rest)) = source.invalid_utf8().split_first() else {
        return;
    };
    let count = source.invalid_utf8().len();
    let message = if count == 1 {
        "File is not valid UTF-8: Ignition replaces this byte sequence with U+FFFD; save the file as UTF-8".to_owned()
    } else {
        format!(
            "File is not valid UTF-8 ({count} invalid byte sequences): Ignition replaces them with U+FFFD; save the file as UTF-8"
        )
    };
    let mut report = cx.report_in(file, *first, message);
    for range in rest {
        report = report.label(file, *range, "invalid UTF-8");
    }
}

impl ResourceRule for InvalidUtf8 {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, _data: &ModelData<'_>) {
        match cx.resource() {
            Some(resource) => {
                for file in resource.files.clone() {
                    report_file(cx, file);
                }
            }
            // A standalone file.
            None => report_file(cx, cx.primary_file()),
        }
    }
}

impl ProjectRule for InvalidUtf8 {
    fn check_project(&self, cx: &mut Checker<'_, '_>, project: ProjectId) {
        let file = cx.workspace().project(project).manifest.file;
        report_file(cx, file);
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::FixMode;
    use crate::rules::test_support::*;

    const LIBRARY: &str = "projects/p/ignition/script-python/m";

    fn library(code: &[u8]) -> Fixture {
        Fixture::new()
            .resource(LIBRARY, &[("code.py", "")])
            .bytes(&format!("{LIBRARY}/code.py"), code)
    }

    #[test]
    fn latin1_library_module_is_linted_not_an_error() {
        let f = library(b"# -*- coding: latin-1 -*-\nx = \"caf\xe9\"\n");
        // `Fixture::lint` asserts that the run has no errors.
        let d = f.lint(&["VIEW007", "SCR001"]);
        assert_eq!(
            messages(&d),
            [
                (
                    "SCR001".to_owned(),
                    d[0].message.clone() // the encoding declaration
                ),
                (
                    "VIEW007".to_owned(),
                    "File is not valid UTF-8: Ignition replaces this byte sequence with U+FFFD; save the file as UTF-8".to_owned()
                ),
            ]
        );
        assert!(
            d[0].message.contains("encoding declaration"),
            "{}",
            d[0].message
        );
        // The finding points at the byte on disk.
        assert_eq!(d[1].range, iglint_text::TextRange::from(34..35));
        assert_eq!(
            d[1].file
                .line_index()
                .line_column(d[1].range.start(), d[1].file.text())
                .line,
            2
        );
    }

    #[test]
    fn findings_at_the_invalid_bytes_are_dropped() {
        // In code, U+FFFD is a syntax error: SCR001 would report the
        // substitute character, VIEW007 explains it.
        let f = library(b"x = 1 \xe2\x82 2\n");
        let d = f.lint(&["VIEW007", "SCR001"]);
        assert_eq!(flagged(&d), ["??"]);
        assert_eq!(d[0].rule.code(), "VIEW007");
        // Two invalid bytes in a row are two sequences.
        let f = library(b"x = 1 \xe9\xe8 2\n");
        let d = f.lint(&["VIEW007", "SCR001"]);
        assert_eq!(flagged(&d), ["?"]);
        assert_eq!(d[0].labels.len(), 1);
        // Several sequences are one finding with labels.
        let f = library(b"a = '\xe9'\nb = '\xe8'\nc = '\xff'\n");
        let d = f.lint(&["VIEW007"]);
        assert_eq!(d.len(), 1);
        assert!(
            d[0].message.contains("3 invalid byte sequences"),
            "{}",
            d[0].message
        );
        assert_eq!(d[0].labels.len(), 2);
    }

    #[test]
    fn rest_of_the_file_is_linted_without_fixes() {
        let code: &[u8] = b"# caf\xe9\ntry:\n    f()\nexcept ValueError, TypeError:\n    pass\n";
        let f = library(code);
        let d = f.lint(&["VIEW007", "SCR115"]);
        assert_eq!(
            d.iter().map(|d| d.rule.code()).collect::<Vec<_>>(),
            ["VIEW007", "SCR115"]
        );
        assert!(d[1].fix.is_none(), "no fix in a file that is not UTF-8");
        let path = f.path(&format!("{LIBRARY}/code.py"));
        let result = f.run(&[("SCR115", "")], FixMode::Unsafe);
        assert!(result.fixed.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), code);
    }

    #[test]
    fn json_resources_and_manifests() {
        let view = b"{\"custom\": {\"label\": \"caf\xe9\"}, \"root\": {\"type\": \"ia.container.flex\", \"meta\": {\"name\": \"root\"}}}";
        let f = Fixture::new()
            .resource(
                &format!("projects/p/{PERSPECTIVE}/views/Main"),
                &[("view.json", "")],
            )
            .bytes(
                &format!("projects/p/{PERSPECTIVE}/views/Main/view.json"),
                view,
            )
            .bytes(
                "projects/p/project.json",
                b"{\"title\": \"\xe9\", \"enabled\": true, \"inheritable\": false}",
            );
        let d = f.lint(&["VIEW007", "VIEW001"]);
        let mut found: Vec<String> = d
            .iter()
            .map(|d| {
                format!(
                    "{} {}",
                    d.rule.code(),
                    d.file.path().file_name().unwrap().to_string_lossy()
                )
            })
            .collect();
        found.sort();
        assert_eq!(found, ["VIEW007 project.json", "VIEW007 view.json"]);
    }

    #[test]
    fn standalone_files() {
        let f = Fixture::new().bytes("loose/view.json", b"{\"custom\": {\"a\": \"\xe9\"}}");
        let d = f.lint(&["VIEW007", "VIEW001"]);
        assert_eq!(flagged(&d), ["?"]);
        assert_eq!(d[0].rule.code(), "VIEW007");
    }

    #[test]
    fn utf8_files_are_clean() {
        let f = library("x = \"caf\u{e9}\"\n".as_bytes());
        assert!(f.lint(&["VIEW007"]).is_empty());
    }

    #[test]
    fn cache_sees_the_difference_with_a_real_substitute() {
        let f = library(b"x = '?'\n");
        let dir = tempfile::tempdir().unwrap();
        let settings = settings(f.root(), &[("VIEW007", "")]);
        let options = crate::engine::CheckOptions {
            cache_dir: Some(dir.path().to_path_buf()),
            ..crate::engine::CheckOptions::default()
        };
        let run = || crate::engine::check(&settings, &[f.root().to_path_buf()], &options);
        assert!(run().diagnostics.is_empty());
        std::fs::write(f.path(&format!("{LIBRARY}/code.py")), b"x = '\xe9'\n").unwrap();
        assert_eq!(run().diagnostics.len(), 1);
    }
}
