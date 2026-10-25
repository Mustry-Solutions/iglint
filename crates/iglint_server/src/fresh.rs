//! Fixes computed by `iglint check` itself (review round 32).
//!
//! The server keeps an incremental [`LiveWorkspace`] per folder for what it
//! shows (diagnostics, hovers, the list of code actions). Rounds 29 to 32
//! each found another way for that state to drift from what a fresh check
//! reports (a file becoming a symbolic link to another one, an unsaved
//! manifest edit, ...), so that a stale safe fix was offered. Every edit the
//! server returns or applies for a fix is therefore computed here, by the
//! same run as `iglint check --fix` in the folder: the configuration and
//! the baseline loaded again, the workspace discovered again
//! ([`iglint_linter::engine::check`]), the editor's text of every open
//! document in place of the file on disk ([`CheckOptions::overlay`]), and
//! nothing taken from the live workspace. When an open document's editor
//! text differs from the disk and the run could not use it (a manifest,
//! which discovery reads, a configuration or baseline file, a file that is
//! not on disk), no fix is computed at all.
//!
//! [`LiveWorkspace`]: iglint_linter::live::LiveWorkspace

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iglint_linter::discovery::absolute;
use iglint_linter::engine::{self, CheckOptions, FixMode, Overlay};
use iglint_linter::{Applicability, Diagnostic, SourceFile};
use lsp_types::MessageType;
use rustc_hash::{FxHashMap, FxHasher};
use serde::{Deserialize, Serialize};

use crate::folder::{CONFIG_NAMES, Configured};
use crate::server::Server;

/// What identifies the finding of a quick fix between the list of code
/// actions and `codeAction/resolve`: where it is, what it says, its fix,
/// and the text it was found in.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingKey {
    pub rule: String,
    pub start: u32,
    pub end: u32,
    pub message: String,
    pub fix_message: String,
    pub safe: bool,
    /// [`text_hash`] of the file's text.
    pub text: String,
}

impl FindingKey {
    pub fn of(d: &Diagnostic) -> Option<FindingKey> {
        let fix = d.fix.as_ref()?;
        Some(FindingKey {
            rule: d.rule.code().to_owned(),
            start: d.range.start().to_usize() as u32,
            end: d.range.end().to_usize() as u32,
            message: d.message.clone(),
            fix_message: fix.message.clone(),
            safe: fix.applicability == Applicability::Safe,
            text: text_hash(d.file.text()),
        })
    }

    /// Whether `d` is this finding with this fix.
    pub fn matches(&self, d: &Diagnostic) -> bool {
        d.fix.as_ref().is_some_and(|fix| {
            d.rule.code() == self.rule
                && d.range.start().to_usize() == self.start as usize
                && d.range.end().to_usize() == self.end as usize
                && d.message == self.message
                && fix.message == self.fix_message
                && match fix.applicability {
                    Applicability::Safe => self.safe,
                    Applicability::Unsafe => !self.safe,
                    Applicability::DisplayOnly => false,
                }
        })
    }
}

/// A digest of a text (in a code action's data).
pub fn text_hash(text: &str) -> String {
    let mut h = FxHasher::default();
    text.hash(&mut h);
    text.len().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The fix a fresh run is asked for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FixWanted {
    /// Every fix of the file, as `--fix` (`--unsafe-fixes`) applies them.
    All { unsafe_fixes: bool },
    /// The fix of one finding, in one pass.
    One(FindingKey),
    /// The fix of every finding of the file, each on its own (the edits
    /// that go with a list of quick fixes).
    Each,
}

/// What a fresh run found for one file.
#[derive(Clone, Debug)]
pub struct Fresh {
    /// The findings a check reports for the file (after the baseline);
    /// `None` after "fix all" changed the text (the run reports what is
    /// left).
    pub diagnostics: Option<Vec<Diagnostic>>,
    /// The text the run linted and its fixed text, when a fix was asked
    /// for and changes it.
    pub fixed: Option<(SourceFile, String)>,
    /// With [`FixWanted::Each`]: the fixed text of each finding's fix.
    pub each: Vec<(FindingKey, SourceFile, String)>,
}

impl Fresh {
    /// The fixed text of the finding `key` ([`FixWanted::Each`]).
    pub fn fix_of(&self, key: &FindingKey) -> Option<(SourceFile, String)> {
        self.each
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, s, t)| (s.clone(), t.clone()))
    }
}

/// Fresh results kept for the requests of one burst, for clients that
/// take the edits with the list of code actions (no `codeAction/resolve`):
/// valid while no notification arrived, for at most [`BURST`], and while
/// the disk fingerprint of the folder is unchanged.
#[derive(Default)]
pub struct BurstCache {
    epoch: u64,
    entries: FxHashMap<(PathBuf, FixWanted), (Instant, u64, Option<Fresh>)>,
}

/// How long a burst of requests may reuse a fresh result.
const BURST: Duration = Duration::from_secs(1);

impl Server {
    /// A fresh run for `path` in the folder that publishes it, with the
    /// fix `wanted` (see the module docs). `None` when no fix may be
    /// computed now (the configuration or the baseline cannot be read, an
    /// open document's text cannot be used, the file cannot be linted).
    /// Diagnostics that differ from what is published are published again,
    /// and a live workspace that disagrees with the fresh run is
    /// discovered again.
    pub fn fresh(&mut self, path: &Path, wanted: &FixWanted) -> Option<Fresh> {
        let start = Instant::now();
        let path = absolute(path);
        let i = self.folder_for(&path);
        let root = self.folders[i].root.clone();
        let outcome = fresh_run(&root, &path, wanted, &self.open_texts());
        let elapsed = start.elapsed();
        let fresh = match outcome {
            Ok(fresh) => fresh,
            Err(reason) => {
                self.log(
                    MessageType::INFO,
                    format!("iglint: no fix for {}: {reason}", path.display()),
                );
                return None;
            }
        };
        self.log(
            MessageType::LOG,
            format!(
                "iglint: fresh check of {} in {:.1} ms",
                path.display(),
                elapsed.as_secs_f64() * 1000.0
            ),
        );
        if let Some(diagnostics) = &fresh.diagnostics {
            self.heal(i, &path, diagnostics);
        }
        Some(fresh)
    }

    /// [`Server::fresh`] reused within a burst of requests (clients
    /// without `codeAction/resolve`).
    pub fn fresh_in_burst(&mut self, path: &Path, wanted: &FixWanted) -> Option<Fresh> {
        let path = absolute(path);
        if self.burst.epoch != self.epoch {
            self.burst = BurstCache {
                epoch: self.epoch,
                ..BurstCache::default()
            };
        }
        let i = self.folder_for(&path);
        let fingerprint = self.folders[i].disk_fingerprint();
        let key = (path.clone(), wanted.clone());
        if let Some((at, seen, fresh)) = self.burst.entries.get(&key)
            && at.elapsed() < BURST
            && *seen == fingerprint
        {
            return fresh.clone();
        }
        let at = Instant::now();
        let fresh = self.fresh(&path, wanted);
        if self.burst.epoch == self.epoch {
            self.burst
                .entries
                .insert(key, (at, fingerprint, fresh.clone()));
        }
        fresh
    }

    /// The editor text of every open document, by absolute path.
    fn open_texts(&self) -> Vec<(PathBuf, Arc<str>)> {
        self.documents
            .values()
            .map(|d| (absolute(&d.path), Arc::from(d.text.as_str())))
            .chain(
                self.other_documents
                    .values()
                    .map(|(p, t)| (absolute(p), Arc::from(t.as_str()))),
            )
            .collect()
    }

    /// After a fresh run of `path` in folder `i`: when the live workspace
    /// reports other findings for the file, it is discovered again (what
    /// it keeps drifted from the disk); when the published findings
    /// differ, the fresh ones are published.
    fn heal(&mut self, i: usize, path: &Path, fresh: &[Diagnostic]) {
        self.sync_documents();
        let live = self.folders[i]
            .live
            .check(std::slice::from_ref(&path.to_path_buf()))
            .pop()
            .map(|r| self.folders[i].filter_baseline(r.diagnostics))
            .unwrap_or_default();
        if !same_set(&live, fresh) {
            self.log(
                MessageType::INFO,
                format!(
                    "iglint: {} differs from a fresh check; discovering the folder again",
                    path.display()
                ),
            );
            self.reload_folder(i);
        }
        let published = self
            .results
            .get(path)
            .is_some_and(|p| same_set(&p.diagnostics, fresh));
        let shown =
            self.results.contains_key(path) || self.documents.values().any(|d| d.path == path);
        if !published && shown {
            self.publish(path, fresh.to_vec());
        }
    }
}

/// Whether two lists hold the same findings, in any order.
pub fn same_set(a: &[Diagnostic], b: &[Diagnostic]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut used = vec![false; b.len()];
    a.iter().all(|d| {
        match (0..b.len()).find(|k| !used[*k] && iglint_linter::live::same_finding(d, &b[*k])) {
            Some(k) => {
                used[k] = true;
                true
            }
            None => false,
        }
    })
}

/// The run of [`Server::fresh`]: `iglint check` (`--fix`) of the folder
/// `root`, linting only `path`, with the open documents' texts `open`.
fn fresh_run(
    root: &Path,
    path: &Path,
    wanted: &FixWanted,
    open: &[(PathBuf, Arc<str>)],
) -> Result<Fresh, String> {
    let configured = Configured::load(root, false);
    if let Some(error) = &configured.error {
        return Err(error.clone());
    }
    if let Err(error) = &configured.baseline {
        return Err(error.clone());
    }
    if configured.baseline_required
        && configured
            .baseline_path
            .as_ref()
            .is_none_or(|p| !p.is_file())
    {
        return Err("the configured baseline does not exist".to_owned());
    }
    let overlay: Arc<Overlay> = Arc::new(open.iter().cloned().collect());
    let (fix_mode, fix_each, select): (FixMode, bool, Option<engine::FixSelect>) = match wanted {
        FixWanted::All { unsafe_fixes } => (
            FixMode::DiffOnly {
                unsafe_fixes: *unsafe_fixes,
            },
            false,
            None,
        ),
        FixWanted::One(key) => {
            let key = key.clone();
            let at = path.to_path_buf();
            let mode = FixMode::DiffOnly {
                unsafe_fixes: !key.safe,
            };
            let select: engine::FixSelect =
                Arc::new(move |d: &Diagnostic| absolute(d.file.path()) == at && key.matches(d));
            (mode, true, Some(select))
        }
        FixWanted::Each => (FixMode::DiffOnly { unsafe_fixes: true }, true, None),
    };
    let options = CheckOptions {
        fix_mode,
        fix_each,
        fix_select: select,
        overlay: Some(overlay),
        only: Some(vec![path.to_path_buf()]),
        ..CheckOptions::default()
    };
    let mut result = engine::check(&configured.settings, &configured.roots, &options);
    let linted = |r: &engine::CheckResult| {
        r.files.iter().any(|f| absolute(&f.path) == path)
            || r.skipped.iter().any(|(p, _)| absolute(p) == path)
            || r.errors.iter().any(|e| absolute(&e.path) == path)
    };
    if !linted(&result) {
        // A file a check of the folder does not find (a Python file
        // outside every project): `iglint check <file>`.
        result = engine::check(
            &configured.settings,
            std::slice::from_ref(&path.to_path_buf()),
            &options,
        );
    }
    if let Some(e) = result.errors.iter().find(|e| absolute(&e.path) == path) {
        return Err(e.message.clone());
    }
    if let Some(e) = result.errors.first() {
        // Invalid rule options: `iglint check` fails.
        return Err(e.to_string());
    }
    unusable_document(&configured, &result, root, open)?;
    let reported: Vec<Diagnostic> = configured.filter_baseline(
        result
            .diagnostics
            .iter()
            .filter(|d| absolute(d.file.path()) == path)
            .cloned()
            .collect(),
    );
    // A fix is made only for a finding the check reports (after the
    // baseline), in the text it reports it in.
    let reported_fix = |d: &Diagnostic| {
        reported
            .iter()
            .any(|r| iglint_linter::live::same_finding(r, d))
    };
    let singles: Vec<(FindingKey, SourceFile, String)> = result
        .single_fixes
        .iter()
        .filter(|f| absolute(f.diagnostic.file.path()) == path && reported_fix(&f.diagnostic))
        .filter_map(|f| {
            let key = FindingKey::of(&f.diagnostic)?;
            let fixed = f.fixed.clone()?;
            Some((key, SourceFile::new(path, Arc::clone(&f.original)), fixed))
        })
        .collect();
    let fresh = match wanted {
        FixWanted::All { .. } => {
            let fixed = result
                .fixed
                .into_iter()
                .find(|f| absolute(&f.path) == path && f.changed())
                .map(|summary| {
                    (
                        SourceFile::new(path, Arc::clone(&summary.original)),
                        summary.fixed,
                    )
                });
            Fresh {
                // Without a change, what the run reports is what a check
                // reports.
                diagnostics: fixed.is_none().then_some(reported),
                fixed,
                each: Vec::new(),
            }
        }
        FixWanted::One(key) => Fresh {
            diagnostics: Some(reported),
            fixed: singles
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, s, t)| (s.clone(), t.clone())),
            each: Vec::new(),
        },
        FixWanted::Each => Fresh {
            diagnostics: Some(reported),
            fixed: None,
            each: singles,
        },
    };
    Ok(fresh)
}

/// Why the run cannot stand for `iglint check` of the editor's texts, if
/// it cannot: an open document whose editor text differs from the disk
/// and that the run read from the disk, or would have read had it been on
/// disk (a manifest, a configuration, baseline or whitelist file, a file
/// of the workspace that is not on disk).
fn unusable_document(
    configured: &Configured,
    result: &engine::CheckResult,
    root: &Path,
    open: &[(PathBuf, Arc<str>)],
) -> Result<(), String> {
    let roots: Vec<PathBuf> = result
        .workspace_roots
        .iter()
        .chain(&configured.roots)
        .map(|r| absolute(r))
        .chain(std::iter::once(absolute(root)))
        .collect();
    for (path, text) in open {
        if result.overlay_applied.binary_search(path).is_ok() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let relevant = roots.iter().any(|r| path.starts_with(r))
            || CONFIG_NAMES.contains(&name)
            || name == iglint_linter::baseline::DEFAULT_BASELINE_FILE
            || name == ".gitignore"
            || matches!(extension, "toml" | "txt")
            || configured.config.as_deref() == Some(path.as_path())
            || configured.baseline_path.as_deref() == Some(path.as_path());
        if !relevant {
            continue;
        }
        let same = std::fs::read(path).is_ok_and(|bytes| bytes == text.as_bytes());
        if !same {
            return Err(format!(
                "the editor text of {} differs from the file on disk, and the check reads the disk",
                path.display()
            ));
        }
    }
    Ok(())
}
