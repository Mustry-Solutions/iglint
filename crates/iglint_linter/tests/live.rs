//! The incremental workspace gives the same findings as a full check, and
//! re-analyses the files that depend on an edit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_linter::discovery::absolute;
use iglint_linter::engine::{CheckOptions, check};
use iglint_linter::live::{Dependents, LiveWorkspace};
use iglint_linter::rules::REGISTRY;
use iglint_linter::settings::{LinterSettings, ProjectSettings, RuleSettings};

fn all_rules(root: &Path) -> ProjectSettings {
    let rules = REGISTRY
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

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iglint_project/tests/fixtures")
        .join(name)
}

type Finding = (String, String, u32, u32, String, String);

fn key(d: &iglint_linter::Diagnostic, root: &Path) -> Finding {
    let path = absolute(d.file.path());
    (
        path.strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned(),
        d.rule.code().to_owned(),
        d.range.start().to_u32(),
        d.range.end().to_u32(),
        d.severity.to_string(),
        d.message.clone(),
    )
}

#[test]
fn same_findings_as_a_full_check() {
    for name in ["basic", "edge"] {
        let root = absolute(&fixture(name));
        let settings = all_rules(&root);
        let full = check(
            &settings,
            std::slice::from_ref(&root),
            &CheckOptions::default(),
        );
        let mut expected: Vec<Finding> = full.diagnostics.iter().map(|d| key(d, &root)).collect();
        expected.sort();

        let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
        let files = live.lintable_files();
        assert_eq!(files.len(), full.files_checked, "{name}");
        let mut actual: Vec<Finding> = live
            .check(&files)
            .iter()
            .flat_map(|f| f.diagnostics.iter().map(|d| key(d, &root)))
            .collect();
        actual.sort();
        assert!(!expected.is_empty());
        assert_eq!(actual, expected, "{name}");
    }
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

const VIEWS: &str = "projects/p/com.inductiveautomation.perspective/views";

fn view_with_param(param: &str) -> String {
    format!(
        r#"{{
  "params": {{"{param}": 1}},
  "propConfig": {{"params.{param}": {{"paramDirection": "input"}}}},
  "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}
}}"#
    )
}

const CALLER: &str = r#"{
  "root": {
    "type": "ia.container.flex",
    "meta": {"name": "root"},
    "children": [
      {
        "type": "ia.display.view",
        "meta": {"name": "Embedded"},
        "props": {"path": "Popup", "params": {"speed": 2}}
      }
    ]
  }
}"#;

fn project(dir: &Path) -> (PathBuf, PathBuf) {
    write(
        dir,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true}"#,
    );
    let manifest = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;
    write(dir, &format!("{VIEWS}/Popup/resource.json"), manifest);
    let popup = write(
        dir,
        &format!("{VIEWS}/Popup/view.json"),
        &view_with_param("speed"),
    );
    write(dir, &format!("{VIEWS}/Caller/resource.json"), manifest);
    let caller = write(dir, &format!("{VIEWS}/Caller/view.json"), CALLER);
    (absolute(&popup), absolute(&caller))
}

fn codes(live: &mut LiveWorkspace, path: &Path) -> Vec<&'static str> {
    live.check(&[path.to_path_buf()])[0]
        .diagnostics
        .iter()
        .map(|d| d.rule.code())
        .collect()
}

#[test]
fn edits_report_their_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let root = absolute(dir.path());
    let (popup, caller) = project(&root);
    let mut live = LiveWorkspace::new(all_rules(&root), vec![root.clone()]);
    assert!(!codes(&mut live, &caller).contains(&"REF007"));

    // Renaming the popup's parameter changes the caller's findings.
    let dependents = live.set_text(&popup, Some(Arc::from(view_with_param("rate"))));
    assert_eq!(
        dependents,
        Dependents::Files(vec![
            caller.parent().unwrap().join("resource.json"),
            caller.clone()
        ])
    );
    assert!(codes(&mut live, &caller).contains(&"REF007"));

    // An edit that keeps the summary affects nobody else.
    let same = view_with_param("rate").replace("\"root\"}", "\"root\"} ");
    assert_eq!(
        live.set_text(&popup, Some(Arc::from(same))),
        Dependents::None
    );

    // The caller stops referencing the popup: the popup's usage changes.
    let detached = CALLER.replace("\"Popup\"", "\"Elsewhere\"");
    match live.set_text(&caller, Some(Arc::from(detached))) {
        Dependents::Files(files) => assert!(files.contains(&popup)),
        other => panic!("{other:?}"),
    }
    assert!(codes(&mut live, &caller).contains(&"REF001"));

    // Back to the disk text.
    live.set_text(&caller, None);
    assert!(!codes(&mut live, &caller).contains(&"REF001"));
}

#[test]
fn fix_all_applies_fixes_without_touching_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = absolute(dir.path());
    let (popup, _) = project(&root);
    let mut live = LiveWorkspace::new(all_rules(&root), vec![root.clone()]);
    let text = r#"{
  "custom": {"unusedCounter": 1},
  "params": {"speed": 1},
  "propConfig": {"params.speed": {"paramDirection": "input"}},
  "root": {"type": "ia.container.flex", "meta": {"name": "root"}}
}"#;
    live.set_text(&popup, Some(Arc::from(text)));
    let found = live
        .check(std::slice::from_ref(&popup))
        .remove(0)
        .diagnostics;
    assert!(
        found
            .iter()
            .any(|d| d.rule.code() == "VIEW015" && d.fix.is_some()),
        "{found:#?}"
    );
    let fixed = live.fix_all(&popup, false).expect("fixes apply");
    assert_eq!(fixed.applied, 1);
    assert!(!fixed.fixed.contains("unusedCounter"));
    // The workspace still has the editor text.
    assert_eq!(live.source(&popup).unwrap().text(), text);
    live.set_text(&popup, Some(Arc::from(fixed.fixed)));
    let after = live
        .check(std::slice::from_ref(&popup))
        .remove(0)
        .diagnostics;
    assert!(!after.iter().any(|d| d.message.contains("unusedCounter")));
}

#[test]
fn update_matches_set_text_and_check() {
    let root = absolute(&fixture("basic"));
    let settings = all_rules(&root);
    let mut a = LiveWorkspace::new(settings.clone(), vec![root.clone()]);
    let mut b = LiveWorkspace::new(settings, vec![root.clone()]);
    for path in a.lintable_files() {
        let text = std::fs::read_to_string(&path).unwrap();
        // Truncated text exercises the error paths too.
        let half = (0..=text.len() / 2)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        for text in [text.clone(), text.get(..half).unwrap().to_owned()] {
            let (found, dependents) = a.update(&path, Arc::from(text.as_str()));
            let expected_dependents = b.set_text(&path, Some(Arc::from(text.as_str())));
            let expected = b.check(std::slice::from_ref(&path)).remove(0);
            assert_eq!(dependents, expected_dependents, "{}", path.display());
            let keys = |d: &[iglint_linter::Diagnostic]| -> Vec<Finding> {
                d.iter().map(|d| key(d, &root)).collect()
            };
            assert_eq!(
                keys(&found.diagnostics),
                keys(&expected.diagnostics),
                "{}",
                path.display()
            );
            assert_eq!(found.error, expected.error);
        }
    }
}

#[test]
fn script_references_match_a_full_check() {
    let dir = tempfile::tempdir().unwrap();
    let root = absolute(dir.path());
    write(
        &root,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true}"#,
    );
    let lib = r#"{"scope": "A", "version": 1, "restricted": false, "overridable": true, "files": ["code.py"], "attributes": {}}"#;
    write(
        &root,
        "projects/p/ignition/script-python/core/util/resource.json",
        lib,
    );
    let module = write(
        &root,
        "projects/p/ignition/script-python/core/util/code.py",
        "def fmt(value, digits=2):\n    return value\ndef unused():\n    pass\n",
    );
    let manifest = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;
    write(&root, &format!("{VIEWS}/Main/resource.json"), manifest);
    let script = "\\tcore.util.fmt(1, 2, 3)\\n\\tcore.util.nope()\\n\\tsystem.perspective.openPopup('p', 'Gone')\\n\\tsystem.db.runNamedQuery('Missing/Q')";
    write(
        &root,
        &format!("{VIEWS}/Main/view.json"),
        &format!(
            r#"{{"root": {{"type": "ia.input.button", "meta": {{"name": "root"}},
                "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G",
                "config": {{"script": "{script}"}}}}}}}}}}}}"#
        ),
    );
    let settings = all_rules(&root);
    let full = check(
        &settings,
        std::slice::from_ref(&root),
        &CheckOptions::default(),
    );
    let mut expected: Vec<Finding> = full.diagnostics.iter().map(|d| key(d, &root)).collect();
    expected.sort();
    for code in ["REF001", "REF002", "REF003", "REF004", "REF012"] {
        assert!(
            expected.iter().any(|f| f.1 == code),
            "{code} missing: {expected:?}"
        );
    }
    let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
    let files = live.lintable_files();
    let mut actual: Vec<Finding> = live
        .check(&files)
        .iter()
        .flat_map(|f| f.diagnostics.iter().map(|d| key(d, &root)))
        .collect();
    actual.sort();
    assert_eq!(actual, expected);

    // Editing the library changes the findings of the view that uses it.
    let (_, dependents) = live.update(
        &module,
        Arc::from("def fmt(value, digits=2, unit=None):\n    return value\ndef nope():\n    pass\ndef unused():\n    pass\n"),
    );
    let view = absolute(&root.join(format!("{VIEWS}/Main/view.json")));
    assert!(format!("{dependents:?}").contains("Main"), "{dependents:?}");
    let codes = codes(&mut live, &view);
    assert!(
        !codes.contains(&"REF002") && !codes.contains(&"REF003"),
        "{codes:?}"
    );
}

/// Resources without their manifest, files outside every resource and
/// files that are not linted get the same treatment in the editor as in a
/// full check.
#[test]
fn files_outside_resources_match_a_full_check() {
    let dir = tempfile::tempdir().unwrap();
    let root = absolute(dir.path());
    let view = r#"{"custom": {"unused": 1}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [{"type": "ia.display.label", "meta": {"name": "bad name"}, "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "now(1) +"}}}}}]}}"#;
    write(&root, "projects/p/project.json", r#"{"title": "p"}"#);
    write(&root, &format!("{VIEWS}/NoManifest/view.json"), view);
    write(&root, "projects/p/views/Stray/view.json", view);
    let script = "def f():\n    return undefined_x\n";
    write(&root, "projects/p/ignition/script-python/m/code.py", script);
    write(
        &root,
        "projects/p/ignition/timer/t/handleTimerEvent.py",
        script,
    );
    write(&root, "projects/p/loose.py", script);
    write(
        &root,
        "projects/p/other.module/things/x/resource.json",
        "{}",
    );
    write(&root, "projects/p/other.module/things/x/code.py", script);

    let settings = all_rules(&root);
    let full = check(
        &settings,
        std::slice::from_ref(&root),
        &CheckOptions::default(),
    );
    let mut expected: Vec<Finding> = full.diagnostics.iter().map(|d| key(d, &root)).collect();
    expected.sort();
    assert_eq!(full.skipped.len(), 1, "{:?}", full.skipped);
    let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
    let files = live.lintable_files();
    assert_eq!(files.len(), full.files_checked + full.skipped.len());
    let mut actual: Vec<Finding> = live
        .check(&files)
        .iter()
        .flat_map(|f| f.diagnostics.iter().map(|d| key(d, &root)))
        .collect();
    actual.sort();
    assert_eq!(actual, expected);
    for file in [
        "NoManifest/view.json",
        "Stray/view.json",
        "m/code.py",
        "t/handleTimerEvent.py",
        "loose.py",
    ] {
        let codes: Vec<&str> = expected
            .iter()
            .filter(|f| f.0.ends_with(file))
            .map(|f| f.1.as_str())
            .collect();
        assert!(codes.contains(&"REF009"), "{file}: {codes:?}");
        assert!(codes.len() > 1, "{file}: {codes:?}");
    }
    assert!(!expected.iter().any(|f| f.0.contains("other.module")));
}
