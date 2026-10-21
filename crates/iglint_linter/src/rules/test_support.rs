//! Test harness for rules: hand-written projects in a temporary directory,
//! linted through the real engine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::diagnostic::Diagnostic;
use crate::engine::{CheckOptions, CheckResult, FixMode, check};
use crate::registry::RuleCode;
use crate::settings::{LinterSettings, OptionValue, ProjectSettings, RuleSettings};

pub const PERSPECTIVE: &str = "com.inductiveautomation.perspective";

/// A project on disk.
pub struct Fixture {
    pub dir: tempfile::TempDir,
}

impl Default for Fixture {
    fn default() -> Self {
        Fixture::new()
    }
}

impl Fixture {
    /// An empty workspace with one project `p`.
    pub fn new() -> Self {
        let f = Fixture {
            dir: tempfile::tempdir().expect("temp dir"),
        };
        f.file(
            "projects/p/project.json",
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        )
    }

    /// A workspace with project `p` holding one view `Main`.
    pub fn view(json: &str) -> Self {
        Fixture::new().with_view("Main", json)
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    /// Writes a file.
    pub fn file(self, rel: &str, text: &str) -> Self {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        self
    }

    /// Writes a resource directory `dir` (relative to the root) with its
    /// data files and a `resource.json` listing them.
    pub fn resource(self, dir: &str, files: &[(&str, &str)]) -> Self {
        let names: Vec<String> = files.iter().map(|(n, _)| format!("\"{n}\"")).collect();
        let manifest = format!(
            "{{\n  \"scope\": \"G\",\n  \"version\": 1,\n  \"restricted\": false,\n  \"overridable\": true,\n  \"files\": [{}],\n  \"attributes\": {{}}\n}}",
            names.join(", ")
        );
        let mut f = self.file(&format!("{dir}/resource.json"), &manifest);
        for (name, text) in files {
            f = f.file(&format!("{dir}/{name}"), text);
        }
        f
    }

    /// A view of project `p` at `path` (e.g. `Popups/Detail`).
    pub fn with_view(self, path: &str, json: &str) -> Self {
        self.resource(
            &format!("projects/p/{PERSPECTIVE}/views/{path}"),
            &[("view.json", json)],
        )
    }

    /// Path of the `view.json` of view `path` in project `p`.
    pub fn view_path(&self, path: &str) -> PathBuf {
        self.path(&format!("projects/p/{PERSPECTIVE}/views/{path}/view.json"))
    }

    /// Lints the whole fixture with exactly `rules` enabled (default
    /// options).
    pub fn lint(&self, rules: &[&str]) -> Vec<Diagnostic> {
        let rules: Vec<(&str, &str)> = rules.iter().map(|r| (*r, "")).collect();
        self.lint_with(&rules)
    }

    /// Lints with `(rule, options as TOML key/values)` pairs.
    pub fn lint_with(&self, rules: &[(&str, &str)]) -> Vec<Diagnostic> {
        self.run(rules, FixMode::None).diagnostics
    }

    pub fn run(&self, rules: &[(&str, &str)], fix_mode: FixMode) -> CheckResult {
        let settings = settings(self.root(), rules);
        let options = CheckOptions {
            fix_mode,
            ..CheckOptions::default()
        };
        let result = check(&settings, &[self.root().to_path_buf()], &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        result
    }

    /// The text of `rel` after applying the fixes of `rules` (without
    /// writing). Asserts that fixing is idempotent and that the fixed text
    /// no longer has fixable findings of those rules.
    pub fn fixed(&self, rules: &[(&str, &str)], unsafe_fixes: bool, rel: &Path) -> String {
        let result = self.run(rules, FixMode::DiffOnly { unsafe_fixes });
        let summary = result
            .fixed
            .iter()
            .find(|f| f.path == rel)
            .unwrap_or_else(|| panic!("no fixes for {}", rel.display()));
        let fixed = summary.fixed.clone();
        // Re-lint the fixed text: idempotent and clean.
        let again = Fixture {
            dir: tempfile::tempdir().expect("temp dir"),
        };
        copy_dir(self.root(), again.root());
        let rel_in_copy = again
            .root()
            .join(rel.strip_prefix(self.root()).expect("inside"));
        std::fs::write(&rel_in_copy, &fixed).expect("write");
        let second = again.run(rules, FixMode::DiffOnly { unsafe_fixes });
        if let Some(s) = second.fixed.iter().find(|f| f.path == rel_in_copy) {
            assert_eq!(s.fixed, fixed, "fixing is not idempotent");
        }
        let remaining: Vec<&Diagnostic> = second
            .diagnostics
            .iter()
            .filter(|d| d.file.path() == rel_in_copy && d.fix.is_some())
            .collect();
        assert!(
            remaining.is_empty(),
            "fixable findings remain after fixing: {remaining:?}"
        );
        fixed
    }
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).expect("read dir") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            std::fs::create_dir_all(&target).expect("mkdir");
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

/// Settings enabling exactly `rules` with options given as TOML.
pub fn settings(root: &Path, rules: &[(&str, &str)]) -> ProjectSettings {
    let mut map = BTreeMap::new();
    for (code, options) in rules {
        let rule: RuleCode = super::REGISTRY
            .find(code)
            .unwrap_or_else(|| panic!("unknown rule {code}"));
        map.insert(
            rule,
            RuleSettings {
                severity: rule.meta().default_severity,
                options: options_from_toml(options),
            },
        );
    }
    ProjectSettings {
        root: root.to_path_buf(),
        base: Arc::new(LinterSettings {
            rules: map,
            strict: false,
        }),
        respect_gitignore: false,
        ..ProjectSettings::default()
    }
}

/// Parses `key = value` lines into options.
pub fn options_from_toml(text: &str) -> BTreeMap<String, OptionValue> {
    let table: toml::Table = text.parse().expect("valid TOML options");
    table
        .into_iter()
        .map(|(k, v)| (k, crate::options::from_toml(v)))
        .collect()
}

/// `(code, message)` of each finding, sorted by position.
pub fn messages(diagnostics: &[Diagnostic]) -> Vec<(String, String)> {
    diagnostics
        .iter()
        .map(|d| (d.rule.code().to_owned(), d.message.clone()))
        .collect()
}

/// The text each finding points at.
pub fn flagged(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| d.range.slice(d.file.text()).to_owned())
        .collect()
}
