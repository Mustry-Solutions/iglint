//! Engine tests with the fake rules of `iglint_linter::testing`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_linter::discovery::{SkipReason, discover};
use iglint_linter::engine::{CheckOptions, FixMode, check};
use iglint_linter::registry::RuleCode;
use iglint_linter::settings::{LinterSettings, ProjectSettings, RuleSettings};
use iglint_linter::{FileKind, testing};

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn settings(root: &Path) -> ProjectSettings {
    let rules: BTreeMap<RuleCode, RuleSettings> = testing::REGISTRY
        .iter()
        .map(|rule| {
            (
                rule,
                RuleSettings {
                    severity: rule.meta().default_severity,
                    options: BTreeMap::new(),
                },
            )
        })
        .collect();
    ProjectSettings {
        root: root.to_path_buf(),
        base: Arc::new(LinterSettings {
            rules,
            strict: false,
        }),
        ..ProjectSettings::default()
    }
}

fn options(fix_mode: FixMode) -> CheckOptions {
    CheckOptions {
        fix_mode,
        analyzer: Some(testing::analyzer()),
        ..CheckOptions::default()
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn discovery_classifies_and_skips() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "projects/p/project.json", "{}");
    write(
        root,
        "projects/p/com.inductiveautomation.perspective/views/A/view.json",
        "{}",
    );
    write(
        root,
        "projects/p/com.inductiveautomation.perspective/views/A/resource.json",
        "{}",
    );
    write(
        root,
        "projects/p/com.inductiveautomation.perspective/views/A/thumbnail.png",
        "",
    );
    write(root, "projects/p/ignition/script-python/lib/code.py", "");
    write(
        root,
        "projects/p/ignition/script-python/lib/resource.json",
        "{}",
    );
    write(root, "projects/p/ignition/timer/t/handleTimerEvent.py", "");
    write(root, "projects/p/ignition/timer/t/resource.json", "{}");
    write(
        root,
        "projects/p/com.inductiveautomation.perspective/style-classes/S/style.json",
        "{}",
    );
    write(
        root,
        "projects/p/com.inductiveautomation.perspective/style-classes/S/resource.json",
        "{}",
    );
    write(root, "package.json", "{}");
    write(root, "tools/gen.py", "");
    write(root, ".hidden/view.json", "{}");
    write(root, "ignored/view.json", "{}");
    write(root, ".gitignore", "ignored/\n");
    write(root, "legacy/view.json", "{}");
    write(root, "old/view.json", "{}");

    let mut settings = settings(root);
    settings.exclude = vec!["legacy".into()];
    settings.whitelist = vec![root.join("old/view.json")];
    let found = discover(&settings, &[root.to_path_buf()]);
    let files: Vec<(String, FileKind)> = found
        .files
        .iter()
        .map(|(p, k)| (relative(root, p), *k))
        .collect();
    let base = "projects/p/com.inductiveautomation.perspective";
    assert_eq!(
        files,
        vec![
            (
                format!("{base}/style-classes/S/resource.json"),
                FileKind::Manifest
            ),
            (
                format!("{base}/style-classes/S/style.json"),
                FileKind::ResourceJson
            ),
            (format!("{base}/views/A/resource.json"), FileKind::Manifest),
            (format!("{base}/views/A/view.json"), FileKind::View),
            (
                "projects/p/ignition/script-python/lib/code.py".into(),
                FileKind::ScriptModule
            ),
            (
                "projects/p/ignition/script-python/lib/resource.json".into(),
                FileKind::Manifest
            ),
            (
                "projects/p/ignition/timer/t/handleTimerEvent.py".into(),
                FileKind::ResourceScript
            ),
            (
                "projects/p/ignition/timer/t/resource.json".into(),
                FileKind::Manifest
            ),
            ("projects/p/project.json".into(), FileKind::Manifest),
        ]
    );
    let skipped: Vec<(String, SkipReason)> = found
        .skipped
        .iter()
        .map(|(p, r)| (relative(root, p), r.clone()))
        .collect();
    assert_eq!(
        skipped,
        vec![("old/view.json".into(), SkipReason::Whitelisted)]
    );

    // Without .gitignore support the ignored directory is linted.
    settings.respect_gitignore = false;
    let found = discover(&settings, &[root.to_path_buf()]);
    assert!(
        found
            .files
            .iter()
            .any(|(p, _)| p.ends_with("ignored/view.json"))
    );

    // Explicit files: unsupported ones and loose JSON.
    let found = discover(
        &settings,
        &[
            root.join("package.json"),
            root.join("legacy/view.json"),
            root.join(format!("{base}/views/A/thumbnail.png")),
            root.join("nope.py"),
        ],
    );
    assert_eq!(
        found.files,
        vec![(root.join("package.json"), FileKind::Json)]
    );
    assert_eq!(found.skipped.len(), 2);
    assert_eq!(found.errors.len(), 1);
}

#[test]
fn walks_skip_python_outside_ignition_trees() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tools/x.py", "");
    write(root, "projects/p/project.json", "{}");
    write(root, "projects/p/scripts/stray.py", "");
    write(root, "projects/p/sub/deeper/stray.py", "");
    write(root, "config/config-mode.json", "{}");
    write(root, "config/x/stray.py", "");
    write(root, "views/view.json", "{}");
    let settings = settings(root);

    let found = discover(&settings, &[root.to_path_buf()]);
    let files: Vec<(String, FileKind)> = found
        .files
        .iter()
        .map(|(p, k)| (relative(root, p), *k))
        .collect();
    assert_eq!(
        files,
        vec![
            ("config/config-mode.json".into(), FileKind::Manifest),
            ("config/x/stray.py".into(), FileKind::Python),
            ("projects/p/project.json".into(), FileKind::Manifest),
            ("projects/p/scripts/stray.py".into(), FileKind::Python),
            ("projects/p/sub/deeper/stray.py".into(), FileKind::Python),
            ("views/view.json".into(), FileKind::View),
        ]
    );
    assert!(found.skipped.is_empty());

    // Walking a subdirectory still sees the project above it.
    let found = discover(&settings, &[root.join("projects/p/sub")]);
    assert_eq!(found.files.len(), 1);
    // Walking the loose directory itself finds nothing.
    assert!(discover(&settings, &[root.join("tools")]).files.is_empty());

    // Named explicitly, the loose file is linted.
    let found = discover(&settings, &[root.join("tools/x.py")]);
    assert_eq!(
        found.files,
        vec![(root.join("tools/x.py"), FileKind::Python)]
    );
}

#[test]
fn explicit_loose_python_is_linted_as_jython() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let loose = write(root, "tools/x.py", "def broken(:\n");
    write(root, "projects/p/project.json", "{}");
    let rules: BTreeMap<RuleCode, RuleSettings> = iglint_linter::rules::REGISTRY
        .iter()
        .map(|rule| {
            (
                rule,
                RuleSettings {
                    severity: rule.meta().default_severity,
                    options: BTreeMap::new(),
                },
            )
        })
        .collect();
    let settings = ProjectSettings {
        root: root.to_path_buf(),
        base: Arc::new(LinterSettings {
            rules,
            strict: false,
        }),
        ..ProjectSettings::default()
    };
    let options = CheckOptions::default();

    let walked = check(&settings, &[root.to_path_buf()], &options);
    assert!(walked.files.iter().all(|f| f.path != loose));
    assert!(walked.diagnostics.is_empty());

    let named = check(&settings, std::slice::from_ref(&loose), &options);
    assert_eq!(named.files_checked, 1);
    let rules: Vec<&str> = named.diagnostics.iter().map(|d| d.rule.code()).collect();
    assert_eq!(rules, ["SCR001"]);
}

#[test]
fn check_reports_sorted_diagnostics_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "project.json", "{}");
    write(root, "b.py", "x = undefined_thing\nunused_var = 1\n");
    write(root, "a.py", "undefined_thing\n");
    std::fs::write(root.join("bad.py"), [0xff, 0xfe]).unwrap();
    let result = check(
        &settings(root),
        &[root.to_path_buf()],
        &options(FixMode::None),
    );
    let found: Vec<(String, &str)> = result
        .diagnostics
        .iter()
        .map(|d| (relative(root, d.file.path()), d.rule.code()))
        .collect();
    assert_eq!(
        found,
        vec![
            ("a.py".into(), "SCR101"),
            ("b.py".into(), "SCR101"),
            ("b.py".into(), "SCR102"),
        ]
    );
    assert_eq!(result.files_checked, 3);
    assert_eq!(result.errors.len(), 1);
    assert_eq!(result.errors[0].message, "file is not valid UTF-8");
    assert!(result.fixed.is_empty());
}

#[test]
fn safe_fixes_are_written_and_preserve_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // Comments: the fake fixes rename words, and a safe fix that renamed
    // Python names would fail the safe-fix net (names resolve differently).
    let file = write(
        root,
        "a.py",
        "# unused_var = 1\r\n# unusedProp\r\n# badName\r\n",
    );
    let result = check(
        &settings(root),
        std::slice::from_ref(&file),
        &options(FixMode::Safe),
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# _ = 1\r\n# unusedProp\r\n# BadName\r\n"
    );
    assert_eq!(result.fixed.len(), 1);
    let summary = &result.fixed[0];
    assert!(summary.written);
    assert_eq!(summary.applied.len(), 2);
    assert_eq!(summary.skipped.len(), 1);
    assert_eq!(summary.skipped[0].reason, "unsafe fix (use --unsafe-fixes)");
    // The remaining finding is the unsafe one.
    let codes: Vec<_> = result.diagnostics.iter().map(|d| d.rule.code()).collect();
    assert_eq!(codes, ["VIEW001"]);
}

#[test]
fn unsafe_fixes_diff_only_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = write(root, "a.py", "# unusedProp unused_var\n");

    let result = check(
        &settings(root),
        std::slice::from_ref(&file),
        &options(FixMode::DiffOnly { unsafe_fixes: true }),
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# unusedProp unused_var\n"
    );
    assert_eq!(result.fixed[0].fixed, "# usedProp _\n");
    assert!(!result.fixed[0].written);
    assert!(result.diagnostics.is_empty());

    let mut unfixable = settings(root);
    unfixable.unfixable = BTreeSet::from([RuleCode::new(&testing::UNUSED_VARIABLE)]);
    let result = check(
        &unfixable,
        std::slice::from_ref(&file),
        &options(FixMode::Unsafe),
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# usedProp unused_var\n"
    );
    assert_eq!(
        result.fixed[0].skipped[0].reason,
        "the rule is marked unfixable"
    );

    let mut only = options(FixMode::Unsafe);
    only.fix_rules = Some(BTreeSet::from([RuleCode::new(&testing::COMPONENT_NAME)]));
    let result = check(&settings(root), std::slice::from_ref(&file), &only);
    assert!(result.fixed[0].applied.is_empty());
    assert_eq!(
        result.fixed[0].skipped[0].reason,
        "filtered by the selected fix rules"
    );
}

#[test]
fn stdin_is_linted_and_fixed_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut options = options(FixMode::Safe);
    options.stdin = Some((root.join("x/code.py"), "# unused_var\n".to_owned()));
    let result = check(&settings(root), &[], &options);
    assert_eq!(result.stdin_output.as_deref(), Some("# _\n"));
    assert!(!root.join("x/code.py").exists());

    options.fix_mode = FixMode::None;
    let result = check(&settings(root), &[], &options);
    assert_eq!(result.diagnostics.len(), 1);
    assert_eq!(result.diagnostics[0].file.path(), root.join("x/code.py"));
}

#[test]
fn cache_serves_unchanged_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = write(root, "a.py", "undefined_thing unused_var\n");
    let mut options = options(FixMode::None);
    options.cache_dir = Some(root.join(".iglint_cache"));
    options.threads = Some(2);
    let first = check(&settings(root), std::slice::from_ref(&file), &options);
    assert!(!first.files[0].cached);
    let second = check(&settings(root), std::slice::from_ref(&file), &options);
    assert!(second.files[0].cached);
    let render = |r: &iglint_linter::CheckResult| {
        r.diagnostics
            .iter()
            .map(|d| format!("{:?} {:?} {} {:?}", d.rule, d.range, d.message, d.fix))
            .collect::<Vec<_>>()
    };
    assert_eq!(render(&first), render(&second));
    assert!(root.join(".iglint_cache/.gitignore").is_file());

    std::fs::write(&file, "undefined_thing\n").unwrap();
    let third = check(&settings(root), std::slice::from_ref(&file), &options);
    assert!(!third.files[0].cached);
    assert_eq!(third.diagnostics.len(), 1);

    // Different settings invalidate the entry too.
    let mut fewer = settings(root);
    let mut base = (*fewer.base).clone();
    base.rules.remove(&RuleCode::new(&testing::UNDEFINED_NAME));
    fewer.base = Arc::new(base);
    let fourth = check(&fewer, &[file], &options);
    assert!(!fourth.files[0].cached);
    assert!(fourth.diagnostics.is_empty());
}

/// An analyzer whose fixes can break the file: `fine` is fixed to `FINE`,
/// `breakme` to `BROKEN`, and `half` to `HALF`; the health rules (SCR001)
/// report `BROKEN`, and `HALF` twice on one line.
fn breaking_analyzer() -> iglint_linter::engine::Analyzer {
    use iglint_linter::diagnostic::{Diagnostic, FileEdit, Fix, Severity};
    use iglint_text::{TextEdit, TextRange, TextSize};
    Arc::new(|file, _kind, settings: &LinterSettings| {
        let syntax = RuleCode::new(&iglint_linter::rules::script::syntax_error::META);
        let fixable = RuleCode::new(&testing::UNUSED_VARIABLE);
        let text = file.text();
        let mut out = Vec::new();
        if settings.is_enabled(syntax) {
            for line in text.lines() {
                if line.contains("BROKEN") || line.matches("HALF").count() > 1 {
                    out.push(Diagnostic::new(
                        syntax,
                        Severity::Error,
                        "SyntaxError: broken".to_owned(),
                        file.clone(),
                        TextRange::default(),
                    ));
                }
            }
        }
        if settings.is_enabled(fixable) {
            for (word, replacement) in [("fine", "FINE"), ("breakme", "BROKEN"), ("half", "HALF")] {
                for (offset, _) in text.match_indices(word) {
                    let range = TextRange::at(TextSize::from(offset), TextSize::of(word));
                    let mut d = Diagnostic::new(
                        fixable,
                        Severity::Warning,
                        format!("`{word}`"),
                        file.clone(),
                        range,
                    );
                    d.fix = Some(Fix::safe(
                        format!("Replace `{word}`"),
                        vec![FileEdit {
                            path: file.path().to_path_buf(),
                            edit: TextEdit::replace(range, replacement),
                        }],
                    ));
                    out.push(d);
                }
            }
        }
        out
    })
}

#[test]
fn fixes_that_break_the_file_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let text = "fine breakme fine\nhalf half\nhalf\n";
    let file = write(root, "a.py", text);
    let options = CheckOptions {
        fix_mode: FixMode::Safe,
        analyzer: Some(breaking_analyzer()),
        ..CheckOptions::default()
    };
    let result = check(&settings(root), std::slice::from_ref(&file), &options);
    // Each `half` alone is fine, both on one line are not: the first is
    // kept, the second dropped.
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "FINE breakme FINE\nHALF half\nHALF\n"
    );
    let summary = &result.fixed[0];
    assert!(summary.written);
    assert_eq!(summary.applied.len(), 4);
    let harmful: Vec<_> = summary
        .skipped
        .iter()
        .filter(|s| s.harmful)
        .map(|s| (s.message.as_str(), s.reason.as_str()))
        .collect();
    assert_eq!(
        harmful,
        [
            (
                "Replace `breakme`",
                "the fix makes the file worse (SCR001: SyntaxError: broken)"
            ),
            (
                "Replace `half`",
                "the fix makes the file worse (SCR001: SyntaxError: broken)"
            ),
        ]
    );
}

#[test]
fn a_file_is_never_written_worse() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let file = write(root, "a.py", "breakme\n");
    let options = CheckOptions {
        fix_mode: FixMode::Safe,
        analyzer: Some(breaking_analyzer()),
        ..CheckOptions::default()
    };
    let result = check(&settings(root), std::slice::from_ref(&file), &options);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "breakme\n");
    let summary = &result.fixed[0];
    assert!(!summary.written);
    assert!(summary.applied.is_empty());
    assert!(summary.skipped.iter().any(|s| s.harmful));
}
