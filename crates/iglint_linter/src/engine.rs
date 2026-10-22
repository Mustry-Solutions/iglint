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
use crate::settings::{LinterSettings, ProjectSettings};

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
    result
        .diagnostics
        .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    result.errors.sort_by(|a, b| a.path.cmp(&b.path));
    result.timings.total = start.elapsed();
    result
}

fn in_pool<T: Send>(threads: Option<usize>, run: impl FnOnce() -> T + Send) -> T {
    match threads {
        Some(threads) => match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
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
    let reanalyze = |text: &str, _edits: &[TextEdit]| {
        let fixed_source = SourceFile::new(path, text);
        analyzer(&fixed_source, kind, &file_settings)
    };
    let (fix_summary, diagnostics) =
        fix_file(settings, options, path, &text, diagnostics, &reanalyze)?;

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
/// previous text's coordinates), and writes the result when the fix mode says so. Returns
/// the summary (in fix mode) and the remaining findings.
pub(crate) fn fix_file(
    settings: &ProjectSettings,
    options: &CheckOptions,
    path: &Path,
    text: &Arc<str>,
    mut diagnostics: Vec<Diagnostic>,
    reanalyze: &dyn Fn(&str, &[TextEdit]) -> Vec<Diagnostic>,
) -> Result<(Option<FixSummary>, Vec<Diagnostic>), FileError> {
    if !options.fix_mode.is_fixing() {
        return Ok((None, diagnostics));
    }
    let unsafe_fixes = options.fix_mode.unsafe_fixes();
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
        Ok(())
    };
    let mut applied = Vec::new();
    let mut current = String::from(&**text);
    let mut skipped = Vec::new();
    for pass in 0..options.max_fix_passes.max(1) {
        let mut pass_skipped = Vec::new();
        let Some((next, edits)) = fix::apply_pass(
            path,
            &current,
            &diagnostics,
            &wanted,
            &mut applied,
            &mut pass_skipped,
        ) else {
            skipped = pass_skipped;
            break;
        };
        current = next;
        diagnostics = reanalyze(&current, &edits);
        if pass + 1 == options.max_fix_passes.max(1) {
            skipped = pass_skipped;
        }
    }
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
