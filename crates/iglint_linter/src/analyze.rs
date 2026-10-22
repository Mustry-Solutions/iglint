//! The analysis pipeline.
//!
//! 1. **Workspace.** The projects and gateway collections around the linted
//!    paths are discovered ([`Workspace::discover`]); every one of their
//!    resources is context for cross-resource checks, but only files in the
//!    lint set get findings.
//! 2. **Phase A, per resource, in parallel.** Each resource's files are read
//!    and parsed once ([`Workspace::load_resource`]), modelled
//!    ([`build_model`]) and summarised ([`summarize`]); rules in
//!    [`Phase::Local`] run on linted resources in the same pass.
//! 3. **Index.** [`ProjectIndex::build`] runs once over all summaries.
//! 4. **pylint.** When `SCR900 pylint` is enabled, pylint runs once over
//!    the scripts of every linted unit ([`pylint_batch`]); its messages are
//!    stored in the units' caches and reported by the rule in phase B.
//! 5. **Phase B, per resource, in parallel.** Rules in [`Phase::Project`]
//!    run with the index on the same models (kept alive since phase A), and
//!    project rules once per linted project.
//! 6. **Fixes.** Files with fixes are fixed and re-analysed one at a time,
//!    against the index of the original text. pylint does not run again:
//!    its findings are moved through the applied edits (dropped where an
//!    edit touches them).
//!
//! Files in the lint set that belong to no resource (a `view.json` outside a
//! project, a loose Python file, stdin) are analysed standalone: the same
//! phases with no project, so cross-resource rules stay silent.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iglint_json::Value;
use iglint_project::EmbeddedScript;
use iglint_project::library::LibraryModule;
use iglint_project::model::{FileError as ModelFileError, LoadedFile, LoadedResource, ModelData};
use iglint_project::script::library_script;
use iglint_project::{
    Container, FileId, JsonDocument, ProjectIndex, ResourceId, ResourceModel, ResourceSummary,
    Workspace, build_model, build_view, summarize,
};
use iglint_text::TextEdit;
use iglint_text::TextRange;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use crate::cache::Cache;
use crate::checker::{Checker, FileTable, Unit, UnitCache};
use crate::diagnostic::{Diagnostic, SourceFile};
use crate::discovery::{FileKind, absolute};
use crate::engine::{CheckOptions, CheckedFile, FileError, Outcome, fix_file};
use crate::registry::RuleCode;
use crate::rule::{PhaseHooks, RuleImpl, RuleSet, RuleSetCache, ScriptUnit};
use crate::rules::script::pylint;
use crate::settings::{LinterSettings, ProjectSettings};
use crate::suppression::Suppressions;

/// A file to lint.
pub(crate) struct Target {
    /// The path as given or discovered, used in output.
    pub path: PathBuf,
    pub kind: FileKind,
    /// Text that replaces the file on disk (stdin).
    pub stdin: Option<Arc<str>>,
    pub settings: Arc<LinterSettings>,
}

/// Everything the run produced.
pub(crate) struct Analysis {
    pub outcomes: Vec<Result<Outcome, FileError>>,
    /// Problems not tied to one linted file (invalid rule options).
    pub errors: Vec<FileError>,
    /// Warnings about the run that do not fail it (pylint unavailable).
    pub notices: Vec<String>,
}

/// Runs every rule of [`crate::rules::IMPLS`] on `targets`.
pub(crate) fn analyze(
    settings: &ProjectSettings,
    roots: &[PathBuf],
    targets: Vec<Target>,
    options: &CheckOptions,
    cache: Option<&Cache>,
) -> Analysis {
    analyze_with(
        crate::rules::IMPLS,
        settings,
        roots,
        targets,
        options,
        cache,
    )
}

/// Like [`analyze`] with an explicit rule list (tests).
pub(crate) fn analyze_with(
    rules: &'static [&'static RuleImpl],
    settings: &ProjectSettings,
    roots: &[PathBuf],
    targets: Vec<Target>,
    options: &CheckOptions,
    cache: Option<&Cache>,
) -> Analysis {
    let sets = RuleSetCache::default();
    let by_abs: FxHashMap<PathBuf, usize> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| (absolute(&t.path), i))
        .collect();
    let ws_roots: Vec<PathBuf> = if roots.is_empty() || targets.iter().any(|t| t.stdin.is_some()) {
        targets.iter().map(|t| t.path.clone()).collect()
    } else {
        roots.to_vec()
    };
    let ws = Workspace::discover(&ws_roots);

    // Which workspace file is which target.
    let target_of: Vec<Option<usize>> = ws
        .files
        .iter()
        .map(|f| by_abs.get(&absolute(&f.path)).copied())
        .collect();
    let mut in_workspace = vec![false; targets.len()];
    for t in target_of.iter().flatten() {
        in_workspace[*t] = true;
    }
    let mut overrides: FxHashMap<FileId, Arc<str>> = FxHashMap::default();
    for (i, t) in target_of.iter().enumerate() {
        if let Some(t) = t
            && let Some(text) = &targets[*t].stdin
        {
            overrides.insert(FileId(i as u32), Arc::clone(text));
        }
    }
    let linted: Vec<bool> = ws
        .resources
        .iter()
        .map(|r| r.files.iter().any(|f| target_of[f.index()].is_some()))
        .collect();

    // Phase A, part 1: read and parse every resource.
    let loaded: Vec<LoadedResource> = ws
        .resources
        .par_iter()
        .map(|r| load(&ws, r.id, &overrides))
        .collect();

    // Standalone targets: read them now so the file table is complete.
    let standalone: Vec<(usize, Result<Arc<str>, String>)> = (0..targets.len())
        .filter(|i| !in_workspace[*i])
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|i| {
            let t = &targets[i];
            let text = match &t.stdin {
                Some(text) => Ok(Arc::clone(text)),
                None => read_text(&t.path),
            };
            (i, text)
        })
        .collect();

    // The file table: every file a finding may point at.
    let display = |id: FileId| -> PathBuf {
        match target_of[id.index()] {
            Some(t) => targets[t].path.clone(),
            None => ws.file(id).path.clone(),
        }
    };
    let mut table = FileTable::with_len(ws.files.len() + standalone.len());
    for l in &loaded {
        for f in &l.files {
            if let Some(text) = shared_text(f) {
                table.insert(f.file, SourceFile::new(display(f.file), text));
            }
        }
    }
    for r in &ws.resources {
        let doc = &r.manifest.doc;
        table.insert(
            r.manifest.file,
            SourceFile::new(display(r.manifest.file), Arc::clone(doc.shared_text())),
        );
    }
    for p in &ws.projects {
        let doc = &p.manifest.doc;
        table.insert(
            p.manifest.file,
            SourceFile::new(display(p.manifest.file), Arc::clone(doc.shared_text())),
        );
    }
    for c in &ws.collections {
        let doc = &c.manifest.doc;
        table.insert(
            c.manifest.file,
            SourceFile::new(display(c.manifest.file), Arc::clone(doc.shared_text())),
        );
    }
    let mut errors_by_target: FxHashMap<usize, String> = FxHashMap::default();
    // Linted files the model does not read (binary-classified data files,
    // files outside resources): read them so they still get file rules.
    let unread: Vec<(FileId, usize)> = target_of
        .iter()
        .enumerate()
        .filter_map(|(f, t)| t.map(|t| (FileId(f as u32), t)))
        .filter(|(f, _)| table.get(*f).is_none())
        .collect();
    let unread_texts: Vec<(FileId, usize, Result<Arc<str>, String>)> = unread
        .into_par_iter()
        .map(|(f, t)| {
            let text = match &targets[t].stdin {
                Some(text) => Ok(Arc::clone(text)),
                None => read_text(&targets[t].path),
            };
            (f, t, text)
        })
        .collect();
    for (f, t, text) in unread_texts {
        match text {
            Ok(text) => table.insert(f, SourceFile::new(targets[t].path.clone(), text)),
            Err(message) => {
                errors_by_target.insert(t, message);
            }
        }
    }
    let mut standalone_files: Vec<(usize, FileId, Arc<str>)> = Vec::new();
    for (k, (t, text)) in standalone.into_iter().enumerate() {
        match text {
            Ok(text) => {
                let id = FileId((ws.files.len() + k) as u32);
                table.insert(
                    id,
                    SourceFile::new(targets[t].path.clone(), Arc::clone(&text)),
                );
                standalone_files.push((t, id, text));
            }
            Err(message) => {
                errors_by_target.insert(t, message);
            }
        }
    }
    for l in &loaded {
        for f in &l.files {
            if let (Some(t), Some(message)) = (target_of[f.file.index()], &f.error) {
                errors_by_target.insert(t, message.clone());
            }
        }
    }

    // Cache: results depend on every file of the workspace (and on how
    // pylint runs).
    let context =
        cache.map(|_| context_hash(&table) ^ pylint_context(rules, &sets, &targets, settings));

    // Phase A, part 2: model, summarise and run local rules.
    let models: Vec<ResourceModel<'_>> = loaded.par_iter().map(|l| build_model(&ws, l)).collect();
    let caches: Vec<UnitCache> = (0..models.len()).map(|_| UnitCache::default()).collect();
    let facts = crate::script_facts::provider();
    let unit_settings = |rid: ResourceId| -> Option<(usize, Arc<LinterSettings>)> {
        let r = ws.resource(rid);
        let t = r
            .files
            .iter()
            .filter(|f| **f != r.manifest.file)
            .chain(std::iter::once(&r.manifest.file))
            .find_map(|f| target_of[f.index()])?;
        Some((t, Arc::clone(&targets[t].settings)))
    };
    let project_of = |c: Container| match c {
        Container::Project(p) => Some(p),
        Container::Collection(_) => None,
    };
    let strict_of = |s: &LinterSettings| s.strict;

    struct UnitResult {
        diagnostics: Vec<Diagnostic>,
        time: Duration,
        cached: bool,
    }
    let cached_unit = |rid: ResourceId| -> Option<Vec<Diagnostic>> {
        let (cache, context) = (cache?, context?);
        let r = ws.resource(rid);
        let mut out = Vec::new();
        for f in &r.files {
            let Some(t) = target_of[f.index()] else {
                continue;
            };
            let source = table.get(*f)?;
            out.extend(cache.get(
                &targets[t].path,
                targets[t].kind,
                &targets[t].settings,
                source,
                context,
            )?);
        }
        Some(out)
    };

    let project_names: Vec<&str> = ws.projects.iter().map(|p| p.name.as_str()).collect();
    let phase_a: Vec<(ResourceSummary, Option<UnitResult>)> = models
        .par_iter()
        .map(|model| {
            let mut summary = summarize(model);
            let rid = model.resource.id;
            if facts.provides_references() {
                let project =
                    project_of(model.resource.container).map(|p| ws.project(p).name.as_str());
                let module = match &model.data {
                    ModelData::Library(l) => Some(l.module_path.as_str()),
                    _ => None,
                };
                let ctx = crate::script_facts::FactsContext {
                    project,
                    projects: &project_names,
                    module,
                };
                add_script_references(&model.data, &caches[rid.index()], &ctx, &mut summary);
            }
            if !linted[rid.index()] {
                return (summary, None);
            }
            if let Some(diagnostics) = cached_unit(rid) {
                return (
                    summary,
                    Some(UnitResult {
                        diagnostics,
                        time: Duration::ZERO,
                        cached: true,
                    }),
                );
            }
            let start = Instant::now();
            let mut diagnostics = Vec::new();
            if let Some((_, s)) = unit_settings(rid) {
                let set = sets.get(rules, &s);
                if !set.is_empty() {
                    let unit = Unit {
                        ws: &ws,
                        resource: Some(model.resource),
                        project: project_of(model.resource.container),
                        data: &model.data,
                        errors: &model.errors,
                        primary: primary_file(model),
                        files: &table,
                        strict: strict_of(&s),
                        cache: &caches[rid.index()],
                    };
                    run_phase(&unit, None, &set, &set.local, &mut diagnostics);
                }
            }
            (
                summary,
                Some(UnitResult {
                    diagnostics,
                    time: start.elapsed(),
                    cached: false,
                }),
            )
        })
        .collect();
    let (summaries, mut unit_results): (Vec<ResourceSummary>, Vec<Option<UnitResult>>) =
        phase_a.into_iter().unzip();

    // Index.
    let index = ProjectIndex::build(&ws, summaries);

    // Files outside any resource, modelled standalone.
    struct Standalone {
        target: usize,
        file: FileId,
        doc: Option<JsonDocument>,
        text: Arc<str>,
    }
    let standalone_docs: Vec<Standalone> = standalone_files
        .into_iter()
        .map(|(target, file, text)| Standalone {
            target,
            file,
            doc: is_json(&targets[target].path).then(|| JsonDocument::parse(Arc::clone(&text))),
            text,
        })
        .collect();
    let standalone_models: Vec<(ModelData<'_>, Vec<ModelFileError>, UnitCache)> = standalone_docs
        .iter()
        .map(|s| {
            let (data, errors) =
                standalone_model(&targets[s.target].path, s.file, s.doc.as_ref(), &s.text);
            (data, errors, UnitCache::default())
        })
        .collect();

    // pylint, once for every linted unit that enables SCR900.
    let mut batch: Vec<BatchUnit<'_, '_>> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        if !unit_results[i].as_ref().is_some_and(|u| !u.cached) {
            continue;
        }
        let Some((_, s)) = unit_settings(model.resource.id) else {
            continue;
        };
        let set = sets.get(rules, &s);
        if pylint::options_in(&set).is_none() {
            continue;
        }
        let scripts = model
            .data
            .scripts()
            .into_iter()
            .enumerate()
            .filter_map(|(position, (file, script))| {
                let t = target_of.get(file.index()).copied().flatten()?;
                Some((position, t, script))
            })
            .collect();
        batch.push(BatchUnit {
            set,
            cache: &caches[i],
            scripts,
        });
    }
    for (s, (data, _, cache)) in standalone_docs.iter().zip(&standalone_models) {
        let set = sets.get(rules, &targets[s.target].settings);
        if pylint::options_in(&set).is_none() {
            continue;
        }
        let scripts = data
            .scripts()
            .into_iter()
            .enumerate()
            .map(|(position, (_, script))| (position, s.target, script))
            .collect();
        batch.push(BatchUnit {
            set,
            cache,
            scripts,
        });
    }
    let (pylint_failed, notices) = pylint_batch(&batch, &settings.root);
    drop(batch);

    // Phase B.
    let phase_b: Vec<(usize, Vec<Diagnostic>, Duration)> = models
        .par_iter()
        .enumerate()
        .filter(|(i, _)| unit_results[*i].as_ref().is_some_and(|u| !u.cached))
        .map(|(i, model)| {
            let start = Instant::now();
            let mut diagnostics = Vec::new();
            if let Some((_, s)) = unit_settings(model.resource.id) {
                let set = sets.get(rules, &s);
                if !set.project.is_empty() {
                    let unit = Unit {
                        ws: &ws,
                        resource: Some(model.resource),
                        project: project_of(model.resource.container),
                        data: &model.data,
                        errors: &model.errors,
                        primary: primary_file(model),
                        files: &table,
                        strict: strict_of(&s),
                        cache: &caches[i],
                    };
                    run_phase(&unit, Some(&index), &set, &set.project, &mut diagnostics);
                }
            }
            (i, diagnostics, start.elapsed())
        })
        .collect();
    for (i, diagnostics, time) in phase_b {
        if let Some(u) = &mut unit_results[i] {
            u.diagnostics.extend(diagnostics);
            u.time += time;
        }
    }

    // Project rules, once per linted project (reported on `project.json`).
    let none = ModelData::None;
    let project_cache = UnitCache::default();
    let mut project_diagnostics = Vec::new();
    for p in &ws.projects {
        let manifest = p.manifest.file;
        let settings_target = target_of[manifest.index()].or_else(|| {
            p.resources
                .iter()
                .find_map(|r| unit_settings(*r).map(|(t, _)| t))
        });
        let Some(t) = settings_target else {
            continue;
        };
        let set = sets.get(rules, &targets[t].settings);
        if set.project.project.is_empty() {
            continue;
        }
        let unit = Unit {
            ws: &ws,
            resource: None,
            project: Some(p.id),
            data: &none,
            errors: &[],
            primary: manifest,
            files: &table,
            strict: targets[t].settings.strict,
            cache: &project_cache,
        };
        for (slot, rule) in &set.project.project {
            let mut cx = Checker::new(
                &unit,
                Some(&index),
                &set.active[*slot],
                &mut project_diagnostics,
            );
            rule.check_project(&mut cx, p.id);
        }
    }

    // Standalone files.
    let standalone_results: Vec<(usize, Vec<Diagnostic>, Duration)> = standalone_docs
        .par_iter()
        .zip(&standalone_models)
        .map(|(s, (data, errors, cache))| {
            let start = Instant::now();
            let t = &targets[s.target];
            let mut diagnostics = Vec::new();
            let set = sets.get(rules, &t.settings);
            if !set.is_empty() {
                let unit = Unit {
                    ws: &ws,
                    resource: None,
                    project: None,
                    data,
                    errors,
                    primary: s.file,
                    files: &table,
                    strict: t.settings.strict,
                    cache,
                };
                run_phase(&unit, None, &set, &set.local, &mut diagnostics);
                run_phase(&unit, Some(&index), &set, &set.project, &mut diagnostics);
            }
            (s.target, diagnostics, start.elapsed())
        })
        .collect();

    // Distribute findings to their files.
    let by_display: FxHashMap<&Path, usize> = targets
        .iter()
        .enumerate()
        .map(|(i, t)| (t.path.as_path(), i))
        .collect();
    let mut per_target: Vec<Vec<Diagnostic>> = (0..targets.len()).map(|_| Vec::new()).collect();
    let mut time: Vec<Duration> = vec![Duration::ZERO; targets.len()];
    let mut cached = vec![false; targets.len()];
    let place = |d: Diagnostic, origin: Option<usize>, per_target: &mut Vec<Vec<Diagnostic>>| {
        let Some(&t) = by_display.get(d.file.path()) else {
            return;
        };
        let Some(rule_settings) = targets[t].settings.rule(d.rule) else {
            return;
        };
        let mut d = d;
        if origin != Some(t) {
            d.severity = rule_settings.severity;
        }
        per_target[t].push(d);
    };
    for (i, result) in unit_results.into_iter().enumerate() {
        let Some(result) = result else {
            continue;
        };
        let origin = unit_settings(ResourceId(i as u32)).map(|(t, _)| t);
        if let Some(t) = origin {
            time[t] += result.time;
            cached[t] = result.cached;
        }
        for d in result.diagnostics {
            place(d, origin, &mut per_target);
        }
    }
    for d in project_diagnostics {
        place(d, None, &mut per_target);
    }
    for (t, diagnostics, elapsed) in standalone_results {
        time[t] += elapsed;
        for d in diagnostics {
            place(d, Some(t), &mut per_target);
        }
    }
    let mut errors: Vec<FileError> = sets
        .errors()
        .into_iter()
        .map(|(rule, message)| FileError {
            path: settings.root.clone(),
            message: format!("invalid options for {rule}: {message}"),
        })
        .collect();
    errors.sort_by(|a, b| a.message.cmp(&b.message));

    // Fixes and outcomes, per target.
    let standalone_by_target: FxHashMap<usize, &Standalone> =
        standalone_docs.iter().map(|s| (s.target, s)).collect();
    let file_of_target: FxHashMap<usize, FileId> = target_of
        .iter()
        .enumerate()
        .filter_map(|(f, t)| t.map(|t| (t, FileId(f as u32))))
        .collect();
    let outcomes: Vec<Result<Outcome, FileError>> = per_target
        .into_par_iter()
        .enumerate()
        .map(|(t, diagnostics)| {
            let target = &targets[t];
            if let Some(message) = errors_by_target.get(&t) {
                return Err(FileError {
                    path: target.path.clone(),
                    message: message.clone(),
                });
            }
            let source = match (file_of_target.get(&t), standalone_by_target.get(&t)) {
                (Some(f), _) => table.get(*f).cloned(),
                (None, Some(s)) => table.get(s.file).cloned(),
                _ => None,
            };
            let Some(source) = source else {
                return Err(FileError {
                    path: target.path.clone(),
                    message: "file could not be read".to_owned(),
                });
            };
            let cache_entry = match (cache, context, cached[t] || pylint_failed.contains(&t)) {
                (Some(c), Some(ctx), false) => c.entry(
                    &target.path,
                    target.kind,
                    &target.settings,
                    &source,
                    ctx,
                    &diagnostics,
                ),
                _ => None,
            };
            // pylint findings are carried through the fixes, not recomputed.
            let pylint_code = RuleCode::new(&pylint::META);
            let carried = std::cell::RefCell::new(
                diagnostics
                    .iter()
                    .filter(|d| d.rule == pylint_code)
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            let reanalyze = |text: &str, edits: &[TextEdit]| -> Vec<Diagnostic> {
                let text: Arc<str> = Arc::from(text);
                let mut out = match (file_of_target.get(&t), standalone_by_target.get(&t)) {
                    (Some(f), _) => reanalyze_resource(
                        rules,
                        &ws,
                        &index,
                        &target_of,
                        &targets,
                        &overrides,
                        *f,
                        Arc::clone(&text),
                    ),
                    (None, Some(s)) => {
                        reanalyze_standalone(rules, &ws, &index, target, s.file, Arc::clone(&text))
                    }
                    _ => Vec::new(),
                };
                let mut carried = carried.borrow_mut();
                if !carried.is_empty() {
                    let file = SourceFile::new(target.path.clone(), text);
                    carried.retain_mut(|d| {
                        let Some(range) = crate::fix::shift_range(d.range, edits) else {
                            return false;
                        };
                        d.range = range;
                        d.file = file.clone();
                        true
                    });
                    out.retain(|d| d.rule != pylint_code);
                    out.extend(carried.iter().cloned());
                }
                out
            };
            let (fix, diagnostics) = fix_file(
                settings,
                options,
                &target.path,
                &source.shared_text(),
                diagnostics,
                &reanalyze,
            )?;
            Ok(Outcome {
                checked: CheckedFile {
                    path: target.path.clone(),
                    kind: target.kind,
                    cached: cached[t],
                    read_time: Duration::ZERO,
                    analysis_time: time[t],
                },
                diagnostics,
                fix,
                cache_entry,
            })
        })
        .collect();
    Analysis {
        outcomes,
        errors,
        notices,
    }
}

/// One unit whose scripts pylint lints.
struct BatchUnit<'m, 'a> {
    set: Arc<RuleSet>,
    cache: &'m UnitCache,
    /// `(position in data.scripts(), target of the script's file, script)`
    /// for the scripts in linted files.
    scripts: Vec<(usize, usize, &'m EmbeddedScript<'a>)>,
}

/// Runs pylint once per distinct pylint configuration (normally once) over
/// the scripts of `units` and stores the messages in the units' caches.
/// Returns the targets whose pylint findings are missing (pylint failed)
/// and the run's notices.
fn pylint_batch(units: &[BatchUnit<'_, '_>], root: &Path) -> (FxHashSet<usize>, Vec<String>) {
    let mut failed = FxHashSet::default();
    let mut notices: Vec<String> = Vec::new();
    let notice = |n: String, notices: &mut Vec<String>| {
        if !notices.contains(&n) {
            notices.push(n);
        }
    };
    let mut groups: std::collections::BTreeMap<pylint::run::Config, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (i, unit) in units.iter().enumerate() {
        let Some(options) = pylint::options_in(&unit.set) else {
            continue;
        };
        let (config, warning) = pylint::run::Config::new(options, root);
        if let Some(w) = warning {
            notice(w, &mut notices);
        }
        groups.entry(config).or_default().push(i);
    }
    let syntax_rule = RuleCode::new(&crate::rules::script::syntax_error::META);
    for (config, members) in groups {
        let mut scripts = Vec::new();
        let mut owners = Vec::new();
        for &u in &members {
            for &(position, _, script) in &units[u].scripts {
                // Blank scripts are VIEW's `empty-script`; pylint would only
                // report a syntax error.
                if script.is_blank() {
                    continue;
                }
                scripts.push(pylint::run::Script {
                    source: &script.source,
                    embedded: matches!(script.body, iglint_project::script::ScriptBody::Json(_)),
                });
                owners.push((u, position));
            }
        }
        let outcome = pylint::run::run(&config, &scripts);
        for n in outcome.notices {
            notice(n, &mut notices);
        }
        let Some(messages) = outcome.messages else {
            for &u in &members {
                failed.extend(units[u].scripts.iter().map(|(_, t, _)| *t));
            }
            continue;
        };
        let mut per_unit: FxHashMap<usize, pylint::UnitMessages> = FxHashMap::default();
        for ((u, position), found) in owners.into_iter().zip(messages) {
            if !found.is_empty() {
                per_unit
                    .entry(u)
                    .or_default()
                    .by_script
                    .insert(position, found);
            }
        }
        for u in members {
            let mut result = per_unit.remove(&u).unwrap_or_default();
            result.syntax_rule_enabled = units[u].set.contains(syntax_rule);
            let _ = units[u].cache.pylint.set(result);
        }
    }
    (failed, notices)
}

/// Hash of how pylint runs for the targets that enable SCR900 (command,
/// jobs and configuration text), part of the cache context.
fn pylint_context(
    rules: &'static [&'static RuleImpl],
    sets: &RuleSetCache,
    targets: &[Target],
    settings: &ProjectSettings,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut configs = std::collections::BTreeSet::new();
    let mut seen: Vec<*const LinterSettings> = Vec::new();
    for t in targets {
        let ptr = Arc::as_ptr(&t.settings);
        if seen.contains(&ptr) {
            continue;
        }
        seen.push(ptr);
        let set = sets.get(rules, &t.settings);
        if let Some(options) = pylint::options_in(&set) {
            configs.insert(pylint::run::Config::new(options, &settings.root).0);
        }
    }
    if configs.is_empty() {
        return 0;
    }
    let mut hasher = FxHasher::default();
    for config in configs {
        config.hash(&mut hasher);
        config.rcfile_text().hash(&mut hasher);
    }
    hasher.finish()
}

/// Runs the hooks of one phase over a unit.
pub(crate) fn run_phase(
    unit: &Unit<'_>,
    index: Option<&ProjectIndex<'_>>,
    set: &RuleSet,
    hooks: &PhaseHooks,
    out: &mut Vec<Diagnostic>,
) {
    for (slot, rule) in &hooks.resource {
        let mut cx = Checker::new(unit, index, &set.active[*slot], out);
        rule.check_resource(&mut cx, unit.data);
    }
    let bindings = match unit.data {
        ModelData::View { view, .. } => {
            for (slot, rule) in &hooks.view {
                let mut cx = Checker::new(unit, index, &set.active[*slot], out);
                rule.check_view(&mut cx, view);
            }
            if !hooks.component.is_empty() {
                for component in &view.components {
                    for (slot, rule) in &hooks.component {
                        let mut cx = Checker::new(unit, index, &set.active[*slot], out);
                        rule.check_component(&mut cx, view, component);
                    }
                }
            }
            view.bindings.as_slice()
        }
        ModelData::SessionProps { props, .. } => props.bindings.as_slice(),
        _ => &[],
    };
    if !hooks.binding.is_empty() {
        for binding in bindings {
            for (slot, rule) in &hooks.binding {
                let mut cx = Checker::new(unit, index, &set.active[*slot], out);
                rule.check_binding(&mut cx, binding);
            }
        }
    }
    if !hooks.expression.is_empty() {
        for (_, expression) in unit.data.expressions() {
            for (slot, rule) in &hooks.expression {
                let mut cx = Checker::new(unit, index, &set.active[*slot], out);
                rule.check_expression(&mut cx, expression);
            }
        }
    }
    if !hooks.script.is_empty() {
        // Unused suppressions are reported after the last phase, when every
        // script rule has run.
        let unused_slot = std::ptr::eq(hooks, &set.project)
            .then(|| crate::rules::script::unused_suppression::slot(set))
            .flatten();
        let parsed = unit.cache.parsed_scripts(unit.data);
        for (position, ((file, script), parsed)) in
            unit.data.scripts().into_iter().zip(parsed).enumerate()
        {
            let start = out.len();
            let script_unit = ScriptUnit::new(file, script, parsed, position);
            for (slot, rule) in &hooks.script {
                let mut cx = Checker::new(unit, index, &set.active[*slot], out);
                rule.check_script(&mut cx, &script_unit);
            }
            if out.len() > start || unused_slot.is_some() {
                let mut cache = unit
                    .cache
                    .suppressions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let suppressions = cache
                    .entry(position)
                    .or_insert_with(|| Suppressions::of(script));
                if !suppressions.is_empty() {
                    let mut found = out.split_off(start);
                    suppressions.apply(&mut found);
                    out.extend(found);
                    if let Some(slot) = unused_slot {
                        let mut cx = Checker::new(unit, index, &set.active[slot], out);
                        crate::rules::script::unused_suppression::report_unused(
                            &mut cx,
                            &script_unit,
                            suppressions,
                            set,
                        );
                    }
                }
            }
        }
    }
}

/// Feeds the literal references of a resource's scripts into its summary
/// (see [`crate::script_facts`]).
fn add_script_references(
    data: &ModelData<'_>,
    cache: &UnitCache,
    ctx: &crate::script_facts::FactsContext<'_>,
    summary: &mut ResourceSummary,
) {
    let scripts = data.scripts();
    if scripts.is_empty() {
        return;
    }
    let facts = crate::script_facts::provider();
    let parsed = cache.parsed_scripts(data);
    for (index, ((file, script), parsed)) in scripts.into_iter().zip(parsed).enumerate() {
        let unit = ScriptUnit::new(file, script, parsed, index);
        facts.summarize(&unit, ctx, summary);
    }
}

/// The file most findings of a resource go to.
fn primary_file(model: &ResourceModel<'_>) -> FileId {
    let data_file = match &model.data {
        ModelData::View { file, .. }
        | ModelData::SessionProps { file, .. }
        | ModelData::Stylesheet { file, .. } => Some(*file),
        ModelData::StyleClass(s) => Some(s.file),
        ModelData::PageConfig(p) => Some(p.file),
        ModelData::Library(l) => Some(l.file),
        ModelData::NamedQuery(q) => q.sql_file,
        ModelData::EventScript(e) => e.script.as_ref().map(|s| s.file),
        ModelData::Gateway(g) => g.config_file,
        ModelData::Tags(t) => t.files.first().map(|f| f.0),
        _ => None,
    };
    data_file.unwrap_or(model.resource.manifest.file)
}

fn shared_text(f: &LoadedFile) -> Option<Arc<str>> {
    match &f.json {
        Some(doc) => Some(Arc::clone(doc.shared_text())),
        None => f.text.clone(),
    }
}

fn read_text(path: &Path) -> Result<Arc<str>, String> {
    match std::fs::read(path) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(Arc::from(text)),
            Err(_) => Err("file is not valid UTF-8".to_owned()),
        },
        Err(error) => Err(error.to_string()),
    }
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// Loads a resource, replacing files whose text is given in `overrides`.
fn load(ws: &Workspace, id: ResourceId, overrides: &FxHashMap<FileId, Arc<str>>) -> LoadedResource {
    let mut loaded = ws.load_resource(id);
    if !overrides.is_empty() {
        for f in &mut loaded.files {
            if let Some(text) = overrides.get(&f.file) {
                replace_text(f, Arc::clone(text));
            }
        }
    }
    loaded
}

fn replace_text(f: &mut LoadedFile, text: Arc<str>) {
    f.error = None;
    if f.kind.is_json() {
        f.json = Some(JsonDocument::parse(text));
        f.text = None;
    } else {
        f.json = None;
        f.text = Some(text);
    }
}

/// The model of a file outside any resource: views are modelled as views,
/// Python files as modules, other JSON files only by their parse result.
fn standalone_model<'a>(
    path: &Path,
    file: FileId,
    doc: Option<&'a JsonDocument>,
    text: &'a str,
) -> (ModelData<'a>, Vec<ModelFileError>) {
    let mut errors = Vec::new();
    if let Some(e) = doc.and_then(JsonDocument::error) {
        errors.push(ModelFileError {
            file,
            range: e.range,
            message: e.to_string(),
        });
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let data = match doc.and_then(JsonDocument::root) {
        Some(root) if name == "view.json" => ModelData::View {
            file,
            view: build_view(root),
        },
        Some(_) | None if doc.is_some() => ModelData::None,
        _ if path.extension().is_some_and(|e| e == "py") => {
            let module_path = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            ModelData::Library(LibraryModule {
                script: library_script(text, module_path.clone()),
                module_path,
                file,
                hint_scope: None,
            })
        }
        _ => ModelData::None,
    };
    (data, errors)
}

/// Re-analyses the resource that owns `file` with new text for `file`,
/// against the existing index. Returns the findings in `file`.
#[allow(clippy::too_many_arguments)]
fn reanalyze_resource(
    rules: &'static [&'static RuleImpl],
    ws: &Workspace,
    index: &ProjectIndex<'_>,
    target_of: &[Option<usize>],
    targets: &[Target],
    overrides: &FxHashMap<FileId, Arc<str>>,
    file: FileId,
    text: Arc<str>,
) -> Vec<Diagnostic> {
    let entry = ws.file(file);
    let Some(t) = target_of[file.index()] else {
        return Vec::new();
    };
    let target = &targets[t];
    let mut table = FileTable::with_len(ws.files.len());
    table.insert(
        file,
        SourceFile::new(target.path.clone(), Arc::clone(&text)),
    );
    let set = RuleSet::build(rules, &target.settings);
    let mut out = Vec::new();
    let Some(rid) = entry.resource else {
        return out;
    };
    let resource = ws.resource(rid);
    if resource.manifest.file == file {
        // A manifest: re-parse it standalone (its resource is unchanged).
        return out;
    }
    let mut loaded = load(ws, rid, overrides);
    for f in &mut loaded.files {
        if f.file == file {
            replace_text(f, Arc::clone(&text));
        }
    }
    let model = build_model(ws, &loaded);
    let cache = UnitCache::default();
    let unit = Unit {
        ws,
        resource: Some(resource),
        project: match resource.container {
            Container::Project(p) => Some(p),
            Container::Collection(_) => None,
        },
        data: &model.data,
        errors: &model.errors,
        primary: primary_file(&model),
        files: &table,
        strict: target.settings.strict,
        cache: &cache,
    };
    run_phase(&unit, None, &set, &set.local, &mut out);
    run_phase(&unit, Some(index), &set, &set.project, &mut out);
    out.retain(|d| d.file.path() == target.path && target.settings.is_enabled(d.rule));
    out
}

fn reanalyze_standalone(
    rules: &'static [&'static RuleImpl],
    ws: &Workspace,
    index: &ProjectIndex<'_>,
    target: &Target,
    file: FileId,
    text: Arc<str>,
) -> Vec<Diagnostic> {
    let mut table = FileTable::with_len(file.index() + 1);
    table.insert(
        file,
        SourceFile::new(target.path.clone(), Arc::clone(&text)),
    );
    let doc = is_json(&target.path).then(|| JsonDocument::parse(Arc::clone(&text)));
    let (data, errors) = standalone_model(&target.path, file, doc.as_ref(), &text);
    let set = RuleSet::build(rules, &target.settings);
    let cache = UnitCache::default();
    let unit = Unit {
        ws,
        resource: None,
        project: None,
        data: &data,
        errors: &errors,
        primary: file,
        files: &table,
        strict: target.settings.strict,
        cache: &cache,
    };
    let mut out = Vec::new();
    run_phase(&unit, None, &set, &set.local, &mut out);
    run_phase(&unit, Some(index), &set, &set.project, &mut out);
    out
}

/// Hash of every file the run may read: cached results are only valid when
/// nothing in the workspace changed.
fn context_hash(table: &FileTable) -> u64 {
    use std::hash::Hasher;
    let mut hasher = FxHasher::default();
    for i in 0..table.len() {
        if let Some(f) = table.get(FileId(i as u32)) {
            hasher.write(f.path().as_os_str().as_encoded_bytes());
            hasher.write_usize(f.text().len());
            hasher.write(f.text().as_bytes());
        }
    }
    hasher.finish()
}

/// The range of a JSON value's key when it is an object member, for
/// reporting on a property name. Falls back to the value.
pub fn member_key_range(object: &Value<'_>, key: &str) -> Option<TextRange> {
    let member = object.as_object()?.member(key)?;
    Some(member.key.range())
}
