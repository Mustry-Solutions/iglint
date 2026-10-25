//! Fixes computed by `iglint check` itself (review rounds 32 and 33).
//!
//! The server keeps an incremental [`LiveWorkspace`] per folder for what it
//! shows (diagnostics, hovers, the list of code actions). Rounds 29 to 32
//! each found another way for that state to drift from what a fresh check
//! reports, so every edit the server returns or applies for a fix is
//! computed here, by the same run as `iglint check --fix` in the folder:
//! the configuration and the baseline loaded again, the workspace
//! discovered again ([`iglint_linter::engine::check`]), and nothing taken
//! from the live workspace.
//!
//! The run reads the disk, with one exception: the editor's text of the
//! document the fix is for (the target) replaces that file, by path and
//! by file identity, as saving the document would write it
//! ([`CheckOptions::overlay`]). A fix is computed only when every other
//! open document has its file's text on disk (round 33, B1: deciding by
//! path which of them the run reads missed links and configuration files
//! outside the folder), so the run is `iglint check --fix` on the disk
//! after saving the target. Otherwise, and when the target is open under
//! several names with different texts, or its editor text differs from
//! the disk and the run could not use it (a manifest, which discovery
//! reads, a configuration or baseline file, a file that is not on disk),
//! or its file starts with a byte order mark or is not valid UTF-8 (round
//! 34: the editor shows another text than iglint reads), no fix is
//! computed ([`Withheld`]).
//!
//! [`LiveWorkspace`]: iglint_linter::live::LiveWorkspace

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use iglint_linter::discovery::absolute;
use iglint_linter::engine::{self, CheckOptions, FixMode};
use iglint_linter::{Applicability, Diagnostic, SourceFile};
use lsp_types::MessageType;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};

use crate::folder::Configured;
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
    /// The fix of each of these findings, each on its own (the edits that
    /// go with a list of quick fixes).
    Keys(Vec<FindingKey>),
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
    /// With [`FixWanted::Keys`]: the fixed text of each finding's fix.
    pub each: Vec<(FindingKey, SourceFile, String)>,
}

impl Fresh {
    /// The fixed text of the finding `key` ([`FixWanted::Keys`]).
    pub fn fix_of(&self, key: &FindingKey) -> Option<(SourceFile, String)> {
        self.each
            .iter()
            .find(|(k, _, _)| k == key)
            .map(|(_, s, t)| (s.clone(), t.clone()))
    }
}

impl Server {
    /// A fresh run for `path` in the folder that publishes it, with the
    /// fix `wanted` (see the module docs). An error, with the reason for
    /// the user, when no fix may be computed now (the configuration or
    /// the baseline cannot be read, an open document's text cannot be
    /// used, the file cannot be linted). Diagnostics that differ from what
    /// is published are published again, and a live workspace that
    /// disagrees with the fresh run is discovered again.
    pub fn fresh(&mut self, path: &Path, wanted: &FixWanted) -> Result<Fresh, &'static str> {
        let start = Instant::now();
        let path = absolute(path);
        let target = match self.withheld(&path) {
            Ok(target) => target,
            Err(withheld) => {
                self.log(
                    MessageType::INFO,
                    format!("iglint: no fix for {}: {}", path.display(), withheld.detail),
                );
                return Err(withheld.reason);
            }
        };
        let i = self.folder_for(&path);
        let root = self.folders[i].root.clone();
        let outcome = fresh_run(&root, &path, wanted, target);
        let elapsed = start.elapsed();
        let fresh = match outcome {
            Ok(fresh) => fresh,
            Err(reason) => {
                self.log(
                    MessageType::INFO,
                    format!("iglint: no fix for {}: {reason}", path.display()),
                );
                return Err(NO_FIX_CHECK);
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
        Ok(fresh)
    }

    /// Whether a fix of `path` may be computed now: when every open
    /// document other than `path` has its file's text on disk, the editor
    /// text of `path` if it is open (the same in every document that is
    /// that file); otherwise why not.
    pub fn withheld(&self, path: &Path) -> Result<Option<Arc<str>>, Withheld> {
        let path = absolute(path);
        let identity = file_identity(&path);
        let mut target: Option<&str> = None;
        let open = self
            .documents
            .values()
            .map(|d| (&d.path, d.text.as_str()))
            .chain(self.other_documents.values().map(|(p, t)| (p, t.as_str())));
        for (at, text) in open {
            let at = absolute(at);
            let alias = at == path || identity.is_some() && file_identity(&at) == identity;
            if alias {
                match target {
                    Some(seen) if seen != text => {
                        return Err(Withheld {
                            reason: WITHHELD_ALIASES,
                            detail: format!(
                                "{} is open as {} with another text",
                                path.display(),
                                at.display()
                            ),
                        });
                    }
                    _ => target = Some(text),
                }
            } else {
                match on_disk(&at, text) {
                    OnDisk::Same => {}
                    OnDisk::Different => {
                        return Err(Withheld {
                            reason: WITHHELD_OTHERS,
                            detail: format!(
                                "the editor text of {} differs from the file on disk",
                                at.display()
                            ),
                        });
                    }
                    OnDisk::NotUtf8 => {
                        return Err(Withheld {
                            reason: WITHHELD_OTHER_NOT_UTF8,
                            detail: format!(
                                "{} is not valid UTF-8 on disk, so its editor text cannot be compared with the file",
                                at.display()
                            ),
                        });
                    }
                }
            }
        }
        // The target itself (review round 34, Opus minor): an editor hides
        // a leading byte order mark (iglint and Ignition do not: a script
        // starting with one does not compile) and writes it back on save,
        // and it shows a file that is not valid UTF-8 in some encoding
        // iglint cannot know; edits computed on what iglint reads would
        // not fit the editor's text, and an overlay of the editor's text
        // would not be what saving writes. No fix for such a file.
        if let Ok(bytes) = std::fs::read(&path) {
            if bytes.starts_with(BOM) || target.is_some_and(|t| t.starts_with('\u{feff}')) {
                return Err(Withheld {
                    reason: WITHHELD_BOM,
                    detail: format!("{} starts with a byte order mark", path.display()),
                });
            }
            if std::str::from_utf8(&bytes).is_err() {
                return Err(Withheld {
                    reason: WITHHELD_NOT_UTF8,
                    detail: format!("{} is not valid UTF-8 on disk", path.display()),
                });
            }
        }
        Ok(target.map(Arc::from))
    }

    /// After a fresh run of `path` in folder `i`: when the live workspace
    /// reports other findings for the file, it is discovered again (what
    /// it keeps drifted from the disk); when the published findings
    /// differ, the fresh ones are published.
    fn heal(&mut self, i: usize, path: &Path, fresh: &[Diagnostic]) {
        // The live workspace does not run pylint (SCR900): what it shows
        // is compared, and shown, without those findings (review round
        // 33, Codex 3).
        let pylint = iglint_linter::rules::script::pylint::META.code;
        let fresh: Vec<Diagnostic> = fresh
            .iter()
            .filter(|d| d.rule.code() != pylint)
            .cloned()
            .collect();
        let fresh = fresh.as_slice();
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

/// Why no fix is computed now ([`Server::withheld`]).
#[derive(Clone, Debug)]
pub struct Withheld {
    /// For the user (a disabled code action's reason).
    pub reason: &'static str,
    /// For the log.
    pub detail: String,
}

/// Why a fix gives no edit: the check that computes it failed.
pub const NO_FIX_CHECK: &str =
    "iglint could not check the file to compute the fix; see the iglint output";
/// Why a fix gives no edit: the check no longer makes it (the file or the
/// workspace changed since the fixes were listed).
pub const NO_FIX_STALE: &str = "This fix no longer applies; the file or the workspace changed";
pub const WITHHELD_OTHERS: &str = "Save the other modified files to apply iglint fixes";
pub const WITHHELD_OTHER_NOT_UTF8: &str =
    "Another open file is not valid UTF-8; save it as UTF-8 or close it to apply iglint fixes";
pub const WITHHELD_BOM: &str = "This file starts with a byte order mark, which the editor hides; save it as UTF-8 without BOM to apply iglint fixes";
pub const WITHHELD_NOT_UTF8: &str =
    "This file is not valid UTF-8; save it as UTF-8 to apply iglint fixes";

/// The UTF-8 byte order mark.
pub const BOM: &[u8] = b"\xEF\xBB\xBF";

/// How an open document's editor text compares with its file on disk.
#[derive(Debug, PartialEq, Eq)]
enum OnDisk {
    /// Saving the document would write the file's bytes again.
    Same,
    /// The document has unsaved changes (or the file is missing).
    Different,
    /// The file is not valid UTF-8 and the editor's text is not what
    /// iglint reads from it: the editor decoded it its own way, and
    /// whether it has unsaved changes cannot be told.
    NotUtf8,
}

/// Compares the editor text of an open document with its file: the same
/// when the bytes are the text, also after a leading byte order mark that
/// the editor hides (VS Code leaves it out of the text and writes it back
/// on save), or when the bytes are not valid UTF-8 and decode to the text
/// as iglint reads them ([`iglint_text::decode_utf8`]).
fn on_disk(path: &Path, text: &str) -> OnDisk {
    let Ok(bytes) = std::fs::read(path) else {
        return OnDisk::Different;
    };
    if bytes == text.as_bytes() {
        return OnDisk::Same;
    }
    let body = match bytes.strip_prefix(BOM) {
        Some(rest) if !text.starts_with('\u{feff}') => rest,
        _ => &bytes[..],
    };
    if body == text.as_bytes() {
        return OnDisk::Same;
    }
    if std::str::from_utf8(body).is_ok() {
        return OnDisk::Different;
    }
    if iglint_text::decode_utf8(body.to_vec()).0 == text {
        OnDisk::Same
    } else {
        OnDisk::NotUtf8
    }
}

/// The text saving `text` to `path` gives the file as iglint reads it: a
/// byte order mark the file starts with is put back when the editor's
/// text lacks it (for what is shown; no fix is computed for such a file).
pub fn as_saved(path: &Path, text: &str) -> Arc<str> {
    use std::io::Read;
    if text.starts_with('\u{feff}') {
        return Arc::from(text);
    }
    let mut start = [0u8; 3];
    let bom = std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut start))
        .is_ok()
        && start == BOM;
    if bom {
        Arc::from(format!("\u{feff}{text}"))
    } else {
        Arc::from(text)
    }
}
pub const WITHHELD_ALIASES: &str = "This file is open more than once with different texts; save or close one to apply iglint fixes";

/// What identifies a file on disk through links: device and inode on
/// Unix, the canonical path elsewhere (as the check's overlay).
#[derive(Debug, PartialEq, Eq)]
enum FileIdentity {
    #[cfg(unix)]
    Inode(u64, u64),
    #[cfg(not(unix))]
    Path(std::path::PathBuf),
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(FileIdentity::Inode(meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        Some(FileIdentity::Path(std::fs::canonicalize(path).ok()?))
    }
}

/// The run of [`Server::fresh`]: `iglint check` (`--fix`) of the folder
/// `root`, linting only `path`, with the disk after saving `target` (the
/// editor's text of `path`, if it is open) to `path`.
fn fresh_run(
    root: &Path,
    path: &Path,
    wanted: &FixWanted,
    target: Option<Arc<str>>,
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
    let overlay = target
        .as_ref()
        .map(|text| Arc::new(std::iter::once((path.to_path_buf(), Arc::clone(text))).collect()));
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
        FixWanted::Keys(keys) => {
            let keys = keys.clone();
            let at = path.to_path_buf();
            let select: engine::FixSelect = Arc::new(move |d: &Diagnostic| {
                absolute(d.file.path()) == at && keys.iter().any(|k| k.matches(d))
            });
            (FixMode::DiffOnly { unsafe_fixes: true }, true, Some(select))
        }
    };
    let options = CheckOptions {
        fix_mode,
        fix_each,
        fix_select: select,
        overlay,
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
    unsaved_target(&configured, &result, path, target.as_deref())?;
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
        FixWanted::Keys(_) => Fresh {
            diagnostics: Some(reported),
            fixed: None,
            each: singles,
        },
    };
    Ok(fresh)
}

/// When the editor's text of the target `path` differs from the disk:
/// why the run does not stand for `iglint check` after saving it, if it
/// does not (the run did not take the text, as for a manifest, which
/// discovery reads from disk, or a file that is not on disk; or the
/// configuration load read the file).
fn unsaved_target(
    configured: &Configured,
    result: &engine::CheckResult,
    path: &Path,
    target: Option<&str>,
) -> Result<(), String> {
    let Some(text) = target else {
        return Ok(());
    };
    if std::fs::read(path).is_ok_and(|bytes| bytes == text.as_bytes()) {
        return Ok(());
    }
    let identity = file_identity(path);
    let configuration = configured
        .sources
        .iter()
        .any(|p| p == path || identity.is_some() && file_identity(p) == identity);
    if configuration
        || result
            .overlay_applied
            .binary_search(&path.to_path_buf())
            .is_err()
    {
        return Err(format!(
            "the editor text of {} differs from the file on disk, and the check reads the disk",
            path.display()
        ));
    }
    Ok(())
}
