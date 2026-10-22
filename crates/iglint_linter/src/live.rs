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

use iglint_project::index::ProjectIndex;
use iglint_project::model::{LoadedFile, LoadedResource, ModelData};
use iglint_project::summary::Facts;
use iglint_project::{
    Container, FileId, JsonDocument, LibraryLookup, ProjectId, ResourceId, ResourceKind,
    ResourceSummary, Workspace, build_model, summarize,
};
use iglint_text::{TextEdit, TextRange, TextSize};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analyze::{
    add_script_references, is_json, primary_file, read_text, run_phase, shared_text,
    standalone_model,
};
use crate::checker::{Checker, FileTable, Unit, UnitCache};
use crate::diagnostic::{Diagnostic, FileEdit, SourceFile};
use crate::discovery::{self, absolute};
use crate::engine::{CheckOptions, FixMode, fix_file};
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
    /// Editor text of files outside the workspace.
    standalone: FxHashMap<PathBuf, Arc<str>>,
}

impl LiveWorkspace {
    /// Discovers the projects and gateway collections below `roots` and
    /// summarises every resource.
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
        let ws = Workspace::discover(&roots);
        let mut table = FileTable::with_len(ws.files.len());
        let mut read_errors = FxHashMap::default();
        for r in &ws.resources {
            let doc = &r.manifest.doc;
            table.insert(
                r.manifest.file,
                SourceFile::new(ws.file(r.manifest.file).path.clone(), Arc::clone(doc.shared_text())),
            );
        }
        for p in &ws.projects {
            let doc = &p.manifest.doc;
            table.insert(
                p.manifest.file,
                SourceFile::new(ws.file(p.manifest.file).path.clone(), Arc::clone(doc.shared_text())),
            );
        }
        for c in &ws.collections {
            let doc = &c.manifest.doc;
            table.insert(
                c.manifest.file,
                SourceFile::new(ws.file(c.manifest.file).path.clone(), Arc::clone(doc.shared_text())),
            );
        }
        type Loaded = (ResourceSummary, Vec<(FileId, Arc<str>)>, Vec<(FileId, String)>);
        let loaded: Vec<Loaded> = ws
            .resources
            .par_iter()
            .map(|r| {
                let loaded = ws.load_resource(r.id);
                let texts = loaded
                    .files
                    .iter()
                    .filter_map(|f| shared_text(f).map(|t| (f.file, t)))
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
            for (file, text) in texts {
                table.insert(file, SourceFile::new(ws.file(file).path.clone(), text));
            }
            read_errors.extend(errors);
        }
        let by_path = ws.files.iter().map(|f| (f.path.clone(), f.id)).collect();
        LiveWorkspace {
            settings,
            roots,
            ws,
            rules,
            sets: RuleSetCache::default(),
            table,
            read_errors,
            summaries,
            by_path,
            standalone: FxHashMap::default(),
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

    /// Whether `path` is excluded from linting by the settings.
    pub fn is_skipped(&self, path: &Path) -> bool {
        discovery::skip_reason(&self.settings, path).is_some()
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
        read_text(&path).ok().map(|t| SourceFile::new(path, t))
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
            Some(text) => Ok(text),
            None => read_text(&path),
        };
        match text {
            Ok(text) => {
                if self
                    .table
                    .get(file)
                    .is_some_and(|s| *s.shared_text() == *text)
                {
                    return Dependents::None;
                }
                self.read_errors.remove(&file);
                self.table.insert(file, SourceFile::new(path, text));
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
        let index = ProjectIndex::build(&self.ws, summaries);
        let dependents = dependents(&self.ws, &index, rid, &old);
        self.summaries = index.into_summaries();
        dependents
    }

    /// Findings of `paths`, in the order given.
    pub fn check(&mut self, paths: &[PathBuf]) -> Vec<FileDiagnostics> {
        let summaries = std::mem::take(&mut self.summaries);
        let index = ProjectIndex::build(&self.ws, summaries);
        let result = self.check_with(&index, paths);
        self.summaries = index.into_summaries();
        result
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
            .flat_map_iter(|r| self.analyze_resource(index, &self.table, *r))
            .collect();
        for p in projects {
            found.extend(self.analyze_project(index, p));
        }
        let mut standalone_found: FxHashMap<&Path, Result<Vec<Diagnostic>, String>> = standalone
            .par_iter()
            .map(|path| {
                let text = match self.standalone.get(*path) {
                    Some(text) => Ok(Arc::clone(text)),
                    None => read_text(path),
                };
                (*path, text.map(|t| self.analyze_standalone(index, path, t)))
            })
            .collect();

        let mut by_file: FxHashMap<PathBuf, Vec<Diagnostic>> = FxHashMap::default();
        for d in found {
            by_file.entry(d.file.path().to_path_buf()).or_default().push(d);
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
                        if !self.reported_with_own_settings(&d) {
                            d.severity = rule.severity;
                        }
                        Some(d)
                    })
                    .collect();
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

    /// The file whose settings a resource's unit runs with: its first data
    /// file that is linted, else its manifest.
    fn unit_settings_path(&self, rid: ResourceId) -> Option<PathBuf> {
        let r = self.ws.resource(rid);
        r.files
            .iter()
            .filter(|f| **f != r.manifest.file)
            .chain(std::iter::once(&r.manifest.file))
            .map(|f| &self.ws.file(*f).path)
            .find(|p| !self.is_skipped(p))
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
                }
            })
            .collect();
        LoadedResource {
            resource: rid,
            files,
        }
    }

    /// Both phases on one resource.
    fn analyze_resource(
        &self,
        index: &ProjectIndex<'_>,
        table: &FileTable,
        rid: ResourceId,
    ) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let Some(settings) = self.unit_settings(rid) else {
            return out;
        };
        let set = self.sets.get(self.rules, &settings);
        if set.is_empty() {
            return out;
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
        let cache = UnitCache::default();
        let unit = Unit {
            ws: &self.ws,
            resource: Some(model.resource),
            project: project_of(model.resource.container),
            data: &model.data,
            errors: &model.errors,
            primary: primary_file(&model),
            files: table,
            strict: settings.strict,
            cache: &cache,
        };
        run_phase(&unit, None, &set, &set.local, &mut out);
        run_phase(&unit, Some(index), &set, &set.project, &mut out);
        out
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
        out
    }

    /// A file outside the workspace, analysed on its own.
    fn analyze_standalone(
        &self,
        index: &ProjectIndex<'_>,
        path: &Path,
        text: Arc<str>,
    ) -> Vec<Diagnostic> {
        let settings = self.settings.for_file(path);
        let set = self.sets.get(self.rules, &settings);
        let mut out = Vec::new();
        if set.is_empty() {
            return out;
        }
        let file = FileId(self.ws.files.len() as u32);
        let mut table = FileTable::with_len(0);
        table.insert(file, SourceFile::new(path, Arc::clone(&text)));
        let doc = is_json(path).then(|| JsonDocument::parse(Arc::clone(&text)));
        let (data, errors) = standalone_model(path, file, doc.as_ref(), &text);
        let cache = UnitCache::default();
        let unit = Unit {
            ws: &self.ws,
            resource: None,
            project: None,
            data: &data,
            errors: &errors,
            primary: file,
            files: &table,
            strict: settings.strict,
            cache: &cache,
        };
        run_phase(&unit, None, &set, &set.local, &mut out);
        run_phase(&unit, Some(index), &set, &set.project, &mut out);
        out
    }

    /// Applies every fix of `path` (safe ones, and unsafe ones when asked)
    /// in passes, re-analysing in between. Returns `None` when nothing
    /// changes. The workspace is not modified.
    pub fn fix_all(&mut self, path: &Path, unsafe_fixes: bool) -> Option<FixAll> {
        let path = absolute(path);
        let source = self.source(&path)?;
        let diagnostics = self
            .check(std::slice::from_ref(&path))
            .pop()
            .map(|f| f.diagnostics)
            .unwrap_or_default();
        if diagnostics.iter().all(|d| d.fix.is_none()) {
            return None;
        }
        let summaries = std::mem::take(&mut self.summaries);
        let index = ProjectIndex::build(&self.ws, summaries);
        let options = CheckOptions {
            fix_mode: FixMode::DiffOnly { unsafe_fixes },
            stdin: Some((path.clone(), String::new())),
            ..CheckOptions::default()
        };
        let reanalyze = |text: &str| -> Vec<Diagnostic> {
            let text: Arc<str> = Arc::from(text);
            match self.by_path.get(&path) {
                Some(f) => {
                    let Some(rid) = self.ws.file(*f).resource else {
                        return Vec::new();
                    };
                    let mut table = self.table.clone();
                    table.insert(*f, SourceFile::new(path.clone(), text));
                    let settings = self.settings.for_file(&path);
                    self.analyze_resource(&index, &table, rid)
                        .into_iter()
                        .filter(|d| d.file.path() == path && settings.is_enabled(d.rule))
                        .collect()
                }
                None => self.analyze_standalone(&index, &path, text),
            }
        };
        let result = fix_file(
            &self.settings,
            &options,
            &path,
            &source.shared_text(),
            diagnostics,
            &reanalyze,
        );
        self.summaries = index.into_summaries();
        let (summary, _) = result.ok()?;
        let summary = summary?;
        summary.changed().then(|| FixAll {
            applied: summary.applied.len(),
            fixed: summary.fixed,
        })
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
    let at = offset.to_usize().min(source.len());
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let mut line_end = source[at..].find('\n').map_or(source.len(), |i| at + i);
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
                _ => (
                    c.start().to_usize(),
                    format!("# iglint: ignore[{code}]  "),
                ),
            }
        }
        None => {
            let trimmed = source[line_start..line_end].trim_end().len() + line_start;
            (trimmed, format!("  # iglint: ignore[{code}]"))
        }
    };
    let mut new = String::with_capacity(source.len() + insert.len());
    new.push_str(&source[..insert_at]);
    new.push_str(&insert);
    new.push_str(&source[insert_at..]);
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

fn is_manifest_kind(kind: iglint_project::FileKind) -> bool {
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
    let model = build_model(ws, loaded);
    let mut summary = summarize(&model);
    if crate::script_facts::provider().provides_references() {
        add_script_references(&model.data, &UnitCache::default(), &mut summary);
    }
    summary
}

/// Resources whose findings may change because the summary of `changed`
/// went from `old` to its current one in `index`.
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
            | (Facts::StyleClass(_), Facts::StyleClass(_))
            | (Facts::Library(_), Facts::Library(_)) => true,
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
        if old.references.messages != new.references.messages {
            return Dependents::All;
        }
        if let Some(project) = project_of(resource.container) {
            let projects: Vec<ProjectId> = std::iter::once(project)
                .chain(ws.project_descendants(project))
                .collect();
            for refs in [&old.references, &new.references] {
                for p in &projects {
                    targets(index, *p, refs, &mut out);
                }
            }
        }
    }
    out.remove(&changed);
    Dependents::from_resources(ws, out)
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
        ResourceKind::ScriptLibrary => refs
            .library
            .iter()
            .any(|l| library_target(index, project, l) == Some(target)),
        _ => false,
    }
}

/// Adds the resources `refs` resolve to from `project`.
fn targets(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    refs: &iglint_project::summary::References,
    out: &mut FxHashSet<ResourceId>,
) {
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
    out.extend(
        refs.library
            .iter()
            .filter_map(|l| library_target(index, project, l)),
    );
}

fn library_target(index: &ProjectIndex<'_>, project: ProjectId, name: &str) -> Option<ResourceId> {
    match index.resolve_library(project, name) {
        LibraryLookup::Symbol { resource, .. }
        | LibraryLookup::Module { resource, .. }
        | LibraryLookup::MissingAttribute { resource, .. } => Some(resource),
        _ => None,
    }
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
            for (i, _) in source[body..].match_indices("bad") {
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

    #[test]
    fn suppressions_of_embedded_scripts_take_effect() {
        let dir = tempfile::tempdir().unwrap();
        let root = absolute(dir.path());
        let views = root.join("projects/p/com.inductiveautomation.perspective/views/V");
        std::fs::create_dir_all(&views).unwrap();
        std::fs::write(root.join("projects/p/project.json"), r#"{"title": "p"}"#).unwrap();
        std::fs::write(views.join("resource.json"), r#"{"scope": "G", "files": ["view.json"]}"#)
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
        let found = live.check(std::slice::from_ref(&view)).remove(0).diagnostics;
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].range.slice(text), "bad");
        assert!(live.suppressible(found[0].rule));
        let edit = live.suppression_edit(&found[0]).expect("an edit");
        let new = iglint_text::apply_edits(text, &[edit.edit]).unwrap();
        assert!(new.contains(r#"\ty = bad\t# iglint: ignore[SCR900]  # note"}"#), "{new}");
        live.set_text(&view, Some(Arc::from(new)));
        assert!(live.check(std::slice::from_ref(&view))[0].diagnostics.is_empty());
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
