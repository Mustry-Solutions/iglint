//! The result cache after a one-file change: every run with the cache gives
//! exactly the findings of a run without it, for every file of the basic
//! fixture workspace changed in a way that keeps its summary (a trailing
//! newline) and in ways that change it (a renamed word, an emptied file),
//! with every rule.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_linter::engine::{CheckOptions, check};
use iglint_linter::rules::ALL;
use iglint_linter::settings::{LinterSettings, ProjectSettings, RuleSettings};
use iglint_linter::{RuleCode, RuleMeta};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if !path.ends_with(".iglint_cache") {
                files(&path, out);
            }
        } else if path
            .extension()
            .is_some_and(|e| e == "json" || e == "py" || e == "sql")
        {
            out.push(path);
        }
    }
}

fn settings(root: &Path) -> ProjectSettings {
    let rules = ALL
        .iter()
        .map(|meta: &&'static RuleMeta| {
            (
                RuleCode::new(meta),
                RuleSettings {
                    severity: meta.default_severity,
                    options: BTreeMap::new(),
                },
            )
        })
        .collect();
    ProjectSettings {
        root: root.to_path_buf(),
        base: Arc::new(LinterSettings {
            rules,
            strict: true,
        }),
        ..ProjectSettings::default()
    }
}

fn render(root: &Path, cache: Option<PathBuf>) -> (Vec<String>, usize) {
    let options = CheckOptions {
        cache_dir: cache,
        ..CheckOptions::default()
    };
    let result = check(&settings(root), &[root.to_path_buf()], &options);
    let mut out: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} {} {:?} {} {:?} {:?}",
                d.file.path().display(),
                d.rule.code(),
                d.range,
                d.message,
                d.labels,
                d.fix
            )
        })
        .collect();
    out.sort();
    let cached = result.files.iter().filter(|f| f.cached).count();
    (out, cached)
}

#[test]
fn one_file_changes_give_uncached_findings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../iglint_project/tests/fixtures/basic"),
        &root,
    );
    let cache = root.join(".iglint_cache");
    let mut all = Vec::new();
    files(&root, &mut all);
    all.sort();
    assert!(all.len() > 50);
    let (baseline, _) = render(&root, None);
    assert!(!baseline.is_empty());
    let mut reused = 0;
    for file in &all {
        let original = std::fs::read_to_string(file).unwrap();
        let toggled = match original.strip_suffix('\n') {
            Some(t) => t.to_owned(),
            None => format!("{original}\n"),
        };
        let renamed = original
            .replacen("Motor", "Motr", 1)
            .replacen("line", "lin", 1);
        // Emptied: the resource loses everything other files refer to.
        let emptied = if original.trim_start().starts_with('[') {
            "[]".to_owned()
        } else if file.extension().is_some_and(|e| e == "json") {
            "{}".to_owned()
        } else {
            "\n".to_owned()
        };
        for changed in [toggled, renamed, emptied] {
            render(&root, Some(cache.clone()));
            std::fs::write(file, &changed).unwrap();
            let (with_cache, cached) = render(&root, Some(cache.clone()));
            let (without, _) = render(&root, None);
            assert_eq!(with_cache, without, "after changing {}", file.display());
            reused += cached;
            std::fs::write(file, &original).unwrap();
        }
    }
    // Most changes keep other units' results.
    assert!(reused > 0);
    assert_eq!(render(&root, None).0, baseline);
}

/// A view that passes a parameter to another view: renaming that
/// parameter in the target changes the caller's findings, although the
/// caller did not change. The caller's cached result must not be served.
#[test]
fn a_changed_summary_invalidates_dependent_units() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let views = "projects/p/com.inductiveautomation.perspective/views";
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let manifest = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;
    let target = |param: &str| {
        format!(
            r#"{{"params": {{"{param}": 1}}, "propConfig": {{"params.{param}": {{"paramDirection": "input"}}}}, "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#
        )
    };
    write(
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true}"#,
    );
    write(&format!("{views}/Target/resource.json"), manifest);
    write(&format!("{views}/Target/view.json"), &target("speed"));
    write(&format!("{views}/Caller/resource.json"), manifest);
    write(
        &format!("{views}/Caller/view.json"),
        r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [{"type": "ia.display.view", "meta": {"name": "Embedded"}, "props": {"path": "Target", "params": {"speed": 2}}}]}}"#,
    );
    let cache = root.join(".iglint_cache");
    let (first, _) = render(root, Some(cache.clone()));
    let (again, cached) = render(root, Some(cache.clone()));
    assert_eq!(first, again);
    assert!(cached > 0, "the caller is served from the cache");
    write(&format!("{views}/Target/view.json"), &target("rate"));
    let (with_cache, _) = render(root, Some(cache.clone()));
    let (without, _) = render(root, None);
    assert_eq!(with_cache, without);
    assert!(
        without
            .iter()
            .any(|d| d.contains("Caller") && d.contains("speed")),
        "{without:#?}"
    );
    // A change that keeps the target's summary keeps the caller cached.
    write(
        &format!("{views}/Target/view.json"),
        &format!("{}\n", target("rate")),
    );
    let (with_cache, cached) = render(root, Some(cache.clone()));
    assert_eq!(with_cache, render(root, None).0);
    assert!(cached > 0);
}
