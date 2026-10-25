//! Incremental analysis for long-running clients (the language server).
//!
//! A [`LiveWorkspace`] runs phase A ([`crate::analyze`]) once over every
//! resource of a workspace and keeps what later runs need: the current text
//! of every file, and every resource's [`ResourceSummary`]. After that:
//!
//! * [`LiveWorkspace::set_text`] replaces the text of one file (an editor
//!   buffer), re-models and re-summarises only the resource that owns it, and
//!   reports which other files may have different findings because the
//!   summary changed ([`Dependents`]).
//! * [`LiveWorkspace::check`] runs both phases on the resources of the
//!   requested files against an index rebuilt from the kept summaries. The
//!   summaries move into the index and back, so nothing is copied.
//!
//! Findings are the same as those of [`crate::check`] for the same texts.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iglint_project::index::ProjectIndex;
use iglint_project::model::{LoadedFile, LoadedResource, ModelData};
use iglint_project::summary::Facts;
use iglint_project::{
    Container, FileId, JsonDocument, ProjectId, ResourceId, ResourceKind, ResourceModel,
    ResourceSummary, Workspace, build_model,
};
use iglint_text::{TextEdit, TextRange, TextSize};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analyze::{
    Treatment, is_json, primary_file, read_text, run_phase, shared_text, standalone_model,
    summarize_with_scripts, treatment,
};
use crate::checker::{Checker, FileTable, Unit, UnitCache};
use crate::diagnostic::{Diagnostic, FileEdit, SourceFile};
use crate::discovery::{self, absolute};
use crate::engine::{CheckOptions, FixMode, Reanalysis, fix_file, health_settings};
use crate::rule::{RuleImpl, RuleSetCache};
use crate::settings::{LinterSettings, ProjectSettings};
use crate::suppression;

/// Findings of one file.
#[derive(Clone, Debug)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    /// Why the file could not be linted (unreadable, not UTF-8).
    pub error: Option<String>,
}

/// Files whose findings may change after a [`LiveWorkspace::set_text`],
/// besides the edited file itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dependents {
    None,
    Files(Vec<PathBuf>),
    /// The change can affect any file of the workspace (page configuration,
    /// session properties, tags, a manifest, ...).
    All,
}

impl Dependents {
    fn from_resources(ws: &Workspace, resources: FxHashSet<ResourceId>) -> Dependents {
        if resources.is_empty() {
            return Dependents::None;
        }
        let mut files: Vec<PathBuf> = resources
            .into_iter()
            .flat_map(|r| ws.resource(r).files.iter())
            .map(|f| ws.file(*f).path.clone())
            .collect();
        files.sort();
        files.dedup();
        Dependents::Files(files)
    }
}

/// What [`LiveWorkspace::refresh_from_disk`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskChange {
    /// Every file and directory is as it was read.
    Unchanged,
    /// Files changed on disk and were read again: the files, and the other
    /// files whose findings may have changed.
    Files {
        changed: Vec<PathBuf>,
        dependents: Dependents,
    },
    /// What discovery found may have changed (a file or directory was
    /// added, removed or renamed, a manifest changed): only a new
    /// [`LiveWorkspace`] is current. The path is the first change seen.
    Structure(PathBuf),
}

/// What identifies the state of a file or directory on disk: its size,
/// modification and change (status) times, and its identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    /// The inode change time on Unix (which no tool can set back), the
    /// creation time elsewhere.
    changed: Option<SystemTime>,
    /// Device and inode on Unix.
    identity: (u64, u64),
    /// The number of hard links (Unix), and whether the path itself is a
    /// symbolic link.
    links: (u64, bool),
}

/// How much later than a read a file system may date a write: a write
/// within this time before a read may not show in the stamp (coarse time
/// stamps, a write in the same tick).
const TIMESTAMP_SLACK: Duration = Duration::from_secs(2);

impl Stamp {
    /// The stamp of `path` through links, `None` when it is missing or
    /// cannot be read.
    fn of(path: &Path) -> Option<Stamp> {
        let meta = std::fs::metadata(path).ok()?;
        #[cfg(unix)]
        let (changed, identity) = {
            use std::os::unix::fs::MetadataExt;
            let ctime = u64::try_from(meta.ctime()).ok().and_then(|s| {
                SystemTime::UNIX_EPOCH.checked_add(Duration::new(s, meta.ctime_nsec() as u32))
            });
            (ctime, (meta.dev(), meta.ino()))
        };
        #[cfg(not(unix))]
        let (changed, identity) = (meta.created().ok(), (0, 0));
        #[cfg(unix)]
        let hard_links = {
            use std::os::unix::fs::MetadataExt;
            meta.nlink()
        };
        #[cfg(not(unix))]
        let hard_links = 1;
        let symlink = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        Some(Stamp {
            len: meta.len(),
            modified: meta.modified().ok(),
            changed,
            identity,
            links: (hard_links, symlink),
        })
    }
}

/// What was on disk when a file or directory was last read: its stamp
/// taken before the read finished, and whether a change could hide from
/// the stamp (it is dated within [`TIMESTAMP_SLACK`] of the read, or
/// cannot be dated), so that only its contents tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Seen {
    stamp: Option<Stamp>,
    racy: bool,
}

impl Seen {
    /// `stamp`, taken after `since`, of something read after `since`.
    fn new(stamp: Option<Stamp>, since: SystemTime) -> Seen {
        let threshold = since.checked_sub(TIMESTAMP_SLACK);
        let recent = |t: Option<SystemTime>| match (t, threshold) {
            (Some(t), Some(threshold)) => t >= threshold,
            _ => true,
        };
        let racy = stamp.is_some_and(|s| recent(s.modified) || recent(s.changed));
        Seen { stamp, racy }
    }

    /// Whether `now` may differ from what was read.
    fn stale(&self, now: &Option<Stamp>) -> bool {
        self.racy || self.stamp != *now
    }
}

/// What [`LiveWorkspace::refresh_from_disk`] found for one file.
enum FileCheck {
    Same(Seen),
    Text(Seen, Result<crate::analyze::Decoded, String>),
    Structure,
}

/// The result of applying every applicable fix to a file.
#[derive(Clone, Debug)]
pub struct FixAll {
    pub fixed: String,
    /// Number of fixes applied.
    pub applied: usize,
}

/// A workspace kept in memory between runs.
pub struct LiveWorkspace {
    settings: ProjectSettings,
    roots: Vec<PathBuf>,
    ws: Workspace,
    /// No workspace root was found around a root
    /// ([`crate::discovery::WorkspaceRoots::unknown`]).
    closed_world_unknown: bool,
    rules: &'static [&'static RuleImpl],
    sets: RuleSetCache,
    /// Current text of every readable workspace file.
    table: FileTable,
    /// Why a workspace file could not be read.
    read_errors: FxHashMap<FileId, String>,
    /// Summaries in resource id order. Empty only while a run has moved
    /// them into an index.
    summaries: Vec<ResourceSummary>,
    by_path: FxHashMap<PathBuf, FileId>,
    /// Files of the workspace that are not linted (see
    /// [`crate::analyze::Treatment::Skip`]).
    not_modelled: FxHashSet<PathBuf>,
    /// Editor text of files outside the workspace.
    standalone: FxHashMap<PathBuf, Arc<str>>,
    /// Files whose text is the editor's, not the disk's.
    overlaid: FxHashSet<FileId>,
    /// What every file of the workspace was on disk when it was read, by
    /// file id (review round 31, B1).
    files_seen: Vec<Seen>,
    /// What every directory of [`Workspace::directories`] was when
    /// discovery read it.
    directories_seen: Vec<Seen>,
}

impl LiveWorkspace {
    /// Discovers the projects and gateway collections of the workspace
    /// roots of `roots` ([`discovery::workspace_roots`]) and summarises
    /// every resource. Findings are computed for files below `roots`.
    pub fn new(settings: ProjectSettings, roots: Vec<PathBuf>) -> LiveWorkspace {
        LiveWorkspace::with_rules(crate::rules::IMPLS, settings, roots)
    }

    /// Like [`LiveWorkspace::new`] with an explicit rule list (tests).
    pub fn with_rules(
        rules: &'static [&'static RuleImpl],
        settings: ProjectSettings,
        roots: Vec<PathBuf>,
    ) -> LiveWorkspace {
        let roots: Vec<PathBuf> = roots.iter().map(|r| absolute(r)).collect();
        // Every read below happens after this: what changes later is dated
        // after it, or within the slack (see `Seen`).
        let since = SystemTime::now();
        // Like `check`: the workspace is everything below the workspace
        // roots of the folder, which may lie above it (a folder opened on
        // one project still sees the gateway's tag scripts).
        let ws_roots = discovery::workspace_roots(&settings, &roots);
        let ws = Workspace::discover(&ws_roots.roots);
        let mut table = FileTable::with_len(ws.files.len());
        let mut read_errors = FxHashMap::default();
        for r in ws.resources.iter().filter(|r| !r.manifest.missing) {
            let doc = &r.manifest.doc;
            table.insert(
                r.manifest.file,
                SourceFile::decoded(
                    ws.file(r.manifest.file).path.clone(),
                    Arc::clone(doc.shared_text()),
                    r.manifest.invalid_utf8.clone(),
                ),
            );
        }
        for p in &ws.projects {
            let doc = &p.manifest.doc;
            table.insert(
                p.manifest.file,
                SourceFile::decoded(
                    ws.file(p.manifest.file).path.clone(),
                    Arc::clone(doc.shared_text()),
                    p.manifest.invalid_utf8.clone(),
                ),
            );
        }
        for c in &ws.collections {
            let doc = &c.manifest.doc;
            table.insert(
                c.manifest.file,
                SourceFile::decoded(
                    ws.file(c.manifest.file).path.clone(),
                    Arc::clone(doc.shared_text()),
                    c.manifest.invalid_utf8.clone(),
                ),
            );
        }
        type Loaded = (
            ResourceSummary,
            Vec<(FileId, Arc<str>, Vec<TextRange>)>,
            Vec<(FileId, String)>,
        );
        let loaded: Vec<Loaded> = ws
            .resources
            .par_iter()
            .map(|r| {
                let loaded = ws.load_resource(r.id);
                let texts = loaded
                    .files
                    .iter()
                    .filter_map(|f| shared_text(f).map(|t| (f.file, t, f.invalid_utf8.clone())))
                    .collect();
                let errors = loaded
                    .files
                    .iter()
                    .filter_map(|f| f.error.clone().map(|e| (f.file, e)))
                    .collect();
                (summary_of(&ws, &loaded), texts, errors)
            })
            .collect();
        let mut summaries = Vec::with_capacity(loaded.len());
        for (summary, texts, errors) in loaded {
            summaries.push(summary);
            for (file, text, invalid) in texts {
                table.insert(
                    file,
                    SourceFile::decoded(ws.file(file).path.clone(), text, invalid),
                );
            }
            read_errors.extend(errors);
        }
        // Files that `check` lints standalone are linted standalone here
        // too; files it skips are skipped.
        let mut not_modelled = FxHashSet::default();
        let by_path = ws
            .files
            .iter()
            .filter(|f| match treatment(&ws, f) {
                Treatment::Resource => true,
                Treatment::Standalone => false,
                Treatment::Skip(_) => {
                    not_modelled.insert(f.path.clone());
                    false
                }
            })
            .map(|f| (f.path.clone(), f.id))
            .collect();
        // The stamps are taken after the reads: a change between a read
        // and its stamp is dated after `since`, which makes the entry racy
        // (its contents are compared on every refresh until it settles).
        let files_seen = ws
            .files
            .par_iter()
            .map(|f| Seen::new(Stamp::of(&f.path), since))
            .collect();
        let directories_seen = ws
            .directories
            .par_iter()
            .map(|d| Seen::new(Stamp::of(&d.path), since))
            .collect();
        LiveWorkspace {
            settings,
            roots,
            ws,
            closed_world_unknown: !ws_roots.unknown.is_empty(),
            rules,
            sets: RuleSetCache::default(),
            table,
            read_errors,
            summaries,
            by_path,
            not_modelled,
            standalone: FxHashMap::default(),
            overlaid: FxHashSet::default(),
            files_seen,
            directories_seen,
        }
    }

    /// Whether `path` is a file of the workspace's index (its text counts
    /// for the findings of other files), wherever the roots are.
    pub fn indexes(&self, path: &Path) -> bool {
        self.contains(path)
    }

    /// Whether a new file at `path` may belong to the workspace's index:
    /// it is below a directory discovery read, or below a root.
    pub fn may_index(&self, path: &Path) -> bool {
        let path = absolute(path);
        self.covers(&path)
            || self.ws.directories.iter().any(|d| {
                d.kind != iglint_project::workspace::DirectoryKind::Probe
                    && path.starts_with(&d.path)
            })
            || self.indexes(&path)
    }

    /// The directories to watch for changes of the workspace: the
    /// outermost directories discovery read.
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = self
            .ws
            .directories
            .iter()
            .filter(|d| d.kind != iglint_project::workspace::DirectoryKind::Probe)
            .map(|d| d.path.clone())
            .chain(self.roots.iter().cloned())
            .collect();
        dirs.sort();
        dirs.dedup();
        let mut out: Vec<PathBuf> = Vec::new();
        for d in dirs {
            if !out.last().is_some_and(|o| d.starts_with(o)) {
                out.push(d);
            }
        }
        out
    }

    /// Compares every file and directory of the workspace with what was
    /// read (review round 31, B1: a file changed on disk without a
    /// notification, by `git pull` or another editor, must not leave a
    /// stale safe fix). Unchanged stamps (size, times, identity) are
    /// trusted, except for entries dated within [`TIMESTAMP_SLACK`] of
    /// their read, whose contents are compared. Changed data files are
    /// read again (except those with editor text) and their resources
    /// summarised again; anything discovery decides (a file or directory
    /// added or removed, a manifest) is [`DiskChange::Structure`], and only
    /// a new workspace is current then.
    pub fn refresh_from_disk(&mut self) -> DiskChange {
        let since = SystemTime::now();
        let directories: Vec<(usize, Option<Seen>)> = self
            .ws
            .directories
            .par_iter()
            .enumerate()
            .filter_map(|(i, d)| {
                let now = Stamp::of(&d.path);
                if !self.directories_seen[i].stale(&now) {
                    return None;
                }
                let same =
                    iglint_project::workspace::directory_signature(&d.path, d.kind) == d.signature;
                Some((i, same.then(|| Seen::new(now, since))))
            })
            .collect();
        for (i, seen) in directories {
            match seen {
                Some(seen) => self.directories_seen[i] = seen,
                None => return DiskChange::Structure(self.ws.directories[i].path.clone()),
            }
        }
        let checks: Vec<(usize, FileCheck)> = self
            .ws
            .files
            .par_iter()
            .enumerate()
            .filter(|(_, f)| !self.overlaid.contains(&f.id))
            .filter_map(|(i, f)| {
                let now = Stamp::of(&f.path);
                let old = self.files_seen[i];
                if !old.stale(&now) {
                    return None;
                }
                let seen = Seen::new(now, since);
                if now.is_none() {
                    // Gone (a placeholder for a missing manifest never
                    // had a stamp).
                    return Some((
                        i,
                        if old.stamp.is_none() {
                            FileCheck::Same(seen)
                        } else {
                            FileCheck::Structure
                        },
                    ));
                }
                // Another number of hard links, a link in place of a file or
                // the other way round, or another file behind a path that is
                // or was shared (a link retargeted): which files are shared
                // may have changed, even with the same bytes (review round
                // 32, Codex 1). A plain file saved by renaming a new one
                // over it is compared by content.
                let shared = |s: &Stamp| s.links != (1, false);
                if old.stamp.is_some_and(|o| {
                    now.is_some_and(|n| {
                        o.links != n.links
                            || (o.identity != n.identity && (shared(&o) || shared(&n)))
                    })
                }) {
                    return Some((i, FileCheck::Structure));
                }
                let current = self.table.get(f.id);
                if is_manifest_kind(f.kind) {
                    let same = read_text(&f.path)
                        .is_ok_and(|(text, _)| current.is_some_and(|c| *c.shared_text() == *text));
                    return Some((
                        i,
                        if same {
                            FileCheck::Same(seen)
                        } else {
                            FileCheck::Structure
                        },
                    ));
                }
                if current.is_none() && !self.read_errors.contains_key(&f.id) {
                    // Not read for the index (not a text file).
                    return Some((i, FileCheck::Same(seen)));
                }
                let read = read_text(&f.path);
                let same = match (&read, current) {
                    (Ok((text, invalid)), Some(c)) => {
                        !self.read_errors.contains_key(&f.id)
                            && *c.shared_text() == **text
                            && c.invalid_utf8() == invalid.as_slice()
                    }
                    (Err(e), _) => self.read_errors.get(&f.id) == Some(e),
                    (Ok(_), None) => false,
                };
                Some((
                    i,
                    if same {
                        FileCheck::Same(seen)
                    } else {
                        FileCheck::Text(seen, read)
                    },
                ))
            })
            .collect();
        let mut changed_files: Vec<FileId> = Vec::new();
        for (i, check) in checks {
            let file = self.ws.files[i].id;
            match check {
                FileCheck::Structure => {
                    return DiskChange::Structure(self.ws.files[i].path.clone());
                }
                FileCheck::Same(seen) => self.files_seen[i] = seen,
                FileCheck::Text(seen, read) => {
                    self.files_seen[i] = seen;
                    match read {
                        Ok((text, invalid)) => {
                            self.read_errors.remove(&file);
                            self.table.insert(
                                file,
                                SourceFile::decoded(self.ws.files[i].path.clone(), text, invalid),
                            );
                        }
                        Err(error) => {
                            self.read_errors.insert(file, error);
                        }
                    }
                    changed_files.push(file);
                }
            }
        }
        if changed_files.is_empty() {
            return DiskChange::Unchanged;
        }
        let changed: Vec<PathBuf> = changed_files
            .iter()
            .map(|f| self.ws.file(*f).path.clone())
            .collect();
        let mut resources: Vec<ResourceId> = changed_files
            .iter()
            .filter_map(|f| self.ws.file(*f).resource)
            .collect();
        resources.sort_unstable();
        resources.dedup();
        let dependents = self.resummarise(&resources);
        DiskChange::Files {
            changed,
            dependents,
        }
    }

    /// Summarises `resources` again from the current texts. Returns the
    /// files whose findings may have changed.
    fn resummarise(&mut self, resources: &[ResourceId]) -> Dependents {
        let fresh: Vec<(ResourceId, ResourceSummary)> = resources
            .par_iter()
            .map(|r| (*r, summary_of(&self.ws, &self.load_current(*r))))
            .collect();
        let mut old: Vec<(ResourceId, ResourceSummary)> = Vec::new();
        for (rid, new) in fresh {
            match self.summaries.get_mut(rid.index()) {
                Some(slot) if *slot == new => {}
                Some(slot) => old.push((rid, std::mem::replace(slot, new))),
                None => return Dependents::All,
            }
        }
        if old.is_empty() {
            return Dependents::None;
        }
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let mut files: Vec<PathBuf> = Vec::new();
        let mut all = false;
        for (rid, old) in &old {
            match dependents(&self.ws, &index, *rid, old) {
                Dependents::All => {
                    all = true;
                    break;
                }
                Dependents::Files(f) => files.extend(f),
                Dependents::None => {}
            }
        }
        self.summaries = index.into_summaries();
        if all {
            return Dependents::All;
        }
        files.sort();
        files.dedup();
        if files.is_empty() {
            Dependents::None
        } else {
            Dependents::Files(files)
        }
    }

    pub fn settings(&self) -> &ProjectSettings {
        &self.settings
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn workspace(&self) -> &Workspace {
        &self.ws
    }

    /// Whether `path` is a discovered file of the workspace.
    pub fn contains(&self, path: &Path) -> bool {
        self.by_path.contains_key(&absolute(path))
    }

    /// Whether `path` is below one of the roots.
    pub fn covers(&self, path: &Path) -> bool {
        let path = absolute(path);
        self.roots.iter().any(|r| path.starts_with(r))
    }

    /// Whether the workspace walk reached the project or collection that
    /// holds `path` on disk (or `path` is outside every one, or behind a
    /// symbolic link to a directory the walk recorded). A file of a
    /// project the walk did not reach (a linked Git work tree or a hidden
    /// directory below the folder) belongs to a workspace of its own.
    pub fn reaches(&self, path: &Path) -> bool {
        self.ws.unreached_container(&absolute(path)).is_none()
    }

    /// Whether `path` is excluded from linting by the settings.
    pub fn is_skipped(&self, path: &Path) -> bool {
        self.not_modelled.contains(&absolute(path))
            || discovery::skip_reason(&self.settings, path).is_some()
    }

    /// Every file below the roots that a check of the roots would lint.
    pub fn lintable_files(&self) -> Vec<PathBuf> {
        discovery::discover(&self.settings, &self.roots)
            .files
            .into_iter()
            .map(|(p, _)| absolute(&p))
            .collect()
    }

    /// The current text of a file: the editor's, the one read at startup, or
    /// (outside the workspace) the one on disk.
    pub fn source(&self, path: &Path) -> Option<SourceFile> {
        let path = absolute(path);
        if let Some(id) = self.by_path.get(&path) {
            return self.table.get(*id).cloned();
        }
        if let Some(text) = self.standalone.get(&path) {
            return Some(SourceFile::new(path, Arc::clone(text)));
        }
        read_text(&path)
            .ok()
            .map(|(t, invalid)| SourceFile::decoded(path, t, invalid))
    }

    /// Whether the file is a manifest (`resource.json`, `project.json`,
    /// `config-mode.json`): manifests are read during discovery, so a new
    /// manifest text only takes effect after [`LiveWorkspace::new`].
    pub fn is_manifest(&self, path: &Path) -> bool {
        self.by_path
            .get(&absolute(path))
            .is_some_and(|f| is_manifest_kind(self.ws.file(*f).kind))
    }

    /// Replaces the text of `path` (`None`: read it from disk again) and
    /// re-summarises its resource. Returns the other files whose findings may
    /// have changed.
    pub fn set_text(&mut self, path: &Path, text: Option<Arc<str>>) -> Dependents {
        let path = absolute(path);
        let Some(&file) = self.by_path.get(&path) else {
            match text {
                Some(text) => {
                    self.standalone.insert(path, text);
                }
                None => {
                    self.standalone.remove(&path);
                }
            }
            return Dependents::None;
        };
        let entry = self.ws.file(file);
        if is_manifest_kind(entry.kind) {
            // Discovery owns manifests; the caller rebuilds on save.
            return Dependents::None;
        }
        let text = match text {
            Some(text) => {
                self.overlaid.insert(file);
                Ok((text, Vec::new()))
            }
            None => {
                self.overlaid.remove(&file);
                let since = SystemTime::now();
                let stamp = Stamp::of(&path);
                let read = read_text(&path);
                if let Some(seen) = self.files_seen.get_mut(file.index()) {
                    *seen = Seen::new(stamp, since);
                }
                read
            }
        };
        match text {
            Ok((text, invalid)) => {
                if self
                    .table
                    .get(file)
                    .is_some_and(|s| *s.shared_text() == *text)
                {
                    return Dependents::None;
                }
                self.read_errors.remove(&file);
                self.table
                    .insert(file, SourceFile::decoded(path, text, invalid));
            }
            Err(error) => {
                self.read_errors.insert(file, error);
            }
        }
        let Some(rid) = entry.resource else {
            return Dependents::None;
        };
        let loaded = self.load_current(rid);
        let new = summary_of(&self.ws, &loaded);
        let Some(old) = self.summaries.get_mut(rid.index()) else {
            return Dependents::All;
        };
        if *old == new {
            return Dependents::None;
        }
        let old = std::mem::replace(old, new);
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let dependents = dependents(&self.ws, &index, rid, &old);
        self.summaries = index.into_summaries();
        dependents
    }

    /// [`LiveWorkspace::set_text`] and [`LiveWorkspace::check`] of `path`
    /// in one step, modelling the edited resource once. Returns the findings
    /// of `path` and the other files whose findings may have changed.
    pub fn update(&mut self, path: &Path, text: Arc<str>) -> (FileDiagnostics, Dependents) {
        let path = absolute(path);
        let resource = self
            .by_path
            .get(&path)
            .filter(|f| !is_manifest_kind(self.ws.file(**f).kind))
            .and_then(|f| self.ws.file(*f).resource.map(|r| (*f, r)));
        let Some((file, rid)) = resource.filter(|_| !self.is_skipped(&path)) else {
            let dependents = self.set_text(&path, Some(text));
            let found = self.check(std::slice::from_ref(&path)).remove(0);
            return (found, dependents);
        };
        self.read_errors.remove(&file);
        self.overlaid.insert(file);
        self.table.insert(file, SourceFile::new(path.clone(), text));
        let loaded = self.load_current(rid);
        let model = build_model(&self.ws, &loaded);
        let cache = UnitCache::default();
        let new = summarize_with_scripts(&self.ws, &model, &loaded, &cache);
        let old = std::mem::replace(&mut self.summaries[rid.index()], new);
        let changed = old != self.summaries[rid.index()];
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let dependents = if changed {
            dependents(&self.ws, &index, rid, &old)
        } else {
            Dependents::None
        };
        let found = self.run_resource(&index, &self.table, &model, &cache, None);
        let result = self
            .distribute(std::slice::from_ref(&path), found, FxHashMap::default())
            .remove(0);
        self.summaries = index.into_summaries();
        drop(model);
        drop_in_background(loaded);
        (result, dependents)
    }

    /// Findings of `paths`, in the order given.
    pub fn check(&mut self, paths: &[PathBuf]) -> Vec<FileDiagnostics> {
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let result = self.check_with(&index, paths);
        self.summaries = index.into_summaries();
        result
    }

    /// [`LiveWorkspace::check`] of `paths` a few files at a time, one index
    /// for all, stopping before the next few once `stop` says so (the
    /// language server: a new message arrived). Returns the findings of the
    /// files checked, in the order given, and the files not checked.
    pub fn check_until(
        &mut self,
        paths: &[PathBuf],
        stop: &dyn Fn() -> bool,
    ) -> (Vec<FileDiagnostics>, Vec<PathBuf>) {
        /// About 15 ms of work in an optimised build on a 3,000-file project.
        const CHUNK: usize = 128;
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let mut out = Vec::with_capacity(paths.len());
        let mut rest: &[PathBuf] = &[];
        for (n, chunk) in paths.chunks(CHUNK).enumerate() {
            if stop() {
                rest = &paths[n * CHUNK..];
                break;
            }
            out.extend(self.check_with(&index, chunk));
        }
        self.summaries = index.into_summaries();
        (out, rest.to_vec())
    }

    fn check_with(&self, index: &ProjectIndex<'_>, paths: &[PathBuf]) -> Vec<FileDiagnostics> {
        let paths: Vec<PathBuf> = paths.iter().map(|p| absolute(p)).collect();
        let mut resources: Vec<ResourceId> = Vec::new();
        let mut projects: Vec<ProjectId> = Vec::new();
        let mut standalone: Vec<&Path> = Vec::new();
        for path in &paths {
            if self.is_skipped(path) {
                continue;
            }
            match self.by_path.get(path) {
                Some(f) => {
                    let entry = self.ws.file(*f);
                    if let Some(r) = entry.resource {
                        resources.push(r);
                    } else if let Some(p) = self.ws.projects.iter().find(|p| p.manifest.file == *f)
                    {
                        projects.push(p.id);
                    }
                }
                None => standalone.push(path),
            }
        }
        resources.sort_unstable();
        resources.dedup();
        projects.sort_unstable();
        projects.dedup();

        let mut found: Vec<Diagnostic> = resources
            .par_iter()
            .flat_map_iter(|r| self.analyze_resource(index, &self.table, *r, None))
            .collect();
        for p in projects {
            found.extend(self.analyze_project(index, p));
        }
        let standalone_found: FxHashMap<&Path, Result<Vec<Diagnostic>, String>> = standalone
            .par_iter()
            .map(|path| {
                let text = match self.standalone.get(*path) {
                    Some(text) => Ok((Arc::clone(text), Vec::new())),
                    None => read_text(path),
                };
                (
                    *path,
                    text.map(|(t, invalid)| self.analyze_standalone(index, path, t, invalid, None)),
                )
            })
            .collect();

        self.distribute(&paths, found, standalone_found)
    }

    /// Gives each of `paths` its findings, with the file's settings applied
    /// (rules switched off for the file, configured severities).
    fn distribute(
        &self,
        paths: &[PathBuf],
        found: Vec<Diagnostic>,
        mut standalone_found: FxHashMap<&Path, Result<Vec<Diagnostic>, String>>,
    ) -> Vec<FileDiagnostics> {
        let mut by_file: FxHashMap<PathBuf, Vec<Diagnostic>> = FxHashMap::default();
        for d in found {
            by_file
                .entry(d.file.path().to_path_buf())
                .or_default()
                .push(d);
        }
        paths
            .iter()
            .map(|path| {
                let mut out = FileDiagnostics {
                    path: path.clone(),
                    diagnostics: Vec::new(),
                    error: None,
                };
                if self.is_skipped(path) {
                    return out;
                }
                let settings = self.settings.for_file(path);
                let diagnostics = match self.by_path.get(path) {
                    Some(f) => {
                        if let Some(error) = self.read_errors.get(f) {
                            out.error = Some(error.clone());
                        }
                        by_file.get(path).cloned().unwrap_or_default()
                    }
                    None => match standalone_found.remove(path.as_path()) {
                        Some(Ok(d)) => d,
                        Some(Err(error)) => {
                            out.error = Some(error);
                            Vec::new()
                        }
                        None => Vec::new(),
                    },
                };
                out.diagnostics = diagnostics
                    .into_iter()
                    .filter_map(|mut d| {
                        let rule = settings.rule(d.rule)?;
                        if !self.reported_with_own_settings(&d)
                            && !(d.severity_from_options
                                && self.unit_rule_settings(&d).as_ref() == Some(rule))
                        {
                            d.severity = rule.severity;
                        }
                        Some(d)
                    })
                    .collect();
                // A safe fix the post-fix net would drop is shown as
                // unsafe, as `check` reports it.
                if let Some(source) = self.source(path) {
                    crate::safety::mark_rejected_safe_fixes(
                        path,
                        source.text(),
                        &mut out.diagnostics,
                        &|text: &str| self.script_texts(path, text),
                    );
                }
                crate::rules::script::pylint::drop_duplicates(&mut out.diagnostics);
                out.diagnostics
                    .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
                out
            })
            .collect()
    }

    /// Whether a finding was reported by a unit that ran with the settings
    /// of the finding's file. Otherwise the file's configured severity
    /// applies, as in [`crate::check`].
    fn reported_with_own_settings(&self, d: &Diagnostic) -> bool {
        let path = d.file.path();
        match self.by_path.get(path) {
            Some(f) => match self.ws.file(*f).resource {
                Some(r) => self.unit_settings_path(r).as_deref() == Some(path),
                None => false,
            },
            None => true,
        }
    }

    /// The settings of `d`'s rule in the file whose settings the unit that
    /// reported `d` ran with.
    fn unit_rule_settings(&self, d: &Diagnostic) -> Option<crate::settings::RuleSettings> {
        let f = self.by_path.get(d.file.path())?;
        let r = self.ws.file(*f).resource?;
        let path = self.unit_settings_path(r)?;
        self.settings.for_file(&path).rule(d.rule).cloned()
    }

    /// The file whose settings a resource's unit runs with: its first data
    /// file that is linted, else its manifest, as `check` picks it (review
    /// round 32: a file iglint does not lint, a view's `thumbnail.png`,
    /// gave the unit its settings, so rules the configuration enables only
    /// for some files did not run).
    fn unit_settings_path(&self, rid: ResourceId) -> Option<PathBuf> {
        let r = self.ws.resource(rid);
        r.files
            .iter()
            .filter(|f| **f != r.manifest.file)
            .chain(std::iter::once(&r.manifest.file))
            .map(|f| &self.ws.file(*f).path)
            .find(|p| discovery::FileKind::classify(p).is_some() && !self.is_skipped(p))
            .cloned()
    }

    fn unit_settings(&self, rid: ResourceId) -> Option<Arc<LinterSettings>> {
        self.unit_settings_path(rid)
            .map(|p| self.settings.for_file(&p))
    }

    /// The data files of a resource with their current text.
    fn load_current(&self, rid: ResourceId) -> LoadedResource {
        let files = self
            .ws
            .data_files(rid)
            .filter(|f| f.kind.is_text())
            .map(|f| {
                let text = self.table.get(f.id).map(SourceFile::shared_text);
                let error = match &text {
                    Some(_) => None,
                    None => Some(
                        self.read_errors
                            .get(&f.id)
                            .cloned()
                            .unwrap_or_else(|| "file could not be read".to_owned()),
                    ),
                };
                let json = match (&text, f.kind.is_json()) {
                    (Some(t), true) => Some(JsonDocument::parse(Arc::clone(t))),
                    _ => None,
                };
                LoadedFile {
                    file: f.id,
                    kind: f.kind,
                    text: if json.is_some() { None } else { text },
                    json,
                    error,
                    invalid_utf8: self
                        .table
                        .get(f.id)
                        .map(|s| s.invalid_utf8().to_vec())
                        .unwrap_or_default(),
                }
            })
            .collect();
        LoadedResource {
            resource: rid,
            files,
        }
    }

    /// Both phases on one resource.
    /// Both phases on one resource, with the configured settings or
    /// `settings`.
    fn analyze_resource(
        &self,
        index: &ProjectIndex<'_>,
        table: &FileTable,
        rid: ResourceId,
        settings: Option<&Arc<LinterSettings>>,
    ) -> Vec<Diagnostic> {
        let Some(settings) = settings.cloned().or_else(|| self.unit_settings(rid)) else {
            return Vec::new();
        };
        if self.sets.get(self.rules, &settings).is_empty() {
            return Vec::new();
        }
        let mut loaded = self.load_current(rid);
        for f in &mut loaded.files {
            if let Some(source) = table.get(f.file)
                && f.text() != Some(source.text())
            {
                crate::analyze::replace_text(f, source.shared_text());
            }
        }
        let model = build_model(&self.ws, &loaded);
        self.run_resource(index, table, &model, &UnitCache::default(), Some(&settings))
    }

    /// Both phases on the model of a resource.
    fn run_resource(
        &self,
        index: &ProjectIndex<'_>,
        table: &FileTable,
        model: &ResourceModel<'_>,
        cache: &UnitCache,
        settings: Option<&Arc<LinterSettings>>,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(settings) = settings
            .cloned()
            .or_else(|| self.unit_settings(model.resource.id))
        else {
            return out;
        };
        let set = self.sets.get(self.rules, &settings);
        if set.is_empty() {
            return out;
        }
        let unit = Unit {
            ws: &self.ws,
            resource: Some(model.resource),
            project: project_of(model.resource.container),
            data: &model.data,
            errors: &model.errors,
            primary: primary_file(model),
            files: table,
            strict: settings.strict,
            cache,
        };
        run_phase(&unit, None, &set, &set.local, &mut out);
        run_phase(&unit, Some(index), &set, &set.project, &mut out);
        self.gate_file_reads(&mut out, index);
        out
    }

    /// Code that may read workspace files observes every fix
    /// ([`crate::rules::vetting::gate_file_reads`]).
    fn gate_file_reads(&self, out: &mut [Diagnostic], index: &ProjectIndex<'_>) {
        crate::rules::vetting::gate_file_reads(
            out,
            index,
            &self.table,
            self.settings.assume_no_workspace_file_reads,
        );
    }

    /// Project rules of one project (reported on `project.json`).
    fn analyze_project(&self, index: &ProjectIndex<'_>, project: ProjectId) -> Vec<Diagnostic> {
        let p = self.ws.project(project);
        let manifest = p.manifest.file;
        let settings = self.settings.for_file(&self.ws.file(manifest).path);
        let set = self.sets.get(self.rules, &settings);
        let mut out = Vec::new();
        let none = ModelData::None;
        let cache = UnitCache::default();
        let unit = Unit {
            ws: &self.ws,
            resource: None,
            project: Some(project),
            data: &none,
            errors: &[],
            primary: manifest,
            files: &self.table,
            strict: settings.strict,
            cache: &cache,
        };
        for (slot, rule) in &set.project.project {
            let mut cx = Checker::new(&unit, Some(index), &set.active[*slot], &mut out);
            rule.check_project(&mut cx, project);
        }
        self.gate_file_reads(&mut out, index);
        out
    }

    /// A file outside the workspace, analysed on its own.
    fn analyze_standalone(
        &self,
        index: &ProjectIndex<'_>,
        path: &Path,
        text: Arc<str>,
        invalid_utf8: Vec<TextRange>,
        settings: Option<&Arc<LinterSettings>>,
    ) -> Vec<Diagnostic> {
        let settings = settings
            .cloned()
            .unwrap_or_else(|| self.settings.for_file(path));
        let set = self.sets.get(self.rules, &settings);
        let mut out = Vec::new();
        if set.is_empty() {
            return out;
        }
        let file = FileId(self.ws.files.len() as u32);
        let mut table = FileTable::with_len(0);
        table.insert(
            file,
            SourceFile::decoded(path, Arc::clone(&text), invalid_utf8),
        );
        let doc = is_json(path).then(|| JsonDocument::parse(Arc::clone(&text)));
        let (data, errors) = standalone_model(path, file, doc.as_ref(), &text);
        let cache = UnitCache::default();
        let unit = Unit {
            ws: &self.ws,
            resource: None,
            project: self.ws.file_by_path(path).and_then(|f| match f.container {
                Some(Container::Project(p)) => Some(p),
                _ => None,
            }),
            data: &data,
            errors: &errors,
            primary: file,
            files: &table,
            strict: settings.strict,
            cache: &cache,
        };
        run_phase(&unit, None, &set, &set.local, &mut out);
        run_phase(&unit, Some(index), &set, &set.project, &mut out);
        self.gate_file_reads(&mut out, index);
        out
    }

    /// Applies every fix of `path` (safe ones, and unsafe ones when asked)
    /// in passes, re-analysing in between. Returns `None` when nothing
    /// changes. The workspace is not modified.
    pub fn fix_all(&mut self, path: &Path, unsafe_fixes: bool) -> Option<FixAll> {
        let path = absolute(path);
        let diagnostics = self
            .check(std::slice::from_ref(&path))
            .pop()
            .map(|f| f.diagnostics)
            .unwrap_or_default();
        self.apply_fixes(&path, diagnostics, unsafe_fixes, None)
    }

    /// Applies the fix of one finding of `path` (a quick fix) the way
    /// [`Live::fix_all`] and `--fix` apply fixes, through
    /// [`crate::engine::fix_file`]: a fix that would make the file worse,
    /// or a safe fix that fails the safe-fix net, gives `None`. Unsafe
    /// fixes are applied (the caller labels them).
    ///
    /// The fix is applied only when a fresh check of the current workspace
    /// still reports the finding with the same fix (review round 29, B2:
    /// a quick fix published before an edit of another file could be stale,
    /// a safe removal of an import that the edit made used).
    pub fn fix_one(&mut self, path: &Path, diagnostic: &Diagnostic) -> Option<FixAll> {
        let path = absolute(path);
        let current = self.check(std::slice::from_ref(&path)).pop()?;
        if !current
            .diagnostics
            .iter()
            .any(|d| same_finding(d, diagnostic))
        {
            return None;
        }
        self.apply_fixes(&path, vec![diagnostic.clone()], true, Some(1))
    }

    /// Runs [`crate::engine::fix_file`], the one function every fix path
    /// shares, on `diagnostics` of `path`.
    fn apply_fixes(
        &mut self,
        path: &Path,
        diagnostics: Vec<Diagnostic>,
        unsafe_fixes: bool,
        passes: Option<usize>,
    ) -> Option<FixAll> {
        let path = path.to_path_buf();
        let source = self.source(&path)?;
        if diagnostics.iter().all(|d| d.fix.is_none()) {
            return None;
        }
        let summaries = std::mem::take(&mut self.summaries);
        let index = crate::analyze::build_index(
            &self.ws,
            summaries,
            self.closed_world_unknown,
            &self.settings.vetted,
        );
        let mut options = CheckOptions {
            fix_mode: FixMode::DiffOnly { unsafe_fixes },
            stdin: Some((path.clone(), String::new())),
            ..CheckOptions::default()
        };
        if let Some(passes) = passes {
            options.max_fix_passes = passes;
        }
        let reanalyze =
            |text: &str, _edits: &[iglint_text::TextEdit], what: Reanalysis| -> Vec<Diagnostic> {
                let text: Arc<str> = Arc::from(text);
                let forced = (what == Reanalysis::Health).then(health_settings);
                match self.by_path.get(&path) {
                    Some(f) => {
                        let Some(rid) = self.ws.file(*f).resource else {
                            return Vec::new();
                        };
                        let mut table = self.table.clone();
                        table.insert(*f, SourceFile::new(path.clone(), text));
                        let settings = forced
                            .cloned()
                            .unwrap_or_else(|| self.settings.for_file(&path));
                        self.analyze_resource(&index, &table, rid, forced)
                            .into_iter()
                            .filter(|d| d.file.path() == path && settings.is_enabled(d.rule))
                            .collect()
                    }
                    None => self.analyze_standalone(&index, &path, text, Vec::new(), forced),
                }
            };
        let scripts = |text: &str| self.script_texts(&path, text);
        let result = fix_file(
            &self.settings,
            &options,
            &path,
            &source.shared_text(),
            diagnostics,
            &reanalyze,
            &scripts,
        );
        self.summaries = index.into_summaries();
        let (summary, _) = result.ok()?;
        let summary = summary?;
        summary.changed().then_some(FixAll {
            applied: summary.applied.len(),
            fixed: summary.fixed,
        })
    }

    /// The scripts of `path` with the text `text`, for the safety net.
    fn script_texts(&self, path: &Path, text: &str) -> Vec<crate::safety::ScriptText> {
        let text: Arc<str> = Arc::from(text);
        match self.by_path.get(path) {
            Some(f) => {
                let Some(rid) = self.ws.file(*f).resource else {
                    return Vec::new();
                };
                let mut loaded = self.load_current(rid);
                for lf in &mut loaded.files {
                    if lf.file == *f {
                        crate::analyze::replace_text(lf, Arc::clone(&text));
                    }
                }
                let model = build_model(&self.ws, &loaded);
                model
                    .scripts()
                    .into_iter()
                    .filter(|(file, _)| file == f)
                    .map(|(_, s)| crate::safety::ScriptText::of(s))
                    .collect()
            }
            None => crate::analyze::standalone_scripts(path, text),
        }
    }

    /// Whether findings of `rule` can be suppressed with a script comment:
    /// the rule checks scripts.
    pub fn suppressible(&self, rule: crate::registry::RuleCode) -> bool {
        self.rules.iter().any(|r| {
            r.code() == rule
                && r.hooks
                    .iter()
                    .any(|h| matches!(h, crate::rule::Hook::Script(_)))
        })
    }

    /// An edit that adds `# iglint: ignore[CODE]` to the script line a
    /// finding starts on, for findings inside scripts (embedded in JSON or
    /// in Python files). `None` when the finding is not in a script or the
    /// comment cannot be placed there (the line ends inside a string).
    pub fn suppression_edit(&self, diagnostic: &Diagnostic) -> Option<FileEdit> {
        let path = absolute(diagnostic.file.path());
        let text = diagnostic.file.shared_text();
        let current = self.source(&path)?;
        if *current.shared_text() != *text {
            return None;
        }
        let code = diagnostic.rule.code();
        let start = diagnostic.range.start();
        let edit_in = |data: &ModelData<'_>, file: FileId| -> Option<TextEdit> {
            data.scripts()
                .into_iter()
                .filter(|(f, _)| *f == file)
                .find_map(|(_, script)| {
                    let offset = script.source_map.generated_offset(start)?;
                    if offset < script.source_map.body_start() {
                        return None;
                    }
                    let (at, insert) = suppression_insert(&script.source, offset, code)?;
                    let file_offset = script.file_range(TextRange::empty(at)).start();
                    Some(TextEdit::insert(file_offset, insert))
                })
        };
        let edit = match self.by_path.get(&path) {
            Some(f) => {
                let rid = self.ws.file(*f).resource?;
                let loaded = self.load_current(rid);
                let model = build_model(&self.ws, &loaded);
                edit_in(&model.data, *f)
            }
            None => {
                let file = FileId(self.ws.files.len() as u32);
                let doc = is_json(&path).then(|| JsonDocument::parse(Arc::clone(&text)));
                let (data, _) = standalone_model(&path, file, doc.as_ref(), &text);
                edit_in(&data, file)
            }
        }?;
        Some(FileEdit {
            path: diagnostic.file.path().to_path_buf(),
            edit,
        })
    }
}

/// Where to put a suppression of `code` on the script line containing
/// `offset`, and the text to insert. The result is verified: the new source
/// has a marker on that line that selects `code`.
fn suppression_insert(source: &str, offset: TextSize, code: &str) -> Option<(TextSize, String)> {
    let at = iglint_text::floor_char_boundary(source, offset.to_usize());
    let (before, after) = source.split_at_checked(at)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let mut line_end = after.find('\n').map_or(source.len(), |i| at + i);
    if line_end > line_start && source.as_bytes()[line_end - 1] == b'\r' {
        line_end -= 1;
    }
    let comment = suppression::comments(source)
        .into_iter()
        .find(|c| c.start().to_usize() >= line_start && c.start().to_usize() <= line_end);
    let (insert_at, insert) = match comment {
        Some(c) => {
            let text = c.slice(source);
            match suppression::parse_comment(text) {
                Some((selectors, false)) if selectors.is_empty() => return None,
                Some((_, false)) => {
                    let close = text.find(']')?;
                    (c.start().to_usize() + close, format!(", {code}"))
                }
                _ => (c.start().to_usize(), format!("# iglint: ignore[{code}]  ")),
            }
        }
        None => {
            let line = source.get(line_start..line_end)?;
            let trimmed = line.trim_end().len() + line_start;
            (trimmed, format!("  # iglint: ignore[{code}]"))
        }
    };
    let mut new = String::with_capacity(source.len() + insert.len());
    let (head, tail) = source.split_at_checked(insert_at)?;
    new.push_str(head);
    new.push_str(&insert);
    new.push_str(tail);
    let new_line_end = line_end + insert.len();
    let ok = suppression::comments(&new).into_iter().any(|c| {
        c.start().to_usize() >= line_start
            && c.end().to_usize() <= new_line_end
            && suppression::parse_comment(c.slice(&new)).is_some_and(|(selectors, file_wide)| {
                !file_wide && selectors.iter().any(|s| s == code)
            })
    });
    ok.then(|| (TextSize::new(insert_at as u32), insert))
}

pub(crate) fn is_manifest_kind(kind: iglint_project::FileKind) -> bool {
    use iglint_project::FileKind as K;
    matches!(
        kind,
        K::ProjectManifest | K::CollectionManifest | K::ResourceManifest | K::UnaryResourceManifest
    )
}

fn project_of(container: Container) -> Option<ProjectId> {
    match container {
        Container::Project(p) => Some(p),
        Container::Collection(_) => None,
    }
}

/// Models and summarises a loaded resource the way phase A does.
fn summary_of(ws: &Workspace, loaded: &LoadedResource) -> ResourceSummary {
    summarize_with_scripts(ws, &build_model(ws, loaded), loaded, &UnitCache::default())
}

/// Freeing the parse trees of a large resource takes milliseconds; do it
/// off the latency path.
fn drop_in_background(loaded: LoadedResource) {
    let spawned = std::thread::Builder::new()
        .name("iglint-drop".to_owned())
        .spawn(move || drop(loaded));
    // Without a thread, `loaded` was dropped with the closure here.
    let _ = spawned;
}

/// Whether `a` and `b` are the same finding of the same text, with the
/// same fix (review round 29, B2).
pub fn same_finding(a: &Diagnostic, b: &Diagnostic) -> bool {
    a.rule == b.rule
        && a.severity == b.severity
        && a.range == b.range
        && a.message == b.message
        && a.node_path == b.node_path
        && a.labels == b.labels
        && a.fix == b.fix
        && a.file.path() == b.file.path()
        && a.file.text() == b.file.text()
}

/// Whether two lists of findings are the same, in order ([`same_finding`]).
pub fn same_findings(a: &[Diagnostic], b: &[Diagnostic]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same_finding(a, b))
}

/// Resources whose findings may change because the summary of `changed`
/// went from `old` to its current one in `index`.
///
/// Default deny (review round 29, B2): only what is known to matter for
/// named resources is followed precisely; any other change counts for
/// every file. A library module's names, imports and references feed every
/// path through it (an edit of a module in the middle of a chain changes
/// where `m1.m2.util.json` ends), which modules are used as values,
/// exposed or reach shared objects, call following (`VIEW015` through two
/// library calls), vetting and the closed world: any change of one counts
/// for every file. So does any change of what a resource contributes to
/// the whole project (stores, values, exposure, star imports, dynamic
/// references, unknown code, file reads, unvetted names, Perspective
/// calls, messages), or of a library path whose objects are used as values.
fn dependents(
    ws: &Workspace,
    index: &ProjectIndex<'_>,
    changed: ResourceId,
    old: &ResourceSummary,
) -> Dependents {
    let Some(new) = index.summary(changed) else {
        return Dependents::All;
    };
    let resource = ws.resource(changed);
    let mut out: FxHashSet<ResourceId> = FxHashSet::default();

    // Facts other resources look up.
    if old.valid != new.valid || old.facts != new.facts {
        let by_reference = match (&old.facts, &new.facts) {
            (Facts::View(a), Facts::View(b)) => a.message_handlers == b.message_handlers,
            (Facts::NamedQuery(_), Facts::NamedQuery(_))
            | (Facts::StyleClass(_), Facts::StyleClass(_)) => true,
            _ => false,
        };
        if !by_reference {
            return Dependents::All;
        }
        for r in &ws.resources {
            let (Some(s), Some(project)) = (index.summary(r.id), project_of(r.container)) else {
                continue;
            };
            if references_resource(index, project, &s.references, resource.kind, changed) {
                out.insert(r.id);
            }
        }
    }

    // Usage counts of the resources it references.
    if old.references != new.references {
        if resource.kind == ResourceKind::ScriptLibrary
            || without_named(&old.references) != without_named(&new.references)
        {
            return Dependents::All;
        }
        if let Some(project) = project_of(resource.container) {
            let projects: Vec<ProjectId> = std::iter::once(project)
                .chain(ws.project_descendants(project))
                .collect();
            for refs in [&old.references, &new.references] {
                for p in &projects {
                    if !targets(index, *p, refs, &mut out) {
                        return Dependents::All;
                    }
                }
            }
        }
    }
    out.remove(&changed);
    Dependents::from_resources(ws, out)
}

/// `refs` without what [`dependents`] follows precisely: the views, named
/// queries, style classes and library paths a resource names, and what
/// only its own findings use (pages, tag paths, historian providers,
/// databases); and without the places of the sites it names (typing moves
/// them; a site only explains another file's unsafe fix, whose finding a
/// quick fix checks again before it applies).
fn without_named(
    refs: &iglint_project::summary::References,
) -> iglint_project::summary::References {
    use iglint_project::summary::UnknownSite;
    let place = |s: &UnknownSite| UnknownSite {
        range: TextRange::default(),
        ..s.clone()
    };
    let places = |list: &[(String, UnknownSite)]| -> Vec<(String, UnknownSite)> {
        list.iter().map(|(n, s)| (n.clone(), place(s))).collect()
    };
    iglint_project::summary::References {
        views: Vec::new(),
        style_classes: Vec::new(),
        named_queries: Vec::new(),
        pages: Vec::new(),
        tag_paths: Vec::new(),
        library: Vec::new(),
        history_providers: Vec::new(),
        databases: Vec::new(),
        unknown_site: refs.unknown_site.as_ref().map(place),
        file_read_site: refs.file_read_site.as_ref().map(place),
        library_changes: places(&refs.library_changes),
        library_values: places(&refs.library_values),
        unvetted_imports: places(&refs.unvetted_imports),
        unvetted_names: refs
            .unvetted_names
            .iter()
            .map(|(n, s)| (n.clone(), place(s)))
            .collect(),
        perspective_calls: refs
            .perspective_calls
            .iter()
            .map(|c| iglint_project::summary::PerspectiveCall {
                site: place(&c.site),
                ..c.clone()
            })
            .collect(),
        ..refs.clone()
    }
}

/// Whether `refs`, seen from `project`, name the resource `target`.
fn references_resource(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    refs: &iglint_project::summary::References,
    kind: ResourceKind,
    target: ResourceId,
) -> bool {
    match kind {
        ResourceKind::View => refs
            .views
            .iter()
            .any(|v| index.view(project, v).resource() == Some(target)),
        ResourceKind::NamedQuery => refs
            .named_queries
            .iter()
            .any(|q| index.named_query(project, q).resource() == Some(target)),
        ResourceKind::StyleClass => refs
            .style_classes
            .iter()
            .any(|c| index.style_class(project, c).resource() == Some(target)),
        ResourceKind::ScriptLibrary => refs.library.iter().any(|l| {
            index
                .resolve_library_path(project, l)
                .modules
                .contains(&target)
        }),
        _ => false,
    }
}

/// Adds the resources `refs` resolve to from `project`. False when a
/// library path reaches objects it uses as values (default deny: whoever
/// holds a module reaches what it imports, transitively).
fn targets(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    refs: &iglint_project::summary::References,
    out: &mut FxHashSet<ResourceId>,
) -> bool {
    out.extend(
        refs.views
            .iter()
            .filter_map(|v| index.view(project, v).resource()),
    );
    out.extend(
        refs.named_queries
            .iter()
            .filter_map(|q| index.named_query(project, q).resource()),
    );
    out.extend(
        refs.style_classes
            .iter()
            .filter_map(|c| index.style_class(project, c).resource()),
    );
    // Every module whose names a library path reads, through imports.
    let library = index.library(project);
    let symbols = |r: ResourceId| index.module_symbols(r);
    for name in &refs.library {
        let path = index.resolve_library_path(project, name);
        if !path.values.is_empty() || !library.value_objects(&path, &symbols).is_empty() {
            return false;
        }
        out.extend(path.modules);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{RuleCode, RuleMeta};
    use crate::rule::{Hook, Phase, ScriptRule, ScriptUnit};
    use crate::settings::RuleSettings;

    static BAD_NAME: RuleMeta = RuleMeta {
        code: "SCR900",
        name: "bad-name",
        ..RuleMeta::DEFAULT
    };

    struct BadName;

    impl ScriptRule for BadName {
        fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
            let source: &str = &unit.script.source;
            let body = unit.script.source_map.body_start().to_usize();
            for (i, _) in source.get(body..).unwrap_or_default().match_indices("bad") {
                let start = TextSize::new((body + i) as u32);
                let range = TextRange::at(start, TextSize::new(3));
                cx.report_script(unit.file, unit.script, range, "bad name");
            }
        }
    }

    static BAD_NAME_RULE: RuleImpl = RuleImpl {
        meta: &BAD_NAME,
        phase: Phase::Local,
        hooks: &[Hook::Script(&BadName)],
        options: None,
    };

    static TEST_RULES: &[&RuleImpl] = &[&BAD_NAME_RULE];

    /// Review round 29 (Opus, B2): a quick fix is applied only while a
    /// fresh check of the current workspace reports the same finding with
    /// the same fix. Here the published finding is stale: another module
    /// changed (`m1` now imports `m2`, so `b`'s `m1.m2.util.json` uses
    /// `json` of `lib.util`) and the caller did not check `lib.util` again.
    #[test]
    fn stale_findings_get_no_quick_fix() {
        use crate::rules::script::testing::{library, with_library};
        let f = with_library(
            library("lib/util", "import json\ndef f():\n    return 1\n"),
            "m2",
            "from lib import util\n",
        );
        let f = with_library(f, "m1", "def x():\n    return 1\n");
        let f = with_library(f, "b", "def g(n):\n    return m1.m2.util.json.dumps(1)\n");
        let root = absolute(f.root());
        let settings = crate::rules::test_support::settings(&root, &[("SCR102", "")]);
        let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
        let util = root.join("projects/p/ignition/script-python/lib/util/code.py");
        let m1 = root.join("projects/p/ignition/script-python/m1/code.py");
        let found = live
            .check(std::slice::from_ref(&util))
            .remove(0)
            .diagnostics;
        let stale = found
            .iter()
            .find(|d| d.rule.code() == "SCR102")
            .unwrap_or_else(|| panic!("{found:#?}"))
            .clone();
        assert_eq!(
            stale.fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Safe)
        );
        // While the finding is current, the fix applies.
        assert!(live.fix_one(&util, &stale).is_some());
        live.set_text(&m1, Some(Arc::from("import m2\ndef x():\n    return 1\n")));
        assert!(live.fix_one(&util, &stale).is_none());
        let fresh = live
            .check(std::slice::from_ref(&util))
            .remove(0)
            .diagnostics;
        assert!(
            !fresh.iter().any(|d| d.rule.code() == "SCR102"),
            "{fresh:#?}"
        );
        // An edit of a library module counts for every file.
        assert_eq!(
            live.set_text(&m1, Some(Arc::from("def x():\n    return 2\n"))),
            Dependents::All
        );
    }

    /// Review round 30 (Opus minor 1): checking a few files at a time with
    /// one index, stopping anywhere and checking the rest later, gives the
    /// findings one check of every file gives.
    #[test]
    fn checks_in_parts_match_one_check() {
        use crate::rules::script::testing::{library, with_library};
        let mut f = with_library(
            library(
                "lib/util",
                "import json\nimport os\ndef f():\n    return 1\n",
            ),
            "m1",
            "from lib import util\n",
        );
        for i in 0..150 {
            f = with_library(
                f,
                &format!("u{i}"),
                &format!(
                    "import sys\ndef g{i}(n):\n    return m1.util.json.dumps({i}) + lib.util.nothere{}\n",
                    i % 3
                ),
            );
        }
        let root = absolute(f.root());
        let settings =
            crate::rules::test_support::settings(&root, &[("SCR102", ""), ("REF002", "")]);
        let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
        let mut paths = live.lintable_files();
        paths.sort();
        let key = |all: &[FileDiagnostics]| -> Vec<(PathBuf, Vec<String>)> {
            all.iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        f.diagnostics
                            .iter()
                            .map(|d| {
                                format!("{} {:?} {} {:?}", d.rule.code(), d.range, d.message, d.fix)
                            })
                            .collect(),
                    )
                })
                .collect()
        };
        let whole = key(&live.check(&paths));
        assert!(paths.len() > 300 && whole.iter().any(|(_, d)| !d.is_empty()));
        let (all, rest) = live.check_until(&paths, &|| false);
        assert!(rest.is_empty());
        assert_eq!(key(&all), whole);
        for stop_after in [0, 1, 2, 4] {
            let calls = std::cell::Cell::new(0);
            let (mut parts, rest) = live.check_until(&paths, &|| {
                calls.set(calls.get() + 1);
                calls.get() > stop_after
            });
            assert_eq!(parts.len() + rest.len(), paths.len());
            parts.extend(live.check(&rest));
            assert_eq!(key(&parts), whole, "stopped after {stop_after}");
        }
    }

    /// Review round 31 (B1): after any changes on disk that nobody
    /// reports (rewrites, rewrites of the same size dated back to the old
    /// time, new and removed modules, a touch), a refresh, or a new live
    /// workspace when the refresh says discovery may have changed, gives
    /// the findings and fixes of a new live workspace read from scratch
    /// (the conservative behaviour); and every change of a file's text is
    /// seen.
    #[test]
    fn refreshing_from_disk_matches_a_new_workspace() {
        use crate::rules::script::testing::{library, with_library};
        let variants = [
            "import json\ndef f():\n    return 1\n",
            "import json\ndef f():\n    return 2\n",
            "def f():\n    return lib.util.json.dumps(1)\n",
            "def f():\n    return lib.util.json.dumps(2)\n",
            "from lib import util\n",
            "import os\ndef f():\n    return 1\n",
        ];
        let modules = ["lib/util", "m1", "m2", "b"];
        let mut f = library("lib/util", variants[0]);
        for m in &modules[1..] {
            f = with_library(f, m, variants[1]);
        }
        let root = absolute(f.root());
        let library = root.join("projects/p/ignition/script-python");
        let settings =
            || crate::rules::test_support::settings(&root, &[("SCR102", ""), ("REF002", "")]);
        let key = |live: &mut LiveWorkspace| -> Vec<(PathBuf, Vec<String>)> {
            let mut paths = live.lintable_files();
            paths.sort();
            live.check(&paths)
                .iter()
                .map(|f| {
                    (
                        f.path.clone(),
                        f.diagnostics
                            .iter()
                            .map(|d| {
                                format!("{} {:?} {} {:?}", d.rule.code(), d.range, d.message, d.fix)
                            })
                            .collect(),
                    )
                })
                .collect()
        };
        let mut live = LiveWorkspace::new(settings(), vec![root.clone()]);
        let mut rng = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % n as u64) as usize
        };
        let (mut seen_text, mut seen_structure) = (0, 0);
        for step in 0..80 {
            let m = modules[next(modules.len())];
            let code = library.join(m).join("code.py");
            let text = variants[next(variants.len())];
            let op = next(6);
            let mut text_changed = false;
            match op {
                // A new module, or one removed.
                0 => {
                    let dir = library.join(format!("n{}", next(3)));
                    if dir.exists() {
                        std::fs::remove_dir_all(&dir).unwrap();
                    } else {
                        std::fs::create_dir_all(&dir).unwrap();
                        std::fs::write(
                            dir.join("resource.json"),
                            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
                        )
                        .unwrap();
                        std::fs::write(dir.join("code.py"), text).unwrap();
                    }
                }
                // Touched only.
                1 => {
                    let file = std::fs::File::options().append(true).open(&code).unwrap();
                    file.set_modified(SystemTime::now()).unwrap();
                }
                // Rewritten with the same size, dated back.
                2 => {
                    let old = std::fs::read_to_string(&code).unwrap();
                    let modified = std::fs::metadata(&code).unwrap().modified().unwrap();
                    let same: Vec<&str> = variants
                        .iter()
                        .copied()
                        .filter(|v| v.len() == old.len() && *v != old)
                        .collect();
                    if let Some(new) = same.first() {
                        std::fs::write(&code, new).unwrap();
                        let file = std::fs::File::options().append(true).open(&code).unwrap();
                        file.set_modified(modified).unwrap();
                        text_changed = true;
                    }
                }
                _ => {
                    text_changed = std::fs::read_to_string(&code).unwrap() != text;
                    std::fs::write(&code, text).unwrap();
                }
            }
            match live.refresh_from_disk() {
                DiskChange::Structure(_) => {
                    seen_structure += 1;
                    live = LiveWorkspace::new(settings(), vec![root.clone()]);
                }
                DiskChange::Files { changed, .. } => {
                    seen_text += 1;
                    assert!(!changed.is_empty());
                }
                DiskChange::Unchanged => {
                    assert!(!text_changed, "step {step}: a change of {m} was not seen");
                }
            }
            let mut fresh = LiveWorkspace::new(settings(), vec![root.clone()]);
            assert_eq!(key(&mut live), key(&mut fresh), "step {step} (op {op})");
        }
        assert!(
            seen_text > 10 && seen_structure > 3,
            "{seen_text} {seen_structure}"
        );
    }

    /// Review round 31 (B1): files dated well before they were read are
    /// trusted by their stamps, and every change shows in them: a rewrite
    /// of another size, one of the same size dated back to the old time
    /// (the change time moves), and a file replaced by another one.
    #[test]
    fn stamps_tell_changes_once_files_settle() {
        use crate::rules::script::testing::{library, with_library};
        let f = with_library(
            library("lib/util", "import json\ndef f():\n    return 1\n"),
            "b",
            "def g():\n    return 1\n",
        );
        let root = absolute(f.root());
        let b = root.join("projects/p/ignition/script-python/b/code.py");
        std::thread::sleep(TIMESTAMP_SLACK + Duration::from_millis(200));
        let mut live = LiveWorkspace::new(
            crate::rules::test_support::settings(&root, &[("SCR102", "")]),
            vec![root.clone()],
        );
        assert!(live.files_seen.iter().all(|s| !s.racy));
        assert_eq!(live.refresh_from_disk(), DiskChange::Unchanged);
        let modified = std::fs::metadata(&b).unwrap().modified().unwrap();
        let rewrite = |text: &str, how: u8| {
            match how {
                0 => std::fs::write(&b, text).unwrap(),
                _ => {
                    let tmp = b.with_file_name("code.py.tmp");
                    std::fs::write(&tmp, text).unwrap();
                    std::fs::rename(&tmp, &b).unwrap();
                }
            }
            let file = std::fs::File::options().append(true).open(&b).unwrap();
            file.set_modified(modified).unwrap();
        };
        for (text, how) in [
            ("def g():\n    return lib.util.json.dumps(1)\n", 0),
            ("def g():\n    return lib.util.json.dumps(2)\n", 0),
            ("def g():\n    return 1\n", 1),
        ] {
            rewrite(text, how);
            match live.refresh_from_disk() {
                DiskChange::Files { changed, .. } => assert_eq!(changed, vec![b.clone()]),
                other => panic!("{text:?}: {other:?}"),
            }
            assert_eq!(&*live.source(&b).unwrap().shared_text(), text);
        }
    }

    /// Review round 32 (Codex 1): a file replaced by a link to a file with
    /// the same bytes, a link retargeted, and a new hard link elsewhere
    /// change which files are shared: the refresh says the workspace
    /// changed, whatever the bytes.
    #[cfg(unix)]
    #[test]
    fn refresh_tells_links_and_hard_links() {
        use crate::rules::script::testing::{library, with_library};
        const TEXT: &str = "import json\ndef f():\n    return 1\n";
        for change in 0..3 {
            let f = with_library(library("lib/util", TEXT), "other", TEXT);
            let root = absolute(f.root());
            let lib = root.join("projects/p/ignition/script-python");
            let other = lib.join("other/code.py");
            let mut live = LiveWorkspace::new(
                crate::rules::test_support::settings(&root, &[("SCR102", "")]),
                vec![root.clone()],
            );
            live.refresh_from_disk();
            match change {
                0 => {
                    std::fs::remove_file(&other).unwrap();
                    std::os::unix::fs::symlink("../lib/util/code.py", &other).unwrap();
                }
                1 => {
                    std::fs::hard_link(&other, root.join("elsewhere.py")).unwrap();
                }
                _ => {
                    std::fs::remove_file(&other).unwrap();
                    std::os::unix::fs::symlink("../lib/util/code.py", &other).unwrap();
                    live = LiveWorkspace::new(
                        crate::rules::test_support::settings(&root, &[("SCR102", "")]),
                        vec![root.clone()],
                    );
                    live.refresh_from_disk();
                    std::fs::write(root.join("copy.py"), TEXT).unwrap();
                    let tmp = lib.join("other/tmp");
                    std::os::unix::fs::symlink("../../../../../copy.py", &tmp).unwrap();
                    std::fs::rename(&tmp, &other).unwrap();
                }
            }
            assert!(
                matches!(live.refresh_from_disk(), DiskChange::Structure(_)),
                "change {change}"
            );
        }
    }

    /// Review round 17 (Opus): a file of a project that the workspace
    /// walk did not reach (below a hidden directory of the root) is linted
    /// on its own, gets no fix, and `REF009` says why, without claiming
    /// that the gateway does not load it.
    #[test]
    fn files_of_projects_the_walk_did_not_reach_get_no_fixes() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute(dir.path());
        let project = root.join(".hidden/projects/P");
        let module = project.join("ignition/script-python/m");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(project.join("project.json"), r#"{"title": "P"}"#).unwrap();
        std::fs::write(
            module.join("resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        )
        .unwrap();
        let code = module.join("code.py");
        std::fs::write(&code, "x = 1   \n").unwrap();
        let mut rules = std::collections::BTreeMap::new();
        for meta in [
            &crate::rules::script::trailing_whitespace::META,
            &crate::rules::refs::manifest_mismatch::META,
        ] {
            rules.insert(
                RuleCode::new(meta),
                RuleSettings {
                    severity: crate::Severity::Warning,
                    options: Default::default(),
                },
            );
        }
        let settings = ProjectSettings {
            root: root.clone(),
            base: Arc::new(LinterSettings {
                rules,
                strict: false,
            }),
            ..ProjectSettings::default()
        };
        let mut live = LiveWorkspace::new(settings, vec![root.clone()]);
        assert!(!live.reaches(&code));
        let found = live
            .check(std::slice::from_ref(&code))
            .remove(0)
            .diagnostics;
        let trailing = found
            .iter()
            .find(|d| d.rule.code() == "SCR124")
            .unwrap_or_else(|| panic!("{found:#?}"));
        assert!(trailing.fix.is_none(), "{trailing:#?}");
        let ref009: Vec<&str> = found
            .iter()
            .filter(|d| d.rule.code() == "REF009")
            .map(|d| d.message.as_str())
            .collect();
        assert_eq!(ref009.len(), 1, "{found:#?}");
        assert!(
            ref009[0].contains("but the workspace walk did not reach it")
                && ref009[0].contains("offers no fixes for it")
                && !ref009[0].contains("does not load"),
            "{ref009:?}"
        );
    }

    #[test]
    fn suppressions_of_embedded_scripts_take_effect() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute(dir.path());
        let views = root.join("projects/p/com.inductiveautomation.perspective/views/V");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(root.join("projects/p/project.json"), r#"{"title": "p"}"#).unwrap();
        std::fs::write(
            views.join("resource.json"),
            r#"{"scope": "G", "files": ["view.json"]}"#,
        )
        .unwrap();
        let view = views.join("view.json");
        let text = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
 "events": {"dom": {"onClick": {"type": "script", "config": {"script": "\tx = 1\n\ty = bad\t# note"}}}}}}"#;
        std::fs::write(&view, text).unwrap();
        let mut rules = std::collections::BTreeMap::new();
        rules.insert(
            RuleCode::new(&BAD_NAME),
            RuleSettings {
                severity: crate::Severity::Warning,
                options: Default::default(),
            },
        );
        let settings = ProjectSettings {
            root: root.clone(),
            base: Arc::new(LinterSettings {
                rules,
                strict: false,
            }),
            ..ProjectSettings::default()
        };
        let mut live = LiveWorkspace::with_rules(TEST_RULES, settings, vec![root.clone()]);
        let found = live
            .check(std::slice::from_ref(&view))
            .remove(0)
            .diagnostics;
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].range.slice(text), "bad");
        assert!(live.suppressible(found[0].rule));
        let edit = live.suppression_edit(&found[0]).expect("an edit");
        let new = iglint_text::apply_edits(text, &[edit.edit]).unwrap();
        assert!(
            new.contains(r#"\ty = bad\t# iglint: ignore[SCR900]  # note"}"#),
            "{new}"
        );
        live.set_text(&view, Some(Arc::from(new)));
        assert!(
            live.check(std::slice::from_ref(&view))[0]
                .diagnostics
                .is_empty()
        );
    }

    fn insert(source: &str, at: usize, code: &str) -> Option<String> {
        let (offset, text) = suppression_insert(source, TextSize::new(at as u32), code)?;
        let mut out = source.to_owned();
        out.insert_str(offset.to_usize(), &text);
        Some(out)
    }

    #[test]
    fn suppression_at_line_end() {
        assert_eq!(
            insert("a = 1\nb = c  \nd = 2", 8, "SCR101").as_deref(),
            Some("a = 1\nb = c  # iglint: ignore[SCR101]  \nd = 2")
        );
    }

    #[test]
    fn suppression_extends_existing_marker() {
        assert_eq!(
            insert("b = c  # iglint: ignore[SCR1]\n", 4, "BIND001").as_deref(),
            Some("b = c  # iglint: ignore[SCR1, BIND001]\n")
        );
        assert_eq!(insert("b = c  # iglint: ignore\n", 4, "BIND001"), None);
    }

    #[test]
    fn suppression_before_other_comment() {
        assert_eq!(
            insert("b = c  # note\n", 0, "SCR101").as_deref(),
            Some("b = c  # iglint: ignore[SCR101]  # note\n")
        );
    }

    #[test]
    fn no_suppression_inside_strings() {
        assert_eq!(insert("x = '''abc\ndef'''\n", 2, "SCR101"), None);
    }
}
