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
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use iglint_json::Value;
use iglint_project::EmbeddedScript;
use iglint_project::library::LibraryModule;
use iglint_project::model::{FileError as ModelFileError, LoadedFile, LoadedResource, ModelData};
use iglint_project::script::library_script;
use iglint_project::{
    Container, FileId, JsonDocument, ProjectId, ProjectIndex, ResourceId, ResourceModel,
    ResourceSummary, Workspace, build_model, build_view, summarize,
};
use iglint_text::TextEdit;
use iglint_text::TextRange;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};

use crate::cache::Cache;
use crate::checker::{Checker, FileTable, Unit, UnitCache};
use crate::diagnostic::{Diagnostic, SourceFile};
use crate::discovery::{FileKind, absolute};
use crate::engine::{
    CheckOptions, CheckedFile, FileError, Outcome, Reanalysis, fix_file, health_settings,
};
use crate::registry::RuleCode;
use crate::rule::{PhaseHooks, RuleImpl, RuleSet, RuleSetCache, ScriptUnit};
use crate::safety::ScriptText;

/// Suppressions of a unit's scripts by position, with what they matched.
type SuppressionState = FxHashMap<usize, Suppressions>;
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
    /// Targets that were found but not linted, with the reason.
    pub skipped: Vec<(PathBuf, crate::discovery::SkipReason)>,
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
    let linted_paths: Vec<PathBuf> =
        if roots.is_empty() || targets.iter().any(|t| t.stdin.is_some()) {
            targets.iter().map(|t| t.path.clone()).collect()
        } else {
            roots.to_vec()
        };
    // The workspace is everything below the roots of the linted paths, not
    // only the linted paths: the closed world and the uses of library
    // names are the same whichever files are linted.
    let ws_roots = crate::discovery::workspace_roots(settings, &linted_paths);
    let ws = Workspace::discover(&ws_roots.roots);
    let closed_world_unknown = !ws_roots.unknown.is_empty();

    // Which workspace file is which target. Files that are not linted as
    // part of their resource (see [`treatment`]) are linted standalone,
    // like files outside any project, or skipped with a reason.
    let mut not_modelled: FxHashMap<usize, &'static str> = FxHashMap::default();
    // Standalone targets inside a project lint with that project's index.
    let mut project_of_standalone: FxHashMap<usize, ProjectId> = FxHashMap::default();
    let target_of: Vec<Option<usize>> = ws
        .files
        .iter()
        .map(|f| {
            let t = by_abs.get(&absolute(&f.path)).copied()?;
            match treatment(&ws, f) {
                Treatment::Resource => Some(t),
                Treatment::Standalone => {
                    if let Some(Container::Project(p)) = f.container {
                        project_of_standalone.insert(t, p);
                    }
                    None
                }
                Treatment::Skip(reason) => {
                    not_modelled.insert(t, reason);
                    None
                }
            }
        })
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
    let standalone: Vec<(usize, Result<Decoded, String>)> = (0..targets.len())
        .filter(|i| !in_workspace[*i] && !not_modelled.contains_key(i))
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|i| {
            let t = &targets[i];
            let text = match &t.stdin {
                Some(text) => Ok((Arc::clone(text), Vec::new())),
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
                table.insert(
                    f.file,
                    SourceFile::decoded(display(f.file), text, f.invalid_utf8.clone()),
                );
            }
        }
    }
    for r in ws.resources.iter().filter(|r| !r.manifest.missing) {
        let doc = &r.manifest.doc;
        table.insert(
            r.manifest.file,
            SourceFile::decoded(
                display(r.manifest.file),
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
                display(p.manifest.file),
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
                display(c.manifest.file),
                Arc::clone(doc.shared_text()),
                c.manifest.invalid_utf8.clone(),
            ),
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
    let unread_texts: Vec<(FileId, usize, Result<Decoded, String>)> = unread
        .into_par_iter()
        .map(|(f, t)| {
            let text = match &targets[t].stdin {
                Some(text) => Ok((Arc::clone(text), Vec::new())),
                None => read_text(&targets[t].path),
            };
            (f, t, text)
        })
        .collect();
    for (f, t, text) in unread_texts {
        match text {
            Ok((text, invalid)) => table.insert(
                f,
                SourceFile::decoded(targets[t].path.clone(), text, invalid),
            ),
            Err(message) => {
                errors_by_target.insert(t, message);
            }
        }
    }
    let mut standalone_files: Vec<(usize, FileId, Arc<str>)> = Vec::new();
    for (k, (t, text)) in standalone.into_iter().enumerate() {
        match text {
            Ok((text, invalid)) => {
                let id = FileId((ws.files.len() + k) as u32);
                table.insert(
                    id,
                    SourceFile::decoded(targets[t].path.clone(), Arc::clone(&text), invalid),
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

    // Cache: a unit's results depend on its own files, on the workspace
    // layout and manifests (phase A), on the project index (phase B) and on
    // how pylint runs. The index is built from the resource summaries, so
    // the cache context is a hash of the layout, the manifests and every
    // summary: an edit that leaves all summaries as they were (most edits
    // inside one view) keeps every other unit's cached results valid.
    let context_base = cache.map(|_| {
        layout_hash(&ws, &table)
            ^ pylint_context(rules, &sets, &targets, settings)
            ^ u64::from(closed_world_unknown).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ {
                // The vetted names decide what counts as unknown code.
                use std::hash::{Hash, Hasher};
                let mut hasher = FxHasher::default();
                settings.vetted.hash(&mut hasher);
                hasher.finish()
            }
    });

    // Phase A, part 2: model, summarise and run local rules.
    let models: Vec<ResourceModel<'_>> = loaded.par_iter().map(|l| build_model(&ws, l)).collect();
    // With a cache, every summary is needed before any unit can be looked
    // up. Their parse trees are dropped right away; units that are not
    // cached parse again in phase A.
    let presummaries: Vec<Option<ResourceSummary>> = match context_base {
        Some(_) => models
            .par_iter()
            .zip(loaded.par_iter())
            .map(|(m, l)| Some(summarize_with_scripts(&ws, m, l, &UnitCache::default())))
            .collect(),
        None => (0..models.len()).map(|_| None).collect(),
    };
    let context = context_base.map(|base| base ^ summaries_hash(&presummaries));
    // Per-unit caches (parsed scripts above all) live only for one phase:
    // keeping every parse tree of the workspace from phase A to phase B
    // doubled the peak memory. What phase B needs from phase A is carried
    // over: suppression bookkeeping and pylint's messages.
    let pylint_results: Vec<OnceLock<pylint::UnitMessages>> =
        (0..models.len()).map(|_| OnceLock::new()).collect();
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
        /// Suppression state of the unit's scripts after phase A.
        suppressions: SuppressionState,
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

    // Cached results first: they decide which units pylint has to lint.
    let precached: Vec<Option<Vec<Diagnostic>>> = if context.is_some() {
        models
            .par_iter()
            .map(|model| {
                let rid = model.resource.id;
                if linted[rid.index()] {
                    cached_unit(rid)
                } else {
                    None
                }
            })
            .collect()
    } else {
        (0..models.len()).map(|_| None).collect()
    };

    // pylint (SCR900) runs in its own processes while the local rules run:
    // it is by far the slowest part when enabled.
    let mut batch: Vec<BatchUnit<'_, '_>> = Vec::new();
    for (i, model) in models.iter().enumerate() {
        if !linted[i] || precached[i].is_some() {
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
            pylint: &pylint_results[i],
            scripts,
        });
    }

    let run_phase_a = || -> Vec<(ResourceSummary, Option<UnitResult>)> {
        models
            .par_iter()
            .zip(precached.into_par_iter())
            .zip(presummaries.into_par_iter())
            .zip(loaded.par_iter())
            .map(|(((model, precached), presummary), l)| {
                let rid = model.resource.id;
                let cache = UnitCache::default();
                let summary =
                    presummary.unwrap_or_else(|| summarize_with_scripts(&ws, model, l, &cache));
                if !linted[rid.index()] {
                    return (summary, None);
                }
                if let Some(diagnostics) = precached {
                    return (
                        summary,
                        Some(UnitResult {
                            diagnostics,
                            time: Duration::ZERO,
                            cached: true,
                            suppressions: SuppressionState::default(),
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
                            cache: &cache,
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
                        suppressions: cache
                            .suppressions
                            .into_inner()
                            .unwrap_or_else(|e| e.into_inner()),
                    }),
                )
            })
            .collect()
    };
    let ((pylint_failed, notices), phase_a) = if batch.is_empty() {
        ((FxHashSet::default(), Vec::new()), run_phase_a())
    } else {
        rayon::join(|| pylint_batch(&batch, &settings.root), run_phase_a)
    };
    drop(batch);
    let (summaries, mut unit_results): (Vec<ResourceSummary>, Vec<Option<UnitResult>>) =
        phase_a.into_iter().unzip();

    // Index.
    let index = build_index(&ws, summaries, closed_world_unknown, &settings.vetted);

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

    // pylint for standalone files (outside any resource).
    let mut batch: Vec<BatchUnit<'_, '_>> = Vec::new();
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
            pylint: &cache.pylint,
            scripts,
        });
    }
    let (mut pylint_failed, mut notices) = (pylint_failed, notices);
    if !batch.is_empty() {
        let (failed, more) = pylint_batch(&batch, &settings.root);
        pylint_failed.extend(failed);
        for n in more {
            if !notices.contains(&n) {
                notices.push(n);
            }
        }
    }
    drop(batch);

    // Phase B.
    let carried: Vec<(usize, SuppressionState, Option<pylint::UnitMessages>)> = unit_results
        .iter_mut()
        .zip(pylint_results)
        .enumerate()
        .filter_map(|(i, (u, pylint))| {
            let u = u.as_mut().filter(|u| !u.cached)?;
            Some((i, std::mem::take(&mut u.suppressions), pylint.into_inner()))
        })
        .collect();
    let phase_b: Vec<(usize, Vec<Diagnostic>, Duration)> = carried
        .into_par_iter()
        .map(|(i, suppressions, pylint)| {
            let model = &models[i];
            let cache = UnitCache::default();
            *cache.suppressions.lock().unwrap_or_else(|e| e.into_inner()) = suppressions;
            if let Some(messages) = pylint {
                let _ = cache.pylint.set(messages);
            }
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
                        cache: &cache,
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
                    project: project_of_standalone.get(&s.target).copied(),
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
    // Cached findings name the workspace's first unknown code as it is now.
    let first_unknown = crate::rules::vetting::first_unknown_site(&index, &table);
    let place = |d: Diagnostic, origin: Option<usize>, per_target: &mut Vec<Vec<Diagnostic>>| {
        let Some(&t) = by_display.get(d.file.path()) else {
            return;
        };
        let Some(rule_settings) = targets[t].settings.rule(d.rule) else {
            return;
        };
        let mut d = d;
        crate::rules::vetting::complete_reasons(
            std::slice::from_mut(&mut d),
            first_unknown.as_ref(),
        );
        crate::rules::vetting::complete_store_reasons(std::slice::from_mut(&mut d), &index, &table);
        let same_settings = || {
            d.severity_from_options
                && origin.and_then(|o| targets[o].settings.rule(d.rule)) == Some(rule_settings)
        };
        if origin != Some(t) && !same_settings() {
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
    let mut skipped: Vec<(PathBuf, crate::discovery::SkipReason)> = not_modelled
        .iter()
        .map(|(t, reason)| {
            (
                targets[*t].path.clone(),
                crate::discovery::SkipReason::NotModelled(reason),
            )
        })
        .collect();
    skipped.sort_by(|a, b| a.0.cmp(&b.0));
    let outcomes: Vec<Result<Outcome, FileError>> = per_target
        .into_par_iter()
        .enumerate()
        .filter(|(t, _)| !not_modelled.contains_key(t))
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
            // Code that may read workspace files observes every fix: after
            // the cache, which keeps the findings of the file as they are.
            let mut diagnostics = diagnostics;
            crate::rules::vetting::gate_file_reads(
                &mut diagnostics,
                &index,
                &table,
                settings.assume_no_workspace_file_reads,
            );
            let scripts = |text: &str| -> Vec<ScriptText> {
                let text: Arc<str> = Arc::from(text);
                match (file_of_target.get(&t), standalone_by_target.get(&t)) {
                    (Some(f), _) => resource_scripts(&ws, &overrides, *f, text),
                    (None, Some(_)) => standalone_scripts(&target.path, text),
                    _ => Vec::new(),
                }
            };
            // A safe fix the post-fix net would drop is reported as unsafe.
            crate::safety::mark_rejected_safe_fixes(
                &target.path,
                source.text(),
                &mut diagnostics,
                &scripts,
            );
            // pylint findings are carried through the fixes, not recomputed.
            let pylint_code = RuleCode::new(&pylint::META);
            let carried = std::cell::RefCell::new(
                diagnostics
                    .iter()
                    .filter(|d| d.rule == pylint_code)
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            let assume_no_reads = settings.assume_no_workspace_file_reads;
            let reanalyze = |text: &str, edits: &[TextEdit], what: Reanalysis| -> Vec<Diagnostic> {
                let text: Arc<str> = Arc::from(text);
                let settings = match what {
                    Reanalysis::Lint => &target.settings,
                    Reanalysis::Health => health_settings(),
                };
                let mut out = match (file_of_target.get(&t), standalone_by_target.get(&t)) {
                    (Some(f), _) => reanalyze_resource(
                        rules,
                        &ws,
                        &index,
                        &target_of,
                        &targets,
                        &overrides,
                        settings,
                        *f,
                        Arc::clone(&text),
                    ),
                    (None, Some(s)) => reanalyze_standalone(
                        rules,
                        &ws,
                        &index,
                        target,
                        settings,
                        (s.file, project_of_standalone.get(&t).copied()),
                        Arc::clone(&text),
                    ),
                    _ => Vec::new(),
                };
                if what == Reanalysis::Health {
                    return out;
                }
                crate::rules::vetting::gate_file_reads(&mut out, &index, &table, assume_no_reads);
                crate::safety::mark_rejected_safe_fixes(&target.path, &text, &mut out, &scripts);
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
                &scripts,
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
    if !options.release_memory {
        // Freeing every parse tree and model node takes a noticeable share
        // of a short run; a process that exits next does not need it.
        std::mem::forget(standalone_models);
        std::mem::forget(standalone_docs);
        std::mem::forget(index);
        std::mem::forget(models);
        std::mem::forget(loaded);
        std::mem::forget(table);
        std::mem::forget(ws);
    }
    Analysis {
        outcomes,
        errors,
        notices,
        skipped,
    }
}

/// How a discovered file of the workspace is linted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Treatment {
    /// With its resource (the usual case).
    Resource,
    /// On its own, like a file outside any project: files outside every
    /// resource directory, JSON files the resource model does not read,
    /// and Python files of a modelled resource type that are not its
    /// script (the gateway does not load them; REF009 says so).
    Standalone,
    /// Not linted, for the reason given (reported in the run summary).
    Skip(&'static str),
}

pub(crate) fn treatment(ws: &Workspace, f: &iglint_project::workspace::FileEntry) -> Treatment {
    use iglint_project::kinds::FileKind as K;
    let extension = Path::new(f.name())
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match (f.kind, extension.as_deref()) {
        (K::Stray, _) => Treatment::Standalone,
        (K::WebDevContent, Some("json" | "py")) => {
            Treatment::Skip("WebDev file resource content, served as is")
        }
        (K::OtherResourceData, Some("json")) => Treatment::Standalone,
        (K::OtherResourceData, Some("py")) => {
            let modelled = f
                .resource
                .is_some_and(|r| ws.resource(r).kind.is_modelled());
            if modelled {
                Treatment::Standalone
            } else {
                Treatment::Skip("Python file of a resource type iglint does not model")
            }
        }
        _ => Treatment::Resource,
    }
}

/// One unit whose scripts pylint lints.
struct BatchUnit<'m, 'a> {
    set: Arc<RuleSet>,
    /// Where the unit's pylint messages go.
    pylint: &'m OnceLock<pylint::UnitMessages>,
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
            let _ = units[u].pylint.set(result);
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
    let start = out.len();
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
        // Scripts are independent: check them in parallel (a large view has
        // hundreds) and keep their findings in script order.
        let found: Vec<Vec<Diagnostic>> = unit
            .data
            .scripts()
            .into_par_iter()
            .zip(parsed.par_iter())
            .enumerate()
            .map(|(position, ((file, script), parsed))| {
                let mut out = Vec::new();
                let script_unit = ScriptUnit::new(file, script, parsed, position);
                for (slot, rule) in &hooks.script {
                    let mut cx = Checker::new(unit, index, &set.active[*slot], &mut out);
                    rule.check_script(&mut cx, &script_unit);
                }
                if !out.is_empty() || unused_slot.is_some() {
                    let mut cache = unit
                        .cache
                        .suppressions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    let suppressions = cache
                        .entry(position)
                        .or_insert_with(|| Suppressions::of(script));
                    if !suppressions.is_empty() {
                        suppressions.apply(&mut out);
                        if let Some(slot) = unused_slot {
                            let mut cx = Checker::new(unit, index, &set.active[slot], &mut out);
                            crate::rules::script::unused_suppression::report_unused(
                                &mut cx,
                                &script_unit,
                                suppressions,
                                set,
                            );
                        }
                    }
                }
                out
            })
            .collect();
        out.extend(found.into_iter().flatten());
    }
    if let Some(index) = index {
        let site = crate::rules::vetting::first_unknown_site(index, unit.files);
        crate::rules::vetting::complete_reasons(&mut out[start..], site.as_ref());
        crate::rules::vetting::complete_store_reasons(&mut out[start..], index, unit.files);
    }
}

/// The project index of `summaries`, with the facts only the script
/// layer can derive from the whole index (Perspective objects handed to
/// library functions that reach other views). Every index the linter uses
/// is built here.
pub(crate) fn build_index<'w>(
    ws: &'w Workspace,
    summaries: Vec<ResourceSummary>,
    closed_world_unknown: bool,
    vetted: &iglint_project::Vetted,
) -> ProjectIndex<'w> {
    let mut index = ProjectIndex::build_with(ws, summaries, vetted.clone());
    if closed_world_unknown {
        index.mark_closed_world_unknown();
    }
    crate::script_facts::access::resolve_perspective_calls(&mut index);
    index
}

/// The summary of a resource model with what its scripts contribute
/// (references and library symbols), as phase A computes it. The live
/// workspace uses the same function so its index equals `check`'s.
pub(crate) fn summarize_with_scripts(
    ws: &Workspace,
    model: &ResourceModel<'_>,
    loaded: &LoadedResource,
    cache: &UnitCache,
) -> ResourceSummary {
    let mut summary = summarize(model);
    if crate::script_facts::provider().provides_references() {
        let project_names: Vec<&str> = ws.projects.iter().map(|p| p.name.as_str()).collect();
        let project = match model.resource.container {
            Container::Project(p) => Some(ws.project(p).name.as_str()),
            Container::Collection(_) => None,
        };
        let module = match &model.data {
            ModelData::Library(l) => Some(l.module_path.as_str()),
            _ => None,
        };
        let ctx = crate::script_facts::FactsContext {
            project,
            projects: &project_names,
            module,
        };
        add_script_references(&model.data, cache, &ctx, &mut summary);
        crate::script_facts::code_sources::run_script_code(model, &ctx, &mut summary);
        crate::script_facts::code_sources::unread_code(model, loaded, &mut summary);
    }
    summary
}

/// Feeds the literal references of a resource's scripts into its summary
/// (see [`crate::script_facts`]).
pub(crate) fn add_script_references(
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
    let view = match data {
        ModelData::View { view, .. } => Some(view),
        _ => None,
    };
    // The scripts of a view in parallel, each into a summary of its own,
    // added in script order (review round 30: a view with two thousand
    // scripts took 33 ms one after the other, in the language server's
    // time to publish an edit). A script only adds references, so the
    // result is the same ([`add_script_references_in_order`]); a library
    // module (whose script sets the module's symbols) runs as before.
    let library = scripts
        .iter()
        .any(|(_, s)| s.kind == iglint_project::ScriptKind::LibraryModule);
    if library || scripts.len() < 2 {
        for (index, ((file, script), parsed)) in scripts.into_iter().zip(parsed).enumerate() {
            let unit = ScriptUnit::new(file, script, parsed, index);
            facts.summarize(&unit, view, ctx, summary);
        }
        return;
    }
    use rayon::prelude::*;
    let resource = summary.resource;
    let parts: Vec<iglint_project::summary::References> = scripts
        .par_iter()
        .zip(parsed.par_iter())
        .enumerate()
        .map(|(index, ((file, script), parsed))| {
            let unit = ScriptUnit::new(*file, script, parsed, index);
            let mut part = ResourceSummary {
                resource,
                valid: true,
                facts: iglint_project::summary::Facts::None,
                references: Default::default(),
            };
            facts.summarize(&unit, view, ctx, &mut part);
            part.references
        })
        .collect();
    for references in parts {
        summary.references.extend(references);
    }
}

/// [`add_script_references`] one script after the other, as before review
/// round 30 (the reference of its differential test).
#[cfg(test)]
pub(crate) fn add_script_references_in_order(
    data: &ModelData<'_>,
    cache: &UnitCache,
    ctx: &crate::script_facts::FactsContext<'_>,
    summary: &mut ResourceSummary,
) {
    let facts = crate::script_facts::provider();
    let parsed = cache.parsed_scripts(data);
    let view = match data {
        ModelData::View { view, .. } => Some(view),
        _ => None,
    };
    for (index, ((file, script), parsed)) in data.scripts().into_iter().zip(parsed).enumerate() {
        let unit = ScriptUnit::new(file, script, parsed, index);
        facts.summarize(&unit, view, ctx, summary);
    }
}

/// The file most findings of a resource go to.
pub(crate) fn primary_file(model: &ResourceModel<'_>) -> FileId {
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

pub(crate) fn shared_text(f: &LoadedFile) -> Option<Arc<str>> {
    match &f.json {
        Some(doc) => Some(Arc::clone(doc.shared_text())),
        None => f.text.clone(),
    }
}

/// A file's text and its invalid UTF-8 sequences (see
/// [`iglint_text::decode_utf8`]).
pub(crate) type Decoded = (Arc<str>, Vec<TextRange>);

/// Reads a file. Invalid UTF-8 is not an error: Ignition reads the file
/// too, with U+FFFD in place of the invalid sequences, and `VIEW007`
/// reports them.
pub(crate) fn read_text(path: &Path) -> Result<Decoded, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let (text, invalid) = iglint_text::decode_utf8(bytes);
            Ok((Arc::from(text), invalid))
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// Loads a resource, replacing files whose text is given in `overrides`.
pub(crate) fn load(
    ws: &Workspace,
    id: ResourceId,
    overrides: &FxHashMap<FileId, Arc<str>>,
) -> LoadedResource {
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

pub(crate) fn replace_text(f: &mut LoadedFile, text: Arc<str>) {
    f.error = None;
    f.invalid_utf8.clear();
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
pub(crate) fn standalone_model<'a>(
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
    settings: &LinterSettings,
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
    let set = RuleSet::build(rules, settings);
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
    out.retain(|d| d.file.path() == target.path && settings.is_enabled(d.rule));
    out
}

fn reanalyze_standalone(
    rules: &'static [&'static RuleImpl],
    ws: &Workspace,
    index: &ProjectIndex<'_>,
    target: &Target,
    settings: &LinterSettings,
    (file, project): (FileId, Option<ProjectId>),
    text: Arc<str>,
) -> Vec<Diagnostic> {
    let mut table = FileTable::with_len(file.index() + 1);
    table.insert(
        file,
        SourceFile::new(target.path.clone(), Arc::clone(&text)),
    );
    let doc = is_json(&target.path).then(|| JsonDocument::parse(Arc::clone(&text)));
    let (data, errors) = standalone_model(&target.path, file, doc.as_ref(), &text);
    let set = RuleSet::build(rules, settings);
    let cache = UnitCache::default();
    let unit = Unit {
        ws,
        resource: None,
        project,
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

/// The scripts of `text` as the content of `file`, a data file of a
/// resource, modelled with the rest of the resource as it is (for the
/// safe-fix net, [`crate::safety`]).
pub(crate) fn resource_scripts(
    ws: &Workspace,
    overrides: &FxHashMap<FileId, Arc<str>>,
    file: FileId,
    text: Arc<str>,
) -> Vec<ScriptText> {
    let Some(rid) = ws.file(file).resource else {
        return Vec::new();
    };
    let mut loaded = load(ws, rid, overrides);
    for f in &mut loaded.files {
        if f.file == file {
            replace_text(f, Arc::clone(&text));
        }
    }
    let model = build_model(ws, &loaded);
    model
        .scripts()
        .into_iter()
        .filter(|(f, _)| *f == file)
        .map(|(_, s)| ScriptText::of(s))
        .collect()
}

/// The scripts of `text` as a file outside any resource (see
/// [`standalone_model`]).
pub(crate) fn standalone_scripts(path: &Path, text: Arc<str>) -> Vec<ScriptText> {
    let file = FileId(0);
    let doc = is_json(path).then(|| JsonDocument::parse(Arc::clone(&text)));
    let (data, _) = standalone_model(path, file, doc.as_ref(), &text);
    data.scripts()
        .into_iter()
        .map(|(_, s)| ScriptText::of(s))
        .collect()
}

/// Hash of the workspace layout: every file's path, and the text of every
/// file that is not a data file of a resource (manifests, standalone
/// files). Data files enter the cache context through their summaries.
fn layout_hash(ws: &Workspace, table: &FileTable) -> u64 {
    use std::hash::Hasher;
    let mut data = vec![false; table.len()];
    for r in &ws.resources {
        for f in &r.files {
            if *f != r.manifest.file
                && let Some(d) = data.get_mut(f.index())
            {
                *d = true;
            }
        }
    }
    let mut hasher = FxHasher::default();
    for (i, is_data) in data.iter().enumerate() {
        if let Some(f) = table.get(FileId(i as u32)) {
            hasher.write(f.path().as_os_str().as_encoded_bytes());
            if !is_data {
                hasher.write_usize(f.text().len());
                hasher.write(f.text().as_bytes());
            }
            // Invalid UTF-8 is replaced in the text; its positions are not
            // in any summary.
            for bad in f.invalid_utf8() {
                hasher.write_u32(bad.start().to_u32());
                hasher.write_u32(bad.end().to_u32());
            }
        }
    }
    // Directories that were not read decide the closed world (their
    // reasons name them).
    for unread in &ws.unread {
        hasher.write(unread.path.as_os_str().as_encoded_bytes());
        hasher.write(unread.message.as_bytes());
    }
    hasher.finish()
}

/// Hash of the resource summaries the project index is built from.
fn summaries_hash(summaries: &[Option<ResourceSummary>]) -> u64 {
    use std::fmt::Write as _;
    use std::hash::Hasher;
    let mut hasher = FxHasher::default();
    let mut text = String::new();
    for s in summaries.iter().flatten() {
        text.clear();
        // Summaries are plain owned data with a deterministic `Debug`.
        let _ = write!(text, "{s:?}");
        hasher.write(text.as_bytes());
    }
    hasher.finish()
}

/// The range of a JSON value's key when it is an object member, for
/// reporting on a property name. Falls back to the value.
pub fn member_key_range(object: &Value<'_>, key: &str) -> Option<TextRange> {
    let member = object.as_object()?.member(key)?;
    Some(member.key.range())
}

/// Review round 30 (Opus minor 1): the scripts of a view are summarised,
/// their property accesses found and their vetting judged in parallel. On
/// generated views full of what each step records (library references,
/// stores and values, code that is not vetted, file reads, frame reads,
/// messages, named queries, Perspective calls, scripts that do not parse,
/// dynamic keys), the parallel results are those of one script after the
/// other, as before.
#[cfg(test)]
mod parallel_tests {
    use super::*;
    use crate::rules::test_support::Fixture;

    const SNIPPETS: &[&str] = &[
        r"\tlib.util.f(self)",
        r"\texec 'x = 1'",
        r"\topen('/tmp/x').read()",
        r"\tsystem.perspective.print(self.view.custom.foo)",
        r"\tx = self.getSibling('B1').custom.bar",
        r"\tsystem.db.runNamedQuery('q', {})",
        r"\tdef f(:",
        r"\tsystem.util.sendMessage('p', 'm', {'a': 1})",
        r"\tgetattr(self.custom, 'x' + str(1))",
        r"\timport json\n\tlib.m.g()",
        r"\tself.view.custom.foo = 1",
        r"\tv = lib.a.b\n\tv.x = 1",
        r"\tlib.util.h = 5\n\tdel lib.util.k",
        r"\tself.view.custom[self.custom.k] = 2",
        r"\tlocals()",
        r"\tsystem.nothere.call()",
        r"\tfrom lib.util import *",
        r"\tlib.two.k(self.parent)",
    ];

    /// SplitMix64.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            usize::try_from((z ^ (z >> 31)) % n as u64).unwrap_or(0)
        }
    }

    /// A view whose button `B<i>` runs `scripts[i]`, one component a line,
    /// with an unread custom property `unused` on the view.
    fn view(scripts: &[&str]) -> String {
        let children: Vec<String> = scripts
            .iter()
            .enumerate()
            .map(|(i, s)| {
                format!(
                    r#"{{"meta": {{"name": "B{i}"}}, "type": "ia.input.button", "custom": {{"bar": {i}, "k": "foo"}}, "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{s}"}}}}}}}}}}"#
                )
            })
            .collect();
        format!(
            "{{\"custom\": {{\"foo\": 1, \"unused\": 2}}, \"params\": {{}},\n\"root\": {{\"meta\": {{\"name\": \"root\"}}, \"type\": \"ia.container.flex\", \"children\": [\n{}\n]}}}}\n",
            children.join(",\n")
        )
    }

    #[test]
    fn parallel_script_steps_match_one_after_the_other() {
        let mut rng = Rng(30);
        let mut vetted = 0;
        let mut named = 0;
        for round in 0..60 {
            let n = 2 + rng.below(40);
            // Every other round, only what keeps `unused` unread, with code
            // that is not vetted in some scripts.
            let plain = [0, 3, 4, 5, 7, 10, 15, 15];
            let scripts: Vec<&str> = (0..n)
                .map(|_| match round % 2 {
                    0 => SNIPPETS[rng.below(SNIPPETS.len())],
                    _ => SNIPPETS[plain[rng.below(plain.len())]],
                })
                .collect();
            let f = Fixture::new().with_view("V", &view(&scripts)).resource(
                "projects/p/ignition/script-python/lib/util",
                &[("code.py", "def f(c):\n    return c.custom.bar\n")],
            );
            let ws = Workspace::discover(&[f.root().join("projects")]);
            let rid = ws
                .resources
                .iter()
                .find(|r| r.kind == iglint_project::ResourceKind::View)
                .expect("view")
                .id;
            let loaded = ws.load_resource(rid);
            let model = build_model(&ws, &loaded);
            let ctx = crate::script_facts::FactsContext {
                project: Some("p"),
                projects: &["p"],
                module: None,
            };
            // Summaries.
            let cache = UnitCache::default();
            let mut parallel = iglint_project::summarize(&model);
            let mut in_order = parallel.clone();
            add_script_references(&model.data, &cache, &ctx, &mut parallel);
            add_script_references_in_order(&model.data, &cache, &ctx, &mut in_order);
            assert_eq!(parallel, in_order, "round {round}: {scripts:?}");
            // Property accesses.
            let accesses = cache.property_accesses(&model.data).to_vec();
            let view = model.data.view().expect("view");
            let mut expected = Vec::new();
            let provider = crate::script_facts::provider();
            for (index, ((file, script), parsed)) in model
                .data
                .scripts()
                .into_iter()
                .zip(cache.parsed_scripts(&model.data))
                .enumerate()
            {
                let unit = ScriptUnit::new(file, script, parsed, index);
                provider.property_accesses(&unit, view, &mut expected);
            }
            assert_eq!(accesses, expected, "round {round}: {scripts:?}");
            // Vetting: the reason names the first script in order with code
            // that is not vetted.
            let found = f.lint(&["VIEW015"]);
            // Unless a script may read every property.
            let Some(unused) = found.iter().find(|d| d.message.contains("`unused`")) else {
                continue;
            };
            vetted += 1;
            let first = scripts
                .iter()
                .position(|s| s.contains("system.nothere"))
                .map(|i| i as u32 + 3);
            let reason = unused.fix.as_ref().and_then(|f| f.unsafe_reason.as_ref());
            if let Some(line) = first {
                let site = reason
                    .filter(|r| r.message.contains("not vetted") && !r.workspace)
                    .and_then(|r| r.site.as_ref())
                    .unwrap_or_else(|| panic!("round {round}: {reason:?} {scripts:?}"));
                assert_eq!(site.line, line, "round {round}: {scripts:?}");
                named += 1;
            }
        }
        assert!(vetted > 10 && named > 5, "{vetted} {named}");
    }
}
