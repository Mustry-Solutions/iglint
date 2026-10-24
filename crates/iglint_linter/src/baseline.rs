//! The baseline: known findings that do not fail a run.
//!
//! `.iglint-baseline.json` lists findings by fingerprint (see
//! [`Diagnostic::fingerprint`]), rule and path. Identical findings in one file
//! share an entry with a `count`. Entries are sorted so the file diffs well.
//!
//! ```json
//! {
//!   "version": 1,
//!   "entries": [
//!     { "fingerprint": "9f0c4a1be2d3c8a1", "rule": "SCR101", "path": "projects/a/x.py", "count": 1 }
//!   ]
//! }
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
use crate::settings::ProjectSettings;

/// Version of the file format written by this build.
pub const BASELINE_VERSION: u32 = 1;

/// Default file name, relative to the project root.
pub const DEFAULT_BASELINE_FILE: &str = ".iglint-baseline.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub version: u32,
    pub entries: Vec<BaselineEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BaselineEntry {
    /// `path` comes first so that sorting groups entries by file.
    pub path: String,
    pub rule: String,
    pub fingerprint: String,
    pub count: u32,
}

#[derive(Debug)]
pub enum BaselineError {
    Io(std::io::Error),
    Parse(String),
    UnsupportedVersion(u32),
}

impl fmt::Display for BaselineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BaselineError::Io(error) => error.fmt(f),
            BaselineError::Parse(message) => write!(f, "invalid baseline file: {message}"),
            BaselineError::UnsupportedVersion(version) => write!(
                f,
                "unsupported baseline version {version} (this iglint reads version {BASELINE_VERSION})"
            ),
        }
    }
}

impl std::error::Error for BaselineError {}

/// The result of filtering diagnostics through a baseline.
#[derive(Debug, Default)]
pub struct BaselineMatch {
    /// Findings not covered by the baseline.
    pub new: Vec<Diagnostic>,
    /// Number of findings the baseline covered.
    pub suppressed: usize,
    /// Entries (or the unused part of their count) that matched nothing:
    /// the finding was fixed.
    pub stale: Vec<BaselineEntry>,
}

fn format_fingerprint(value: u64) -> String {
    format!("{value:016x}")
}

/// The directory that baseline paths (and the fingerprints computed from
/// them) are relative to.
///
/// With a configuration file it is the configuration file's directory
/// (the settings root). Without one, the settings root is the working
/// directory, which would make the same finding a different entry from
/// every directory iglint is run in; the origin is then the workspace
/// root of the run ([`crate::discovery::workspace_roots`]), which does
/// not depend on the working directory or on what is linted, when the
/// run has exactly one. Baselines written by earlier builds from another
/// directory have paths relative to the working directory of that run:
/// an entry that does not match relative to the origin is matched
/// relative to the settings root too ([`BaselineOrigin::legacy`]), and
/// rewriting the baseline stores it relative to the origin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaselineOrigin {
    root: PathBuf,
    /// The settings root when it differs from `root`.
    legacy: Option<PathBuf>,
}

impl BaselineOrigin {
    /// The origin of a run over `paths` with `settings`.
    pub fn new(settings: &ProjectSettings, paths: &[PathBuf]) -> BaselineOrigin {
        let settings_root = crate::discovery::absolute(&settings.root);
        if settings.root_is_configured || settings.root.as_os_str().is_empty() {
            return BaselineOrigin::at(settings_root);
        }
        let paths: Vec<PathBuf> = if paths.is_empty() {
            vec![settings_root.clone()]
        } else {
            paths.to_vec()
        };
        let roots = crate::discovery::workspace_roots(settings, &paths);
        match roots.roots.as_slice() {
            [root] if roots.unknown.is_empty() && *root != settings_root => BaselineOrigin {
                root: root.clone(),
                legacy: Some(settings_root),
            },
            _ => BaselineOrigin::at(settings_root),
        }
    }

    /// Paths relative to `root`.
    pub fn at(root: PathBuf) -> BaselineOrigin {
        BaselineOrigin { root, legacy: None }
    }

    /// The directory paths are relative to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The settings root that baselines of earlier builds may be relative
    /// to, when it is not [`BaselineOrigin::root`].
    pub fn legacy(&self) -> Option<&Path> {
        self.legacy.as_deref()
    }

    /// `file` relative to the origin, `/`-separated.
    pub fn relative_path(&self, file: &Path) -> String {
        relative_to(&self.root, file)
    }

    /// `file` relative to the legacy root, when there is one.
    pub fn legacy_path(&self, file: &Path) -> Option<String> {
        self.legacy.as_deref().map(|root| relative_to(root, file))
    }

    /// Whether the file of a baseline entry is gone: it exists neither
    /// relative to the origin nor relative to the legacy root.
    pub fn is_gone(&self, path: &str) -> bool {
        !self.root.join(path).exists()
            && self
                .legacy
                .as_deref()
                .is_none_or(|root| !root.join(path).exists())
    }

    /// The baseline file used when none is configured or given:
    /// `.iglint-baseline.json` in the origin, or in the legacy root when
    /// only that one exists (a baseline written by an earlier build).
    pub fn default_file(&self) -> PathBuf {
        let at_origin = self.root.join(DEFAULT_BASELINE_FILE);
        match &self.legacy {
            Some(root) if !at_origin.is_file() && root.join(DEFAULT_BASELINE_FILE).is_file() => {
                root.join(DEFAULT_BASELINE_FILE)
            }
            _ => at_origin,
        }
    }
}

fn relative_to(root: &Path, file: &Path) -> String {
    let file = crate::discovery::absolute(file);
    crate::discovery::slash_path(file.strip_prefix(root).unwrap_or(&file))
}

/// The `(path, rule, fingerprint)` key of a finding with `path`.
fn entry_key(diagnostic: &Diagnostic, path: String) -> (String, String, String) {
    let fingerprint = format_fingerprint(diagnostic.fingerprint(&path));
    (path, diagnostic.rule.code().to_owned(), fingerprint)
}

impl Baseline {
    /// Builds a baseline from findings. Paths are made relative to the
    /// origin.
    pub fn from_diagnostics<'a>(
        origin: &BaselineOrigin,
        diagnostics: impl IntoIterator<Item = &'a Diagnostic>,
    ) -> Baseline {
        let mut counts: BTreeMap<(String, String, String), u32> = BTreeMap::new();
        for diagnostic in diagnostics {
            let path = origin.relative_path(diagnostic.file.path());
            *counts.entry(entry_key(diagnostic, path)).or_default() += 1;
        }
        Baseline {
            version: BASELINE_VERSION,
            entries: counts
                .into_iter()
                .map(|((path, rule, fingerprint), count)| BaselineEntry {
                    path,
                    rule,
                    fingerprint,
                    count,
                })
                .collect(),
        }
    }

    pub fn parse(text: &str) -> Result<Baseline, BaselineError> {
        #[derive(Deserialize)]
        struct Version {
            version: u32,
        }
        let version: Version =
            serde_json::from_str(text).map_err(|e| BaselineError::Parse(e.to_string()))?;
        if version.version != BASELINE_VERSION {
            return Err(BaselineError::UnsupportedVersion(version.version));
        }
        let mut baseline: Baseline =
            serde_json::from_str(text).map_err(|e| BaselineError::Parse(e.to_string()))?;
        baseline.entries.sort();
        Ok(baseline)
    }

    pub fn load(path: &Path) -> Result<Baseline, BaselineError> {
        let text = std::fs::read_to_string(path).map_err(BaselineError::Io)?;
        Baseline::parse(&text)
    }

    /// The JSON text, with a final newline.
    pub fn to_json(&self) -> String {
        let mut sorted = self.clone();
        sorted.entries.sort();
        let mut text = serde_json::to_string_pretty(&SerializedBaseline::from(&sorted))
            .expect("baseline serialises");
        text.push('\n');
        text
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        crate::engine::write_atomically(path, &self.to_json())
    }

    pub fn len(&self) -> usize {
        self.entries.iter().map(|e| e.count as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Splits `diagnostics` into new findings and findings covered by the
    /// baseline, and reports entries that matched nothing. A finding is
    /// matched with its path relative to the origin, else (a baseline of
    /// an earlier build) relative to the legacy root.
    pub fn filter(&self, origin: &BaselineOrigin, diagnostics: Vec<Diagnostic>) -> BaselineMatch {
        let mut remaining: BTreeMap<(String, String, String), u32> = self
            .entries
            .iter()
            .map(|e| {
                (
                    (e.path.clone(), e.rule.clone(), e.fingerprint.clone()),
                    e.count,
                )
            })
            .collect();
        let mut result = BaselineMatch::default();
        for diagnostic in diagnostics {
            let keys = std::iter::once(origin.relative_path(diagnostic.file.path()))
                .chain(origin.legacy_path(diagnostic.file.path()))
                .map(|path| entry_key(&diagnostic, path));
            let mut covered = false;
            for key in keys {
                if let Some(count) = remaining.get_mut(&key).filter(|c| **c > 0) {
                    *count -= 1;
                    covered = true;
                    break;
                }
            }
            if covered {
                result.suppressed += 1;
            } else {
                result.new.push(diagnostic);
            }
        }
        result.stale = remaining
            .into_iter()
            .filter(|(_, count)| *count > 0)
            .map(|((path, rule, fingerprint), count)| BaselineEntry {
                path,
                rule,
                fingerprint,
                count,
            })
            .collect();
        result
    }

    /// `self` plus the entries of `previous` whose path is not
    /// `in_scope`: writing a baseline from a run over some files keeps the
    /// entries of the other files.
    pub fn with_entries_outside(
        mut self,
        previous: &Baseline,
        in_scope: impl Fn(&str) -> bool,
    ) -> Baseline {
        self.entries.extend(
            previous
                .entries
                .iter()
                .filter(|e| !in_scope(&e.path))
                .cloned(),
        );
        self.entries.sort();
        self
    }

    /// The baseline without the stale part of each entry.
    pub fn pruned(&self, stale: &[BaselineEntry]) -> Baseline {
        let mut entries = self.entries.clone();
        for gone in stale {
            if let Some(entry) = entries.iter_mut().find(|e| {
                e.path == gone.path && e.rule == gone.rule && e.fingerprint == gone.fingerprint
            }) {
                entry.count = entry.count.saturating_sub(gone.count);
            }
        }
        entries.retain(|e| e.count > 0);
        Baseline {
            version: BASELINE_VERSION,
            entries,
        }
    }
}

/// Field order in the file: fingerprint, rule, path, count.
#[derive(Serialize)]
struct SerializedBaseline<'a> {
    version: u32,
    entries: Vec<SerializedEntry<'a>>,
}

#[derive(Serialize)]
struct SerializedEntry<'a> {
    fingerprint: &'a str,
    rule: &'a str,
    path: &'a str,
    count: u32,
}

impl<'a> From<&'a Baseline> for SerializedBaseline<'a> {
    fn from(baseline: &'a Baseline) -> Self {
        SerializedBaseline {
            version: baseline.version,
            entries: baseline
                .entries
                .iter()
                .map(|e| SerializedEntry {
                    fingerprint: &e.fingerprint,
                    rule: &e.rule,
                    path: &e.path,
                    count: e.count,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use iglint_text::{TextRange, TextSize};

    use super::*;
    use crate::diagnostic::{Severity, SourceFile};
    use crate::registry::{Category, RuleCode, RuleMeta};

    static RULE: RuleMeta = RuleMeta {
        code: "TST001",
        name: "test-rule",
        category: Category::Script,
        ..RuleMeta::DEFAULT
    };

    fn settings() -> BaselineOrigin {
        BaselineOrigin::at(PathBuf::from("/repo"))
    }

    fn diagnostic(file: &SourceFile, start: u32, end: u32) -> Diagnostic {
        Diagnostic::new(
            RuleCode::new(&RULE),
            Severity::Error,
            "bad thing",
            file.clone(),
            TextRange::new(TextSize::new(start), TextSize::new(end)),
        )
    }

    #[test]
    fn entries_outside_the_scope_are_kept() {
        let file = SourceFile::new("/repo/a/x.py", "foo\n");
        let settings = settings();
        let previous = Baseline::from_diagnostics(
            &settings,
            &[
                diagnostic(&file, 0, 3),
                diagnostic(&SourceFile::new("/repo/b/y.py", "bar\n"), 0, 3),
            ],
        );
        let rewritten = Baseline::from_diagnostics(&settings, &[])
            .with_entries_outside(&previous, |path| path == "a/x.py");
        let paths: Vec<&str> = rewritten.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["b/y.py"]);
    }

    #[test]
    fn identical_findings_share_an_entry_and_match_by_count() {
        let file = SourceFile::new("/repo/a/x.py", "foo\nfoo\nbar\n");
        let settings = settings();
        let diagnostics = vec![diagnostic(&file, 0, 3), diagnostic(&file, 4, 7)];
        let baseline = Baseline::from_diagnostics(&settings, &diagnostics);
        assert_eq!(baseline.entries.len(), 1);
        assert_eq!(baseline.entries[0].count, 2);
        assert_eq!(baseline.entries[0].path, "a/x.py");

        // One of the two was fixed, a new one appeared.
        let now = vec![diagnostic(&file, 0, 3), diagnostic(&file, 8, 11)];
        let matched = baseline.filter(&settings, now);
        assert_eq!(matched.suppressed, 1);
        assert_eq!(matched.new.len(), 1);
        assert_eq!(matched.stale.len(), 1);
        assert_eq!(matched.stale[0].count, 1);
        let pruned = baseline.pruned(&matched.stale);
        assert_eq!(pruned.entries[0].count, 1);
    }

    #[test]
    fn fingerprints_survive_line_moves() {
        let settings = settings();
        let before = SourceFile::new("/repo/x.py", "foo\n");
        let after = SourceFile::new("/repo/x.py", "\n\nfoo\n");
        let baseline = Baseline::from_diagnostics(&settings, &[diagnostic(&before, 0, 3)]);
        let matched = baseline.filter(&settings, vec![diagnostic(&after, 2, 5)]);
        assert_eq!(matched.suppressed, 1);
        assert!(matched.stale.is_empty());
    }

    #[test]
    fn round_trip_is_sorted_and_stable() {
        let settings = settings();
        let a = SourceFile::new("/repo/b.py", "x");
        let b = SourceFile::new("/repo/a.py", "y");
        let baseline =
            Baseline::from_diagnostics(&settings, &[diagnostic(&a, 0, 1), diagnostic(&b, 0, 1)]);
        let json = baseline.to_json();
        assert!(json.find("a.py").unwrap() < json.find("b.py").unwrap());
        assert!(json.contains("\"fingerprint\""));
        assert!(json.ends_with("}\n"));
        let first_entry = json.find("\"fingerprint\"").unwrap();
        assert!(first_entry < json.find("\"rule\"").unwrap());
        assert_eq!(Baseline::parse(&json).unwrap(), baseline);
    }

    /// Reviewer round 16 (Codex C2): a baseline of an earlier build, with
    /// paths relative to the directory it was written from, still matches
    /// through the legacy root, and the gone check looks there too.
    #[test]
    fn entries_of_earlier_builds_match_relative_to_the_legacy_root() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join("projects/A")).unwrap();
        std::fs::write(repo.join("projects/A/x.py"), "foo\n").unwrap();
        let file = SourceFile::new(repo.join("projects/A/x.py"), "foo\n");
        let old = Baseline::from_diagnostics(
            &BaselineOrigin::at(repo.join("projects/A")),
            &[diagnostic(&file, 0, 3)],
        );
        assert_eq!(old.entries[0].path, "x.py");
        let origin = BaselineOrigin {
            root: repo.clone(),
            legacy: Some(repo.join("projects/A")),
        };
        let matched = old.filter(&origin, vec![diagnostic(&file, 0, 3)]);
        assert_eq!(matched.suppressed, 1);
        assert!(matched.stale.is_empty());
        assert!(!origin.is_gone("x.py"));
        assert!(!origin.is_gone("projects/A/x.py"));
        assert!(origin.is_gone("projects/A/y.py"));
        // Rewritten, the entry is relative to the origin.
        let new = Baseline::from_diagnostics(&origin, &[diagnostic(&file, 0, 3)]);
        assert_eq!(new.entries[0].path, "projects/A/x.py");
        assert_eq!(
            new.filter(&origin, vec![diagnostic(&file, 0, 3)])
                .suppressed,
            1
        );
    }

    #[test]
    fn rejects_other_versions() {
        let error = Baseline::parse(r#"{"version": 99, "entries": []}"#).unwrap_err();
        assert_eq!(
            error.to_string(),
            "unsupported baseline version 99 (this iglint reads version 1)"
        );
    }
}
