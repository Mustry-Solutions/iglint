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
use std::path::Path;

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

impl Baseline {
    /// Builds a baseline from findings. Paths are made relative to the
    /// settings root.
    pub fn from_diagnostics<'a>(
        settings: &ProjectSettings,
        diagnostics: impl IntoIterator<Item = &'a Diagnostic>,
    ) -> Baseline {
        let mut counts: BTreeMap<(String, String, String), u32> = BTreeMap::new();
        for diagnostic in diagnostics {
            let path = settings.relative_path(diagnostic.file.path());
            let fingerprint = format_fingerprint(diagnostic.fingerprint(&path));
            *counts
                .entry((path, diagnostic.rule.code().to_owned(), fingerprint))
                .or_default() += 1;
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
    /// baseline, and reports entries that matched nothing.
    pub fn filter(
        &self,
        settings: &ProjectSettings,
        diagnostics: Vec<Diagnostic>,
    ) -> BaselineMatch {
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
            let path = settings.relative_path(diagnostic.file.path());
            let fingerprint = format_fingerprint(diagnostic.fingerprint(&path));
            let key = (path, diagnostic.rule.code().to_owned(), fingerprint);
            match remaining.get_mut(&key) {
                Some(count) if *count > 0 => {
                    *count -= 1;
                    result.suppressed += 1;
                }
                _ => result.new.push(diagnostic),
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

    fn settings() -> ProjectSettings {
        ProjectSettings {
            root: PathBuf::from("/repo"),
            ..ProjectSettings::default()
        }
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

    #[test]
    fn rejects_other_versions() {
        let error = Baseline::parse(r#"{"version": 99, "entries": []}"#).unwrap_err();
        assert_eq!(
            error.to_string(),
            "unsupported baseline version 99 (this iglint reads version 1)"
        );
    }
}
