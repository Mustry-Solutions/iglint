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

    /// Writes a file with arbitrary bytes.
    pub fn bytes(self, rel: &str, bytes: &[u8]) -> Self {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, bytes).expect("write");
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

    /// Lints with `rules` enabled (default options) in strict mode.
    pub fn lint_strict(&self, rules: &[&str]) -> Vec<Diagnostic> {
        let rules: Vec<(&str, &str)> = rules.iter().map(|r| (*r, "")).collect();
        let mut settings = settings(self.root(), &rules);
        Arc::make_mut(&mut settings.base).strict = true;
        let result = check(
            &settings,
            &[self.root().to_path_buf()],
            &CheckOptions::default(),
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_unsafe_fixes_have_reasons(&result.diagnostics);
        result.diagnostics
    }

    /// Lints with `(rule, options as TOML key/values)` pairs.
    pub fn lint_with(&self, rules: &[(&str, &str)]) -> Vec<Diagnostic> {
        self.run(rules, FixMode::None).diagnostics
    }

    pub fn run(&self, rules: &[(&str, &str)], fix_mode: FixMode) -> CheckResult {
        self.run_assuming(rules, fix_mode, false)
    }

    /// Lints with `rules` (default options) when the project vets the
    /// names of `vetted` (`[fix] vetted-system-functions` and
    /// `vetted-expression-functions`).
    pub fn lint_vetting(
        &self,
        rules: &[&str],
        vetted: &crate::settings::Vetted,
    ) -> Vec<Diagnostic> {
        let rules: Vec<(&str, &str)> = rules.iter().map(|r| (*r, "")).collect();
        let mut settings = settings(self.root(), &rules);
        settings.vetted = vetted.clone();
        let result = check(
            &settings,
            &[self.root().to_path_buf()],
            &CheckOptions::default(),
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_unsafe_fixes_have_reasons(&result.diagnostics);
        result.diagnostics
    }

    /// [`Fixture::run`] with `[fix] assume-scripts-do-not-read-workspace-files`
    /// set to `assume_no_file_reads`.
    pub fn run_assuming(
        &self,
        rules: &[(&str, &str)],
        fix_mode: FixMode,
        assume_no_file_reads: bool,
    ) -> CheckResult {
        let mut settings = settings(self.root(), rules);
        settings.assume_no_workspace_file_reads = assume_no_file_reads;
        let options = CheckOptions {
            fix_mode,
            ..CheckOptions::default()
        };
        let result = check(&settings, &[self.root().to_path_buf()], &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_unsafe_fixes_have_reasons(&result.diagnostics);
        if fix_mode == FixMode::None {
            assert_safe_fixes_apply(&settings, self.root(), &result.diagnostics);
        }
        result
    }

    /// The text of `rel` after applying the fixes of `rules` (without
    /// writing). Asserts that fixing is idempotent and that the fixed text
    /// no longer has fixable findings of those rules.
    pub fn fixed(&self, rules: &[(&str, &str)], unsafe_fixes: bool, rel: &Path) -> String {
        self.fixed_assuming(rules, unsafe_fixes, rel, false)
    }

    /// [`Fixture::fixed`] with `[fix] assume-scripts-do-not-read-workspace-files`
    /// set to `assume_no_file_reads`.
    pub fn fixed_assuming(
        &self,
        rules: &[(&str, &str)],
        unsafe_fixes: bool,
        rel: &Path,
        assume_no_file_reads: bool,
    ) -> String {
        let result = self.run_assuming(
            rules,
            FixMode::DiffOnly { unsafe_fixes },
            assume_no_file_reads,
        );
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
        let second = again.run_assuming(
            rules,
            FixMode::DiffOnly { unsafe_fixes },
            assume_no_file_reads,
        );
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

/// Every unsafe fix says why it is unsafe (review round 14): the rule
/// tests lint thousands of fixtures through [`Fixture`], so this checks
/// every unsafe fix those fixtures produce.
pub fn assert_unsafe_fixes_have_reasons(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        if let Some(fix) = &d.fix
            && fix.applicability == crate::diagnostic::Applicability::Unsafe
        {
            assert!(
                fix.unsafe_reason
                    .as_ref()
                    .is_some_and(|r| !r.message.is_empty()),
                "{} unsafe fix without a reason: {} ({})",
                d.rule.code(),
                fix.message,
                d.message
            );
        }
    }
}

/// Every fix that `check` reports as safe is applied by `--fix` (review
/// round 18, N1; round 25, M1: a fix in a module that fails to compile
/// on its size either way was reported safe and refused as "making the
/// file worse"). The rule tests lint thousands of fixtures through
/// [`Fixture`], so this checks every safe fix those fixtures report: fixed
/// with safe fixes only, no fix may be dropped as harmful (by the health
/// check or the safety net).
fn assert_safe_fixes_apply(settings: &ProjectSettings, root: &Path, diagnostics: &[Diagnostic]) {
    let safe = |d: &&Diagnostic| {
        d.fix
            .as_ref()
            .is_some_and(|f| f.applicability == crate::diagnostic::Applicability::Safe)
    };
    if !diagnostics.iter().any(|d| safe(&d)) {
        return;
    }
    let options = CheckOptions {
        fix_mode: FixMode::DiffOnly {
            unsafe_fixes: false,
        },
        ..CheckOptions::default()
    };
    let result = check(settings, &[root.to_path_buf()], &options);
    assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
    for summary in &result.fixed {
        for skipped in summary.skipped.iter().filter(|s| s.harmful) {
            let reported: Vec<&Diagnostic> = diagnostics
                .iter()
                .filter(safe)
                .filter(|d| d.rule == skipped.rule)
                .collect();
            assert!(
                reported.is_empty(),
                "{} fix reported safe but not applied by --fix: {} ({})",
                skipped.rule.code(),
                skipped.message,
                skipped.reason
            );
        }
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
