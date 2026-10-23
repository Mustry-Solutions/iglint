//! The check driver: discovery, parallel analysis, fixes and caching.
//!
//! [`check`] is the single entry point used by the CLI, the language server
//! and the ign-lint compatible mode.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iglint_text::TextEdit;
use rayon::prelude::*;

use crate::cache::Cache;
use crate::diagnostic::{Diagnostic, SourceFile};
use crate::discovery::{self, FileKind, SkipReason};
use crate::fix;
use crate::registry::RuleCode;
use crate::safety::{self, ScriptFacts, ScriptText, Scripts};
use crate::settings::{LinterSettings, ProjectSettings};
use rustc_hash::{FxHashMap, FxHashSet};

/// Whether and how fixes are applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FixMode {
    /// Report only.
    #[default]
    None,
    /// Apply safe fixes and write the files.
    Safe,
    /// Apply safe and unsafe fixes and write the files.
    Unsafe,
    /// Compute fixes (safe, plus unsafe when asked) without writing; the
    /// fixed text is returned in [`FixSummary::fixed`].
    DiffOnly { unsafe_fixes: bool },
}

impl FixMode {
    pub const fn is_fixing(self) -> bool {
        !matches!(self, FixMode::None)
    }

    pub const fn writes(self) -> bool {
        matches!(self, FixMode::Safe | FixMode::Unsafe)
    }

    pub const fn unsafe_fixes(self) -> bool {
        matches!(
            self,
            FixMode::Unsafe | FixMode::DiffOnly { unsafe_fixes: true }
        )
    }
}

/// Lints one file in isolation. Tests and tools can substitute this for the
/// project pipeline ([`crate::analyze`]) through [`CheckOptions::analyzer`].
pub type Analyzer =
    Arc<dyn Fn(&SourceFile, FileKind, &LinterSettings) -> Vec<Diagnostic> + Send + Sync>;

/// Options of one check run that are not settings of the project.
#[derive(Clone)]
pub struct CheckOptions {
    pub fix_mode: FixMode,
    /// Only fixes of these rules are applied, when set.
    pub fix_rules: Option<BTreeSet<RuleCode>>,
    /// Upper bound of analyse-and-fix rounds per file.
    pub max_fix_passes: usize,
    /// Worker threads; `None` uses one per CPU.
    pub threads: Option<usize>,
    /// Directory of the result cache; `None` disables caching.
    pub cache_dir: Option<PathBuf>,
    /// Lint this text as if it were the file at this path, instead of
    /// discovering files. Fixed text is returned, never written.
    pub stdin: Option<(PathBuf, String)>,
    /// Replaces the project pipeline with a per-file analyzer (tests).
    pub analyzer: Option<Analyzer>,
    /// Free the parse trees and models at the end of the run. A process
    /// that exits right after the run sets this to `false`: the operating
    /// system reclaims the memory at once, while freeing millions of nodes
    /// one by one takes a noticeable share of a run.
    pub release_memory: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        CheckOptions {
            fix_mode: FixMode::None,
            fix_rules: None,
            max_fix_passes: 10,
            threads: None,
            cache_dir: None,
            stdin: None,
            analyzer: None,
            release_memory: true,
        }
    }
}

impl std::fmt::Debug for CheckOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckOptions")
            .field("fix_mode", &self.fix_mode)
            .field("fix_rules", &self.fix_rules)
            .field("max_fix_passes", &self.max_fix_passes)
            .field("threads", &self.threads)
            .field("cache_dir", &self.cache_dir)
            .field("stdin", &self.stdin.as_ref().map(|(p, _)| p))
            .finish_non_exhaustive()
    }
}

/// A fix that was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedFix {
    pub rule: RuleCode,
    pub message: String,
}

/// A fix that was not applied, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedFix {
    pub rule: RuleCode,
    pub message: String,
    pub reason: String,
    /// The fix was dropped because applying it made the file worse (a
    /// script stopped compiling or the JSON stopped parsing), or because
    /// a safe fix failed the safe-fix net ([`crate::safety`]).
    pub harmful: bool,
}

/// Re-analyses fixed text: the new text, the edits that produced it (in
/// the previous text's coordinates) and what to compute.
pub(crate) type Reanalyze<'r> = dyn Fn(&str, &[TextEdit], Reanalysis) -> Vec<Diagnostic> + 'r;

/// What a re-analysis during fixing computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reanalysis {
    /// The findings of the enabled rules.
    Lint,
    /// The findings of [`health_settings`] (whether the file and its
    /// scripts and expressions still parse and compile), whatever the
    /// configuration.
    Health,
}

/// The rules that decide whether fixed text is still valid: a fix must
/// never add a finding of these.
pub(crate) fn health_settings() -> &'static Arc<LinterSettings> {
    static SETTINGS: std::sync::LazyLock<Arc<LinterSettings>> = std::sync::LazyLock::new(|| {
        use crate::rules::{bind, script, view};
        let metas = [
            &view::invalid_json::META,
            &view::empty_script::META,
            &script::syntax_error::META,
            &script::compile_error::META,
            &bind::expression_syntax_error::META,
        ];
        Arc::new(LinterSettings {
            rules: metas
                .into_iter()
                .map(|m| {
                    (
                        RuleCode::new(m),
                        crate::settings::RuleSettings {
                            severity: m.default_severity,
                            options: Default::default(),
                        },
                    )
                })
                .collect(),
            strict: false,
        })
    });
    &SETTINGS
}

/// The health findings of a text, as a multiset of what they say.
fn health(
    reanalyze: &Reanalyze<'_>,
    text: &str,
    edits: &[TextEdit],
) -> FxHashMap<(RuleCode, String, Option<String>), u32> {
    let mut out = FxHashMap::default();
    for d in reanalyze(text, edits, Reanalysis::Health) {
        *out.entry((d.rule, d.message, d.node_path)).or_default() += 1;
    }
    out
}

/// The first finding `after` has more often than `before`.
/// For a `.py` script (a library module), whether an edit changes the
/// docstring of the module or of any function or class body in it. A
/// statement deletion can promote a following bare string literal to a
/// docstring, changing `__doc__`, which code can read; the health
/// reanalysis does not see that. Catches the cases the per-statement
/// deletion guard cannot (several deletions exposing a later string).
fn py_docstring_changed(path: &Path, before: &str, after: &str) -> bool {
    use iglint_python::ast::{Expr, Stmt};
    use iglint_python::visitor::{Visitor, walk_stmt};

    if path.extension().and_then(|e| e.to_str()) != Some("py") {
        return false;
    }
    let (b, a) = (
        iglint_python::parse_module(before),
        iglint_python::parse_module(after),
    );
    if !b.errors.is_empty() || !a.errors.is_empty() {
        // Parse regressions are the health check's job.
        return false;
    }
    fn first_doc(body: &[Stmt]) -> Option<String> {
        match body.first() {
            Some(Stmt::Expr(e)) => match &*e.value {
                Expr::Str(s) => Some(s.s.to_string_lossy().into_owned()),
                _ => None,
            },
            _ => None,
        }
    }
    struct Walk(Vec<String>);
    impl<'a> Visitor<'a> for Walk {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(f) => self.0.extend(first_doc(&f.body)),
                Stmt::ClassDef(c) => self.0.extend(first_doc(&c.body)),
                _ => {}
            }
            walk_stmt(self, stmt);
        }
    }
    // The sorted multiset of the actual docstrings, so promoting a bare
    // string to a docstring (a new entry), changing one or removing one is
    // caught, while merely adding or removing a scope (an SCR301 wrap,
    // whose bodies have no docstring) is not.
    let docstrings = |body: &[Stmt]| {
        let mut w = Walk(first_doc(body).into_iter().collect());
        for stmt in body {
            w.visit_stmt(stmt);
        }
        w.0.sort();
        w.0
    };
    docstrings(&b.module.body) != docstrings(&a.module.body)
}

fn new_problem(
    before: &FxHashMap<(RuleCode, String, Option<String>), u32>,
    after: &FxHashMap<(RuleCode, String, Option<String>), u32>,
) -> Option<String> {
    let mut found: Vec<_> = after
        .iter()
        .filter(|(k, n)| before.get(*k).copied().unwrap_or(0) < **n)
        .map(|((rule, message, _), _)| format!("{}: {message}", rule.code()))
        .collect();
    found.sort();
    found.into_iter().next()
}

/// Fixes for one file.
#[derive(Clone, Debug)]
pub struct FixSummary {
    pub path: PathBuf,
    pub applied: Vec<AppliedFix>,
    pub skipped: Vec<SkippedFix>,
    pub original: Arc<str>,
    /// The text after all applied fixes.
    pub fixed: String,
    /// Whether the file on disk was rewritten.
    pub written: bool,
}

impl FixSummary {
    pub fn changed(&self) -> bool {
        *self.original != *self.fixed
    }
}

/// A file that could not be linted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// A file that was linted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedFile {
    pub path: PathBuf,
    pub kind: FileKind,
    /// Served from the cache.
    pub cached: bool,
    pub read_time: Duration,
    pub analysis_time: Duration,
}

/// Wall-clock durations of the phases of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timings {
    pub discovery: Duration,
    pub analysis: Duration,
    pub cache: Duration,
    pub total: Duration,
}

/// Everything a check run produced.
#[derive(Clone, Debug, Default)]
pub struct CheckResult {
    /// Remaining findings (after fixes), sorted by path and position.
    pub diagnostics: Vec<Diagnostic>,
    pub files: Vec<CheckedFile>,
    pub files_checked: usize,
    pub files_skipped: usize,
    pub skipped: Vec<(PathBuf, SkipReason)>,
    pub fixed: Vec<FixSummary>,
    pub errors: Vec<FileError>,
    /// Warnings about the run that do not fail it, for example that pylint
    /// (`SCR900`) could not run.
    pub notices: Vec<String>,
    pub timings: Timings,
    /// With [`CheckOptions::stdin`] and a fix mode: the fixed text.
    pub stdin_output: Option<String>,
}

impl CheckResult {
    pub fn fixes_applied(&self) -> usize {
        self.fixed.iter().map(|f| f.applied.len()).sum()
    }
}

/// Runs a check over `paths` (files or directories).
pub fn check(settings: &ProjectSettings, paths: &[PathBuf], options: &CheckOptions) -> CheckResult {
    let start = Instant::now();
    let mut result = CheckResult::default();

    let discovery_start = Instant::now();
    let files: Vec<(PathBuf, FileKind, Option<String>)> = match &options.stdin {
        Some((path, text)) => {
            let kind = FileKind::classify(path).unwrap_or(
                if path.extension().is_some_and(|e| e == "py") {
                    FileKind::Python
                } else {
                    FileKind::Json
                },
            );
            vec![(path.clone(), kind, Some(text.clone()))]
        }
        None => {
            let found = discovery::discover(settings, paths);
            result.skipped = found.skipped;
            result.errors.extend(
                found
                    .errors
                    .into_iter()
                    .map(|(path, message)| FileError { path, message }),
            );
            found
                .files
                .into_iter()
                .map(|(path, kind)| (path, kind, None))
                .collect()
        }
    };
    result.files_skipped = result.skipped.len();
    result.timings.discovery = discovery_start.elapsed();

    let cache_start = Instant::now();
    let cache = options
        .cache_dir
        .as_ref()
        .filter(|_| options.stdin.is_none())
        .map(|dir| Cache::load(dir, settings));
    result.timings.cache = cache_start.elapsed();

    let analysis_start = Instant::now();
    let outcomes = match &options.analyzer {
        Some(analyzer) => {
            let run = || {
                files
                    .par_iter()
                    .map(|(path, kind, text)| {
                        lint_one(
                            settings,
                            options,
                            analyzer,
                            cache.as_ref(),
                            path,
                            *kind,
                            text.as_deref(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            in_pool(options.threads, run)
        }
        None => {
            let targets: Vec<crate::analyze::Target> = files
                .iter()
                .map(|(path, kind, text)| crate::analyze::Target {
                    path: path.clone(),
                    kind: *kind,
                    stdin: text.as_deref().map(Arc::from),
                    settings: settings.for_file(path),
                })
                .collect();
            let run = || crate::analyze::analyze(settings, paths, targets, options, cache.as_ref());
            let analysis = in_pool(options.threads, run);
            result.errors.extend(analysis.errors);
            result.notices.extend(analysis.notices);
            result.skipped.extend(analysis.skipped);
            result.files_skipped = result.skipped.len();
            analysis.outcomes
        }
    };
    result.timings.analysis = analysis_start.elapsed();

    let mut new_cache_entries = Vec::new();
    for outcome in outcomes {
        match outcome {
            Err(error) => result.errors.push(error),
            Ok(outcome) => {
                result.files_checked += 1;
                result.files.push(outcome.checked);
                result.diagnostics.extend(outcome.diagnostics);
                if let Some(summary) = outcome.fix {
                    if options.stdin.is_some() {
                        result.stdin_output = Some(summary.fixed.clone());
                    }
                    if !summary.applied.is_empty() || !summary.skipped.is_empty() {
                        result.fixed.push(summary);
                    }
                }
                if let Some(entry) = outcome.cache_entry {
                    new_cache_entries.push(entry);
                }
            }
        }
    }
    if options.stdin.is_some() && result.stdin_output.is_none() && options.fix_mode.is_fixing() {
        result.stdin_output = files.first().and_then(|f| f.2.clone());
    }
    if let Some(cache) = cache {
        let cache_start = Instant::now();
        cache.store(new_cache_entries);
        result.timings.cache += cache_start.elapsed();
    }
    crate::rules::script::pylint::drop_duplicates(&mut result.diagnostics);
    result
        .diagnostics
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    result.errors.sort_by(|a, b| a.path.cmp(&b.path));
    result.timings.total = start.elapsed();
    result
}

fn in_pool<T: Send>(threads: Option<usize>, run: impl FnOnce() -> T + Send) -> T {
    match threads {
        Some(threads) => match crate::stack::pool_builder().num_threads(threads).build() {
            Ok(pool) => pool.install(run),
            Err(_) => run(),
        },
        None => run(),
    }
}

/// What linting one file produced.
pub(crate) struct Outcome {
    pub checked: CheckedFile,
    pub diagnostics: Vec<Diagnostic>,
    pub fix: Option<FixSummary>,
    pub cache_entry: Option<crate::cache::Entry>,
}

fn lint_one(
    settings: &ProjectSettings,
    options: &CheckOptions,
    analyzer: &Analyzer,
    cache: Option<&Cache>,
    path: &Path,
    kind: FileKind,
    stdin: Option<&str>,
) -> Result<Outcome, FileError> {
    let read_start = Instant::now();
    let text: Arc<str> = match stdin {
        Some(text) => Arc::from(text),
        None => match std::fs::read(path) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Arc::from(text),
                Err(_) => {
                    return Err(FileError {
                        path: path.to_path_buf(),
                        message: "file is not valid UTF-8".to_owned(),
                    });
                }
            },
            Err(error) => {
                return Err(FileError {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                });
            }
        },
    };
    let read_time = read_start.elapsed();
    let file_settings = settings.for_file(path);
    let analysis_start = Instant::now();
    let source = SourceFile::new(path, Arc::clone(&text));

    let cached = cache.and_then(|c| c.get(path, kind, &file_settings, &source, 0));
    let from_cache = cached.is_some();
    let diagnostics = match cached {
        Some(diagnostics) => diagnostics,
        None => analyzer(&source, kind, &file_settings),
    };
    let cache_entry = match (cache, from_cache) {
        (Some(c), false) => c.entry(path, kind, &file_settings, &source, 0, &diagnostics),
        _ => None,
    };
    let reanalyze = |text: &str, _edits: &[TextEdit], what: Reanalysis| {
        let fixed_source = SourceFile::new(path, text);
        match what {
            Reanalysis::Lint => analyzer(&fixed_source, kind, &file_settings),
            Reanalysis::Health => analyzer(&fixed_source, kind, health_settings()),
        }
    };
    let scripts = |text: &str| crate::analyze::standalone_scripts(path, Arc::from(text));
    let (fix_summary, diagnostics) = fix_file(
        settings,
        options,
        path,
        &text,
        diagnostics,
        &reanalyze,
        &scripts,
    )?;

    Ok(Outcome {
        checked: CheckedFile {
            path: path.to_path_buf(),
            kind,
            cached: from_cache,
            read_time,
            analysis_time: analysis_start.elapsed(),
        },
        diagnostics,
        fix: fix_summary,
        cache_entry,
    })
}

/// Applies fixes to one file in passes, re-analysing after each pass with
/// `reanalyze` (given the new text and the edits of the pass, in the
/// previous text's coordinates), and writes the result when the fix mode
/// says so. Returns the summary (in fix mode) and the remaining findings.
///
/// A pass whose fixes together make the file worse (a script or
/// expression that no longer parses or compiles, an empty Perspective
/// script, JSON that no longer parses: findings of [`health_settings`]
/// that were not there before) is redone one fix at a time, dropping
/// every fix that makes it worse; dropped fixes are reported as skipped
/// and not retried. The file is never written worse than it was. Safe
/// fixes also pass the safe-fix net ([`crate::safety`]) on the scripts of
/// the file (`scripts` lists them for a text): a safe fix that would move
/// a line, change whether a script compiles, a docstring or what a name
/// resolves to is dropped the same way.
pub(crate) fn fix_file(
    settings: &ProjectSettings,
    options: &CheckOptions,
    path: &Path,
    text: &Arc<str>,
    mut diagnostics: Vec<Diagnostic>,
    reanalyze: &Reanalyze<'_>,
    scripts: &Scripts<'_>,
) -> Result<(Option<FixSummary>, Vec<Diagnostic>), FileError> {
    if !options.fix_mode.is_fixing() {
        return Ok((None, diagnostics));
    }
    let unsafe_fixes = options.fix_mode.unsafe_fixes();
    // Fixes the safety net dropped: (rule, fix message, finding message).
    let rejected: std::cell::RefCell<FxHashSet<(RuleCode, String, String)>> =
        std::cell::RefCell::default();
    let wanted = |d: &Diagnostic| -> Result<(), String> {
        let fix = d.fix.as_ref().expect("only called for fixes");
        if settings.unfixable.contains(&d.rule) {
            return Err("the rule is marked unfixable".to_owned());
        }
        if let Some(rules) = &options.fix_rules
            && !rules.contains(&d.rule)
        {
            return Err("filtered by the selected fix rules".to_owned());
        }
        if !fix::applicability_allowed(fix.applicability, unsafe_fixes) {
            return Err(match fix.applicability {
                crate::diagnostic::Applicability::Unsafe => {
                    "unsafe fix (use --unsafe-fixes)".to_owned()
                }
                _ => "the fix is only a suggestion".to_owned(),
            });
        }
        if rejected
            .borrow()
            .contains(&(d.rule, fix.message.clone(), d.message.clone()))
        {
            return Err("the fix makes the file worse".to_owned());
        }
        Ok(())
    };
    let mut applied = Vec::new();
    let mut current = String::from(&**text);
    let mut skipped = Vec::new();
    let mut harmful: Vec<SkippedFix> = Vec::new();
    let mut current_health: Option<FxHashMap<_, _>> = None;
    // The scripts of `current` and what they do, for the safe-fix net.
    let mut current_scripts: Option<(Vec<ScriptText>, Vec<ScriptFacts>)> = None;
    // Set once no further fix applies: the fixes have reached a fixpoint
    // and a second `--fix` run would change nothing. If the pass budget
    // runs out while fixes are still being applied, this stays false and
    // the run reports a non-convergence error.
    let mut converged = false;
    for pass in 0..options.max_fix_passes.max(1) {
        let mut pass_skipped = Vec::new();
        let mut pass_applied = Vec::new();
        let Some(pass_result) = fix::apply_pass(
            path,
            &current,
            &diagnostics,
            &wanted,
            &mut pass_applied,
            &mut pass_skipped,
        ) else {
            skipped = pass_skipped;
            converged = true;
            break;
        };
        let before = current_health
            .get_or_insert_with(|| health(reanalyze, &current, &[]))
            .clone();
        if pass_result.safe.iter().any(|s| *s) && current_scripts.is_none() {
            let found = scripts(&current);
            let facts = found.iter().map(ScriptFacts::of).collect();
            current_scripts = Some((found, facts));
        }
        // Why the safe fixes with `safe_edits` (in `current`'s
        // coordinates) change a script, if they do.
        let safe_problem = |safe_edits: &[TextEdit]| -> Option<String> {
            if safe_edits.is_empty() {
                return None;
            }
            let (before_scripts, before_facts) = current_scripts.as_ref()?;
            let Ok(after) = iglint_text::apply_edits(&current, safe_edits) else {
                return Some("its edits overlap".to_owned());
            };
            let mut sorted = safe_edits.to_vec();
            sorted.sort_by_key(|e| e.range);
            safety::safe_fix_problem(
                path,
                &current,
                before_scripts,
                before_facts,
                &sorted,
                &scripts(&after),
            )
        };
        let safe_edits: Vec<TextEdit> = pass_result
            .fixes
            .iter()
            .zip(&pass_result.safe)
            .filter(|(_, safe)| **safe)
            .flat_map(|((_, edits), _)| edits.iter().cloned())
            .collect::<FxHashSet<_>>()
            .into_iter()
            .collect();
        let after = health(reanalyze, &pass_result.text, &pass_result.edits);
        let pass_ok = new_problem(&before, &after).is_none()
            && !py_docstring_changed(path, &current, &pass_result.text)
            && safe_problem(&safe_edits).is_none();
        let (next, edits, after) = if pass_ok {
            applied.extend(pass_applied);
            (pass_result.text, pass_result.edits, after)
        } else {
            // One fix at a time, keeping those that do not make it worse.
            let mut kept: Vec<TextEdit> = Vec::new();
            let mut kept_safe: Vec<TextEdit> = Vec::new();
            let mut kept_text = None;
            let mut kept_health = before.clone();
            for (fixed, ((applied_fix, edits), safe)) in pass_result
                .fixes
                .into_iter()
                .zip(pass_result.safe)
                .enumerate()
            {
                // The same edits as a fix already kept (one fix reported
                // for several findings).
                if !edits.is_empty() && edits.iter().all(|e| kept.contains(e)) {
                    applied.push(applied_fix);
                    continue;
                }
                // Whether this fix is being tried on its own. A fix that
                // makes the file worse only in combination with others is
                // retried in a later pass (where the others have been
                // applied or dropped); only a fix that fails on its own is
                // blocked for good, so that `--fix` reaches a fixpoint in
                // one invocation instead of needing a second run.
                let solo = kept.is_empty();
                let mut trial = kept.clone();
                trial.extend(edits.iter().cloned());
                let Ok(trial_text) = iglint_text::apply_edits(&current, &trial) else {
                    continue;
                };
                let mut trial_safe = kept_safe.clone();
                if safe {
                    trial_safe.extend(edits.iter().cloned());
                }
                let trial_health = health(reanalyze, &trial_text, &trial);
                let problem = new_problem(&before, &trial_health)
                    .or_else(|| {
                        py_docstring_changed(path, &current, &trial_text)
                            .then(|| "it would change a docstring".to_owned())
                    })
                    .map(|p| format!("the fix makes the file worse ({p})"))
                    .or_else(|| {
                        safe.then(|| safe_problem(&trial_safe))
                            .flatten()
                            .map(|p| format!("the safe fix could change what the code does ({p})"))
                    });
                match problem {
                    None => {
                        kept = trial;
                        kept_safe = trial_safe;
                        kept_text = Some(trial_text);
                        kept_health = trial_health;
                        applied.push(applied_fix);
                    }
                    Some(problem) if solo => {
                        let d = &pass_result.diagnostics[fixed];
                        rejected.borrow_mut().insert((
                            d.0,
                            applied_fix.message.clone(),
                            d.1.clone(),
                        ));
                        harmful.push(SkippedFix {
                            rule: applied_fix.rule,
                            message: applied_fix.message,
                            reason: problem,
                            harmful: true,
                        });
                    }
                    // Rejected only in combination: leave it for a later
                    // pass rather than reporting or blocking it.
                    Some(_) => {}
                }
            }
            let Some(kept_text) = kept_text else {
                // Nothing could be kept this pass, so `current` is
                // unchanged and another pass would do the same: the run
                // has reached a fixpoint. Every rejection here was tried
                // on its own (nothing was kept before it), so it is
                // already recorded.
                skipped = pass_skipped;
                converged = true;
                break;
            };
            (kept_text, kept, kept_health)
        };
        current = next;
        current_health = Some(after);
        current_scripts = None;
        diagnostics = reanalyze(&current, &edits, Reanalysis::Lint);
        if pass + 1 == options.max_fix_passes.max(1) {
            skipped = pass_skipped;
        }
    }
    // Single-pass mode (`max_fix_passes == 1`, used by the language
    // server's quick fixes) applies one round by design and does not aim
    // for a fixpoint, so non-convergence is only a problem when iterating.
    if !converged && options.max_fix_passes.max(1) > 1 {
        // The pass budget ran out while fixes were still being applied.
        // Rather than write a file that a further `--fix` would keep
        // changing, report it: this is a bug in a fix (fixes should reach
        // a fixpoint quickly), not something the user can resolve.
        return Err(FileError {
            path: path.to_path_buf(),
            message: format!(
                "fixes did not converge after {} passes; this is a bug, please report it",
                options.max_fix_passes.max(1)
            ),
        });
    }
    skipped.extend(harmful);
    let mut written = false;
    if options.fix_mode.writes() && options.stdin.is_none() && *current != **text {
        write_atomically(path, &current).map_err(|error| FileError {
            path: path.to_path_buf(),
            message: format!("failed to write fixes: {error}"),
        })?;
        written = true;
    }
    Ok((
        Some(FixSummary {
            path: path.to_path_buf(),
            applied,
            skipped,
            original: Arc::clone(text),
            fixed: current,
            written,
        }),
        diagnostics,
    ))
}

/// Writes `text` to a temporary file next to `path` and renames it over
/// `path`, keeping the original permissions.
pub fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dir.join(format!(".{name}.iglint-{}.tmp", std::process::id()));
    std::fs::write(&temp, text)?;
    if let Ok(metadata) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&temp, metadata.permissions());
    }
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

#[cfg(test)]
mod idempotency_tests {
    use super::*;
    use crate::rules::script::testing::{library, library_path};

    /// Fixing is a fixpoint: a second `--fix` run over the same text
    /// changes nothing. Covers hand-written programs that exercise the
    /// combined-deletion and docstring-promotion paths, plus fuzzed
    /// programs built from fix-triggering fragments (reviewer round 7,
    /// finding 3). `FixMode::DiffOnly` is the same path `--fix` writes.
    fn fix_twice(code: &str) {
        let rules = [
            ("SCR102", ""),
            ("SCR103", ""),
            ("SCR114", ""),
            ("SCR123", ""),
        ];
        for unsafe_fixes in [false, true] {
            let f = library("m", code);
            let path = library_path(&f, "m");
            let first = f.run(&rules, FixMode::DiffOnly { unsafe_fixes });
            let once = first
                .fixed
                .iter()
                .find(|s| s.path == path)
                .map_or_else(|| code.to_owned(), |s| s.fixed.clone());
            // Re-run over the already-fixed text: it must not change.
            let g = library("m", &once);
            let gpath = library_path(&g, "m");
            let second = g.run(&rules, FixMode::DiffOnly { unsafe_fixes });
            let twice = second
                .fixed
                .iter()
                .find(|s| s.path == gpath)
                .map_or_else(|| once.clone(), |s| s.fixed.clone());
            assert_eq!(once, twice, "not idempotent ({unsafe_fixes}) for:\n{code}");
        }
    }

    #[test]
    fn hand_written_programs_fix_to_a_fixpoint() {
        for code in [
            "def f():\n    import math\n    import sys\n    \"doc\"\n    return 1\n",
            "import os\nimport sys\n\"module docstring\"\n",
            "def f():\n    import os\n    'promoted'\n    return f.__doc__\n",
            "def f():\n    import os\n    import sys\n    import re\n    return 1\n",
            "import json\nimport os, sys\nprint json.dumps(1)\n",
            "def f(x):\n    if x:\n        import os\n        import sys\n    return x\n",
            "x = 1  # iglint: ignore[SCR101]\ny = 2  # iglint: ignore[SCR101] reason\n",
        ] {
            fix_twice(code);
        }
    }

    #[test]
    fn fuzzed_programs_fix_to_a_fixpoint() {
        // A small deterministic LCG builds programs out of fragments that
        // trigger safe and unsafe fixes, nested in functions and suites.
        let fragments = [
            "import math\n",
            "import os\n",
            "import sys\n",
            "import re\n",
            "import json\n",
            "'a bare string'\n",
            "\"doc\"\n",
            "x = 1\n",
            "pass\n",
            "return 1\n",
            "print x\n",
        ];
        let mut state: u64 = 0x9e3779b97f4a7c15;
        let mut next = |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state as usize) % n
        };
        for _ in 0..400 {
            let count = 1 + next(5);
            let mut body = String::new();
            for _ in 0..count {
                body.push_str("    ");
                body.push_str(fragments[next(fragments.len())]);
            }
            let code = format!("def f():\n{body}");
            if iglint_python::parse_module(&code).errors.is_empty() {
                fix_twice(&code);
            }
            // Also at module scope.
            let mut mody = String::new();
            for _ in 0..count {
                mody.push_str(fragments[next(fragments.len())]);
            }
            if iglint_python::parse_module(&mody).errors.is_empty() {
                fix_twice(&mody);
            }
        }
    }
}
