//! `iglint ign-lint`: a drop-in replacement for the ign-lint 0.6.4 CLI.
//!
//! Every ign-lint flag is accepted with the same meaning, output layout,
//! report files and exit codes (`docs/compat/ign-lint.md`, sections 8 and 9).
//! The findings come from iglint's rules that declare an ign-lint
//! equivalent. Where the spec documents an ign-lint bug, this mode behaves
//! correctly and keeps the output format:
//!
//! * an invalid JSON file is reported and skipped instead of aborting the run
//!   (the exit code is still 1);
//! * `--version` prints the real version;
//! * `--stats-only --timing-output` writes the timing report;
//! * `--debug-output` mirrors each file's path instead of overwriting;
//! * `--debug-nodes`/`--analyze-rules` count every node once;
//! * rules are reported in a fixed order;
//! * fixes edit the file text in place instead of reformatting the file.

pub mod files;
pub mod model;
pub mod report;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

use clap::{Arg, ArgAction, ArgMatches, Command};
use iglint_config::compat::{self, CompatMode, CompatSettings, RuleConfig, ign_lint_rule};
use iglint_linter::diagnostic::Applicability;
use iglint_linter::discovery::absolute;
use iglint_linter::engine::{self, CheckOptions, CheckResult, FixMode};
use iglint_linter::{Diagnostic, RuleCode, Severity};
use serde_json::{Value, json};

use crate::{Environment, ExitStatus, Io};
use model::{Flat, NodeType, ViewModel};
use report::{FileReport, FileTiming, RuleBlock, Totals};

const WIDE_EQ: &str = "========================================================================================================================";

fn command(program: &str) -> Command {
    let flag = |name: &'static str, help: &'static str| {
        Arg::new(name)
            .long(name)
            .action(ArgAction::SetTrue)
            .help(help)
    };
    Command::new("ign-lint")
        .bin_name(program.to_owned())
        .about("Lint Ignition Perspective view.json files (ign-lint compatible mode of iglint)")
        .disable_version_flag(true)
        .infer_long_args(true)
        .arg(flag("version", "Show the version and exit"))
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("PATH")
                .default_value("rule_config.json")
                .help("Rule configuration file"),
        )
        .arg(
            Arg::new("files")
                .long("files")
                .value_name("VALUE")
                .help("A glob or file, or a comma-separated list of them (default: **/view.json)"),
        )
        .arg(
            Arg::new("filenames")
                .value_name("FILE")
                .num_args(0..)
                .help("Files to lint"),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::SetTrue)
                .help("Show rule breakdown, per-file statistics and whitelist details"),
        )
        .arg(flag("stats-only", "Print model statistics without linting"))
        .arg(
            Arg::new("debug-nodes")
                .long("debug-nodes")
                .value_name("TYPE")
                .num_args(0..)
                .help("Print up to 10 nodes (of the given types) per file"),
        )
        .arg(flag(
            "analyze-rules",
            "Show which nodes every rule processes",
        ))
        .arg(
            Arg::new("debug-output")
                .long("debug-output")
                .value_name("DIR")
                .help("Write the flattened map, model and statistics per file"),
        )
        .arg(flag("ignore-warnings", "Warnings do not fail the run"))
        .arg(
            Arg::new("timing-output")
                .long("timing-output")
                .value_name("PATH")
                .help("Write a timing report"),
        )
        .arg(
            Arg::new("results-output")
                .long("results-output")
                .value_name("PATH")
                .help("Write the results report"),
        )
        .arg(
            Arg::new("whitelist")
                .long("whitelist")
                .value_name("PATH")
                .help("Skip the files listed in PATH"),
        )
        .arg(flag("no-whitelist", "Ignore --whitelist"))
        .arg(
            Arg::new("generate-whitelist")
                .long("generate-whitelist")
                .value_name("PATTERN")
                .num_args(1..)
                .help("Write a whitelist of the files matching the patterns and exit"),
        )
        .arg(
            Arg::new("whitelist-output")
                .long("whitelist-output")
                .value_name("PATH")
                .default_value(".whitelist.txt")
                .help("Target of --generate-whitelist"),
        )
        .arg(flag("append", "Merge with the existing whitelist"))
        .arg(flag("dry-run", "Print what would be whitelisted"))
        .arg(flag("fix", "Apply safe fixes"))
        .arg(flag("fix-unsafe", "Apply safe and unsafe fixes"))
        .arg(flag(
            "fix-dry-run",
            "Report results and print the proposed fixes",
        ))
        .arg(
            Arg::new("fix-rules")
                .long("fix-rules")
                .value_name("A,B")
                .help("Only apply fixes of these rules (overrides allow_fix: false)"),
        )
}

/// Parsed command line.
struct Options {
    config: String,
    files: Option<String>,
    filenames: Vec<String>,
    verbose: bool,
    stats_only: bool,
    debug_nodes: Option<Vec<String>>,
    analyze_rules: bool,
    debug_output: Option<String>,
    ignore_warnings: bool,
    timing_output: Option<String>,
    results_output: Option<String>,
    whitelist: Option<String>,
    no_whitelist: bool,
    generate_whitelist: Option<Vec<String>>,
    whitelist_output: String,
    append: bool,
    dry_run: bool,
    fix: bool,
    fix_unsafe: bool,
    fix_dry_run: bool,
    fix_rules: Option<String>,
}

impl Options {
    fn from(m: &ArgMatches) -> Options {
        let string = |name: &str| m.get_one::<String>(name).cloned();
        let list = |name: &str| {
            m.get_many::<String>(name)
                .map(|values| values.cloned().collect::<Vec<_>>())
        };
        Options {
            config: string("config").unwrap_or_default(),
            files: string("files"),
            filenames: list("filenames").unwrap_or_default(),
            verbose: m.get_flag("verbose"),
            stats_only: m.get_flag("stats-only"),
            debug_nodes: m
                .contains_id("debug-nodes")
                .then(|| list("debug-nodes").unwrap_or_default())
                .filter(|_| {
                    m.value_source("debug-nodes") == Some(clap::parser::ValueSource::CommandLine)
                }),
            analyze_rules: m.get_flag("analyze-rules"),
            debug_output: string("debug-output"),
            ignore_warnings: m.get_flag("ignore-warnings"),
            timing_output: string("timing-output"),
            results_output: string("results-output"),
            whitelist: string("whitelist"),
            no_whitelist: m.get_flag("no-whitelist"),
            generate_whitelist: list("generate-whitelist"),
            whitelist_output: string("whitelist-output").unwrap_or_default(),
            append: m.get_flag("append"),
            dry_run: m.get_flag("dry-run"),
            fix: m.get_flag("fix"),
            fix_unsafe: m.get_flag("fix-unsafe"),
            fix_dry_run: m.get_flag("fix-dry-run"),
            fix_rules: string("fix-rules"),
        }
    }

    fn fix_mode(&self) -> bool {
        self.fix || self.fix_unsafe || self.fix_dry_run
    }
}

/// Runs ign-lint compatible mode. `args[0]` is the program name as ign-lint
/// would show it (`ign-lint`).
pub fn run(args: Vec<OsString>, environment: &Environment, io: &mut Io<'_>) -> ExitStatus {
    match run_inner(args, environment, io) {
        Ok(status) => status,
        Err(error) => {
            let _ = writeln!(io.stderr, "ERROR: {error}");
            ExitStatus::Error
        }
    }
}

fn status(code: u8) -> ExitStatus {
    match code {
        0 => ExitStatus::Success,
        1 => ExitStatus::Failure,
        _ => ExitStatus::Error,
    }
}

/// One selected file after reading and model building.
struct Prepared {
    display: String,
    absolute: PathBuf,
    state: FileState,
    timing: FileTiming,
}

enum FileState {
    Ready(Box<(Flat, ViewModel)>),
    /// `{}`, `[]` or another falsy JSON document.
    Empty,
    /// Not readable or not JSON: `ERROR: ...` message.
    Invalid(String),
}

/// What the loaded configuration means for this run.
struct Rules {
    config: RuleConfig,
    compat: CompatSettings,
    /// iglint rule -> ign-lint rule name.
    names: HashMap<RuleCode, String>,
    /// Rules whose fixes `--fix-rules` selected.
    fix_rules: Option<BTreeSet<RuleCode>>,
}

fn run_inner(
    args: Vec<OsString>,
    environment: &Environment,
    io: &mut Io<'_>,
) -> io::Result<ExitStatus> {
    let program = args
        .first()
        .and_then(|a| Path::new(a).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ign-lint".to_owned());
    let matches = match command(&program).try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let text = error.render().to_string();
            return Ok(match error.kind() {
                clap::error::ErrorKind::DisplayHelp => {
                    write!(io.stdout, "{text}")?;
                    ExitStatus::Success
                }
                _ => {
                    write!(io.stderr, "{text}")?;
                    ExitStatus::Error
                }
            });
        }
    };
    let options = Options::from(&matches);
    let out = &mut *io.stdout;
    let cwd = environment.cwd.as_path();

    if matches.get_flag("version") {
        writeln!(out, "{program} {}", env!("CARGO_PKG_VERSION"))?;
        return Ok(ExitStatus::Success);
    }

    if let Some(patterns) = &options.generate_whitelist {
        let code = files::generate_whitelist(
            patterns,
            &options.whitelist_output,
            options.append,
            options.dry_run,
            cwd,
            out,
        )?;
        return Ok(status(code));
    }

    let start = Instant::now();
    let now = SystemTime::now();
    for report_file in [&options.timing_output, &options.results_output]
        .into_iter()
        .flatten()
    {
        report::clean_up(&cwd.join(report_file), now);
    }
    if let Some(timing) = &options.timing_output {
        writeln!(out, "🔍 Performance profiling enabled (output: {timing})")?;
    }

    // Configuration.
    let rules = if options.stats_only {
        None
    } else {
        let config_path = cwd.join(&options.config);
        let config = match std::fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                compat::parse_rule_config(Path::new(&options.config), &text).map_err(|e| e.message)
            }) {
            Ok(config) => config,
            Err(message) => {
                writeln!(
                    out,
                    "Error loading config file {}: {message}",
                    options.config
                )?;
                writeln!(out, "❌ No valid configuration found")?;
                return Ok(ExitStatus::Failure);
            }
        };
        writeln!(out, "🔧 Loaded configuration from {}", options.config)?;
        for message in &config.messages {
            writeln!(out, "{message}")?;
        }
        if config.loaded_rules().next().is_none() {
            writeln!(out, "❌ No valid rules configured")?;
            return Ok(ExitStatus::Failure);
        }
        let compat =
            compat::to_settings(&config, environment.registry, cwd, CompatMode::IgnLintOnly);
        let mut names = HashMap::new();
        for (name, codes) in &compat.rule_map {
            let canonical = ign_lint_rule(name).map_or(name.as_str(), |r| r.name);
            for code in codes {
                names.entry(*code).or_insert_with(|| canonical.to_owned());
            }
        }
        let mut rules = Rules {
            config,
            compat,
            names,
            fix_rules: None,
        };
        if let Some(list) = &options.fix_rules {
            let mut selected = BTreeSet::new();
            for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let entry = rules
                    .config
                    .loaded_rules()
                    .find(|e| e.name == name || e.ign_lint.is_some_and(|i| i.name == name));
                match entry {
                    None => writeln!(
                        out,
                        "⚠️  --fix-rules: '{name}' does not match any loaded rule; its fixes cannot apply"
                    )?,
                    Some(entry) if !entry.ign_lint.is_some_and(|i| i.supports_fix) => {
                        writeln!(out, "⚠️  --fix-rules: '{name}' does not support auto-fix")?;
                    }
                    Some(entry) => {
                        if let Some(codes) = rules.compat.rule_map.get(&entry.name) {
                            selected.extend(codes.iter().copied());
                        }
                    }
                }
            }
            for code in &selected {
                rules.compat.settings.unfixable.remove(code);
            }
            rules.fix_rules = Some(selected);
        }
        if options.verbose {
            let loaded: Vec<_> = rules.config.loaded_rules().collect();
            writeln!(out, "\n📋 Rules loaded: {}", loaded.len())?;
            for entry in loaded {
                let codes = rules.compat.rule_map.get(&entry.name);
                let target = match codes {
                    Some(codes) if !codes.is_empty() => codes
                        .iter()
                        .map(|c| c.code())
                        .collect::<Vec<_>>()
                        .join(", "),
                    _ => "no iglint equivalent yet".to_owned(),
                };
                writeln!(out, "  • {} -> {target}", entry.name)?;
            }
            for notice in &rules.compat.notices {
                writeln!(out, "ℹ️  {notice}")?;
            }
        }
        Some(rules)
    };

    // File selection.
    let selection = files::select(options.files.as_deref(), &options.filenames, cwd);
    for message in &selection.messages {
        writeln!(out, "{message}")?;
    }
    if selection.files.is_empty() {
        writeln!(out, "❌ No files specified or found")?;
        return Ok(ExitStatus::Success);
    }
    let whitelist = match (&options.whitelist, options.no_whitelist) {
        (Some(path), false) => {
            let list = files::whitelist(path, cwd)?;
            if options.verbose {
                if list.is_empty() {
                    writeln!(
                        out,
                        "⚠️  Whitelist file specified but empty or not found: {path}"
                    )?;
                } else {
                    writeln!(out, "🔒 Whitelist loaded: {} files from {path}", list.len())?;
                }
            }
            list
        }
        _ => BTreeSet::new(),
    };
    let mut selected = Vec::new();
    let mut whitelisted = Vec::new();
    for file in selection.files {
        if whitelist.contains(&absolute(&cwd.join(&file))) {
            writeln!(out, "🔒 Skipped (whitelisted): {file}")?;
            whitelisted.push(file);
        } else {
            selected.push(file);
        }
    }
    if options.verbose {
        writeln!(out, "📁 Processing {} files", selected.len())?;
    }

    let debug_types = match &options.debug_nodes {
        Some(types) => {
            let mut valid = BTreeSet::new();
            for t in types {
                match NodeType::parse(t) {
                    Some(t) => {
                        valid.insert(t);
                    }
                    None => writeln!(
                        out,
                        "Warning: Unknown node type '{t}'. Available types: {}",
                        NodeType::available_list()
                    )?,
                }
            }
            Some(valid)
        }
        None => None,
    };

    // Read, parse and model every file.
    let prepared: Vec<Prepared> = selected
        .into_iter()
        .map(|display| prepare(cwd, display))
        .collect();

    // Lint.
    let fix_mode = if options.fix_dry_run || !options.fix_mode() || rules.is_none() {
        FixMode::None
    } else if options.fix_unsafe {
        FixMode::Unsafe
    } else {
        FixMode::Safe
    };
    let result = match &rules {
        Some(rules) => {
            let paths: Vec<PathBuf> = prepared
                .iter()
                .filter(|p| matches!(p.state, FileState::Ready(_)))
                .map(|p| p.absolute.clone())
                .collect();
            let mut check_options = CheckOptions {
                fix_mode,
                fix_rules: rules.fix_rules.clone(),
                ..CheckOptions::default()
            };
            if let Some(analyzer) = &environment.analyzer {
                check_options.analyzer = Some(analyzer.clone());
            }
            engine::check(&rules.compat.settings, &paths, &check_options)
        }
        None => CheckResult::default(),
    };
    let mut by_file: HashMap<PathBuf, Vec<&Diagnostic>> = HashMap::new();
    for diagnostic in &result.diagnostics {
        by_file
            .entry(diagnostic.file.path().to_path_buf())
            .or_default()
            .push(diagnostic);
    }
    let analysis_times: HashMap<&Path, std::time::Duration> = result
        .files
        .iter()
        .map(|f| (f.path.as_path(), f.analysis_time))
        .collect();
    let engine_errors: HashMap<&Path, &str> = result
        .errors
        .iter()
        .map(|e| (e.path.as_path(), e.message.as_str()))
        .collect();

    // Report.
    let mut totals = Totals::default();
    let mut reports = Vec::new();
    let mut failed = false;
    for mut file in prepared {
        writeln!(out, "\n📄 Evaluating file:\n    {}", file.display)?;
        let (flat, model) = match &file.state {
            FileState::Invalid(message) => {
                writeln!(
                    io.stderr,
                    "ERROR: Invalid JSON in {}: {message}",
                    file.absolute.display()
                )?;
                failed = true;
                continue;
            }
            FileState::Empty => {
                writeln!(out, "❌ Failed to read file, skipping")?;
                totals.files += 1;
                reports.push(FileReport {
                    path: file.display.clone(),
                    timing: file.timing.clone(),
                    ..FileReport::default()
                });
                continue;
            }
            FileState::Ready(parsed) => (&parsed.0, &parsed.1),
        };
        if let Some(message) = engine_errors.get(file.absolute.as_path()) {
            writeln!(
                io.stderr,
                "ERROR: Failed to lint {}: {message}",
                file.absolute.display()
            )?;
            failed = true;
            continue;
        }
        totals.files += 1;
        if let Some(types) = &debug_types {
            print_debug_nodes(out, model, types)?;
        }
        if options.verbose || options.stats_only {
            print_statistics(out, &file.display, model, rules.as_ref())?;
        }
        if options.analyze_rules
            && let Some(rules) = &rules
        {
            print_rule_analysis(out, model, rules)?;
        }
        if let Some(dir) = &options.debug_output {
            write_debug_output(out, cwd, dir, &file.display, flat, model, rules.as_ref())?;
        }
        if options.stats_only {
            reports.push(FileReport {
                path: file.display.clone(),
                timing: file.timing.clone(),
                ..FileReport::default()
            });
            continue;
        }
        let rules = rules
            .as_ref()
            .expect("rules are loaded unless --stats-only");
        if let Some(summary) = result.fixed.iter().find(|f| f.path == file.absolute) {
            print_applied_fixes(out, &file.display, summary)?;
        }
        let diagnostics = by_file.remove(&file.absolute).unwrap_or_default();
        file.timing.rules = analysis_times
            .get(file.absolute.as_path())
            .copied()
            .unwrap_or_default();
        let file_report = build_report(&file, &diagnostics, rules);
        report::print_file_results(out, &file_report)?;
        if options.fix_dry_run {
            print_proposed_fixes(
                out,
                cwd,
                &file.display,
                &diagnostics,
                rules,
                options.fix_unsafe,
            )?;
        }
        totals.errors += file_report.error_count();
        totals.warnings += file_report.warning_count();
        if file_report.has_issues() {
            totals.files_with_issues += 1;
        }
        reports.push(file_report);
    }

    if let Some(timing) = &options.timing_output {
        let target = cwd.join(timing);
        let (path, _) = report::report_path(&target, now);
        create_parent(&path)?;
        std::fs::write(&path, report::timing_text(&reports, start.elapsed(), now))?;
        writeln!(
            out,
            "\n📊 Timing report written to: {}",
            shown(cwd, &path, timing)
        )?;
    }
    if let Some(results) = &options.results_output
        && !options.stats_only
        && totals.files > 0
    {
        let target = cwd.join(results);
        let (path, batch) = report::report_path(&target, now);
        create_parent(&path)?;
        std::fs::write(&path, report::results_text(&totals, &whitelisted, &reports))?;
        writeln!(
            out,
            "\n📝 Results written to: {}",
            shown(cwd, &path, results)
        )?;
        if batch {
            let aggregated = report::write_aggregated(&target, now)?;
            writeln!(
                out,
                "\n📊 Aggregated summary written to: {}",
                shown(cwd, &aggregated, results)
            )?;
        }
    }

    writeln!(out, "\n📈 Summary:")?;
    writeln!(out, "  Files processed: {}", totals.files)?;
    if options.stats_only {
        writeln!(out, "  📊 Statistics analysis complete")?;
        return Ok(if failed {
            ExitStatus::Failure
        } else {
            ExitStatus::Success
        });
    }
    if totals.warnings + totals.errors == 0 {
        writeln!(out, "  ✅ No style inconsistencies found!")?;
    } else {
        if totals.warnings > 0 {
            writeln!(out, "  ⚠️  Total warnings: {}", totals.warnings)?;
        }
        if totals.errors > 0 {
            writeln!(out, "  ❌ Total errors: {}", totals.errors)?;
        }
        writeln!(out, "  📁 Files with issues: {}", totals.files_with_issues)?;
        writeln!(out, "  📁 Clean files: {}", totals.clean())?;
        if options.ignore_warnings && totals.errors == 0 {
            writeln!(out, "  ✅ No errors found (ignoring warnings)")?;
        }
    }
    let findings_fail = totals.errors > 0 || (totals.warnings > 0 && !options.ignore_warnings);
    Ok(if failed || findings_fail {
        ExitStatus::Failure
    } else {
        ExitStatus::Success
    })
}

fn create_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => std::fs::create_dir_all(parent),
        _ => Ok(()),
    }
}

/// A report path as the user wrote it, or the batch path next to it.
fn shown(cwd: &Path, actual: &Path, given: &str) -> String {
    if Path::new(given).is_absolute() {
        return actual.display().to_string();
    }
    actual
        .strip_prefix(cwd)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| given.to_owned())
}

fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn prepare(cwd: &Path, display: String) -> Prepared {
    let absolute = absolute(&cwd.join(&display));
    let mut timing = FileTiming::default();
    let read_start = Instant::now();
    let text = std::fs::read_to_string(&absolute);
    timing.read = read_start.elapsed();
    let state = match text {
        Err(error) => FileState::Invalid(error.to_string()),
        Ok(text) => {
            let flatten_start = Instant::now();
            match serde_json::from_str::<Value>(&text) {
                Err(error) => FileState::Invalid(error.to_string()),
                Ok(value) if !python_truthy(&value) => FileState::Empty,
                Ok(value) => {
                    let flat = model::flatten(&value);
                    timing.flatten = flatten_start.elapsed();
                    let model_start = Instant::now();
                    let model = ViewModel::build(&flat);
                    timing.model = model_start.elapsed();
                    FileState::Ready(Box::new((flat, model)))
                }
            }
        }
    };
    Prepared {
        display,
        absolute,
        state,
        timing,
    }
}

/// The ign-lint line of a finding.
fn message_line(diagnostic: &Diagnostic) -> String {
    match &diagnostic.node_path {
        Some(path) => format!("{path}: {}", diagnostic.message),
        None => diagnostic.message.clone(),
    }
}

/// Groups findings per ign-lint rule, in ign-lint's rule order.
fn build_report(file: &Prepared, diagnostics: &[&Diagnostic], rules: &Rules) -> FileReport {
    let order = |name: &str| {
        compat::IGN_LINT_RULES
            .iter()
            .position(|r| r.name == name)
            .unwrap_or(usize::MAX)
    };
    let mut errors: BTreeMap<(usize, String), Vec<String>> = BTreeMap::new();
    let mut warnings: BTreeMap<(usize, String), Vec<String>> = BTreeMap::new();
    for diagnostic in diagnostics {
        let name = rules
            .names
            .get(&diagnostic.rule)
            .cloned()
            .unwrap_or_else(|| diagnostic.rule.code().to_owned());
        let target = if diagnostic.severity == Severity::Error {
            &mut errors
        } else {
            &mut warnings
        };
        target
            .entry((order(&name), name))
            .or_default()
            .push(message_line(diagnostic));
    }
    let blocks = |map: BTreeMap<(usize, String), Vec<String>>| -> Vec<RuleBlock> {
        map.into_iter().map(|((_, name), m)| (name, m)).collect()
    };
    FileReport {
        path: file.display.clone(),
        errors: blocks(errors),
        warnings: blocks(warnings),
        timing: file.timing.clone(),
    }
}

fn rule_name(rules: &Rules, rule: RuleCode) -> String {
    rules
        .names
        .get(&rule)
        .cloned()
        .unwrap_or_else(|| rule.code().to_owned())
}

fn print_applied_fixes(
    out: &mut dyn Write,
    display: &str,
    summary: &engine::FixSummary,
) -> io::Result<()> {
    if summary.applied.is_empty() && summary.skipped.is_empty() {
        return Ok(());
    }
    if !summary.applied.is_empty() {
        writeln!(out, "\nApplying fixes to {display}:")?;
        for fix in &summary.applied {
            writeln!(out, "  Applied: {}", fix.message)?;
        }
        if summary.written {
            writeln!(out, "  File updated: {display}")?;
        }
    }
    for fix in &summary.skipped {
        let reason = match fix.reason.as_str() {
            "unsafe fix (use --unsafe-fixes)" => "unsafe fix (use --fix-unsafe)",
            "filtered by the selected fix rules" => "filtered by --fix-rules",
            "the rule is marked unfixable" => "allow_fix is false",
            other => other,
        };
        writeln!(out, "  Skipped: {} ({reason})", fix.message)?;
    }
    writeln!(
        out,
        "\nApplied: {} fix(es) | Skipped: {} fix(es)",
        summary.applied.len(),
        summary.skipped.len()
    )
}

fn print_proposed_fixes(
    out: &mut dyn Write,
    cwd: &Path,
    display: &str,
    diagnostics: &[&Diagnostic],
    rules: &Rules,
    fix_unsafe: bool,
) -> io::Result<()> {
    let unfixable = &rules.compat.settings.unfixable;
    let fixes: Vec<&Diagnostic> = diagnostics
        .iter()
        .copied()
        .filter(|d| {
            d.fix
                .as_ref()
                .is_some_and(|f| f.applicability != Applicability::DisplayOnly)
                && !unfixable.contains(&d.rule)
        })
        .collect();
    if fixes.is_empty() {
        return Ok(());
    }
    writeln!(
        out,
        "\n{WIDE_EQ}\nProposed Fixes for {display}:\n{WIDE_EQ}\n"
    )?;
    let (mut safe, mut unsafe_count, mut filtered) = (0, 0, 0);
    for (i, diagnostic) in fixes.iter().enumerate() {
        let fix = diagnostic.fix.as_ref().expect("filtered for fixes");
        if rules
            .fix_rules
            .as_ref()
            .is_some_and(|selected| !selected.contains(&diagnostic.rule))
        {
            filtered += 1;
            continue;
        }
        let label = if fix.applicability == Applicability::Safe {
            safe += 1;
            "SAFE"
        } else {
            unsafe_count += 1;
            "UNSAFE"
        };
        writeln!(
            out,
            "  Fix {} ({}) [{label}]:",
            i + 1,
            rule_name(rules, diagnostic.rule)
        )?;
        writeln!(out, "    {}", fix.message)?;
        writeln!(out, "    Operations:")?;
        for edit in &fix.edits {
            let shown_path = edit
                .path
                .strip_prefix(cwd)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| edit.path.display().to_string());
            let (old, location) = if edit.path == diagnostic.file.path() {
                let text = diagnostic.file.text();
                let old = text
                    .get(edit.edit.range.to_std())
                    .unwrap_or_default()
                    .to_owned();
                let position = diagnostic
                    .file
                    .line_index()
                    .line_column(edit.edit.range.start(), text);
                (old, format!(":{}:{}", position.line, position.column))
            } else {
                (String::new(), String::new())
            };
            writeln!(out, "      REPLACE in {shown_path}{location}:")?;
            writeln!(out, "              '{old}' -> '{}'", edit.edit.content)?;
        }
        writeln!(out)?;
    }
    let total = safe + unsafe_count;
    writeln!(
        out,
        "Summary: {total} fixes ({safe} safe, {unsafe_count} unsafe)"
    )?;
    if filtered > 0 {
        writeln!(out, "  Filtered out: {filtered} (not in --fix-rules)")?;
    }
    if !fix_unsafe && unsafe_count > 0 {
        writeln!(out, "  --fix would apply: {safe} safe fix(es)")?;
        writeln!(out, "  --fix --fix-unsafe would apply: {total} fix(es)")?;
    } else {
        writeln!(out, "  Would apply: {total} fix(es)")?;
    }
    Ok(())
}

fn print_debug_nodes(
    out: &mut dyn Write,
    model: &ViewModel,
    types: &BTreeSet<NodeType>,
) -> io::Result<()> {
    if types.is_empty() {
        writeln!(out, "\n🔧 Debug info for all nodes:")?;
    } else {
        let names: Vec<&str> = types.iter().map(|t| t.as_str()).collect();
        writeln!(out, "\n🔧 Debug info for node types: {}", names.join(", "))?;
    }
    let nodes: Vec<_> = model
        .nodes()
        .filter(|n| types.is_empty() || types.contains(&n.node_type))
        .collect();
    for (i, node) in nodes.iter().take(10).enumerate() {
        writeln!(out, "  {}. {} ({})", i + 1, node.path, node.node_type)?;
    }
    if nodes.len() > 10 {
        writeln!(out, "     ... and {} more nodes", nodes.len() - 10)?;
    }
    Ok(())
}

/// `(rule name, targets, processed node count)` for every loaded rule.
fn coverage(model: &ViewModel, rules: &Rules) -> Vec<(String, BTreeSet<NodeType>, usize)> {
    rules
        .config
        .loaded_rules()
        .map(|entry| {
            let name = entry.ign_lint.map_or(entry.name.as_str(), |i| i.name);
            let targets = model::rule_targets(name, &entry.kwargs);
            let count = model::nodes_for(model, &targets).len();
            (name.to_owned(), targets, count)
        })
        .collect()
}

fn print_statistics(
    out: &mut dyn Write,
    display: &str,
    model: &ViewModel,
    rules: Option<&Rules>,
) -> io::Result<()> {
    writeln!(out, "\n📊 Model statistics for {display}:")?;
    writeln!(out, "  Total nodes: {}", model.node_count())?;
    writeln!(out, "  Node types found:")?;
    for (node_type, count) in model.type_counts() {
        writeln!(out, "    {node_type}: {count}")?;
    }
    writeln!(out, "  Components by type:")?;
    for (component_type, count) in model.components_by_type() {
        writeln!(out, "    {component_type}: {count}")?;
    }
    if let Some(rules) = rules {
        writeln!(out, "  Rule coverage:")?;
        for (name, targets, count) in coverage(model, rules) {
            writeln!(
                out,
                "    {name}: {count} nodes ({})",
                model::targets_label(&targets)
            )?;
        }
    }
    Ok(())
}

fn print_rule_analysis(out: &mut dyn Write, model: &ViewModel, rules: &Rules) -> io::Result<()> {
    writeln!(out, "\n🔍 Rule Impact Analysis:")?;
    for (name, targets, _) in coverage(model, rules) {
        let nodes = model::nodes_for(model, &targets);
        writeln!(out, "  📋 {name}:")?;
        writeln!(out, "    Targets: {}", model::targets_label(&targets))?;
        writeln!(out, "    Will process: {} nodes", nodes.len())?;
        if !nodes.is_empty() {
            writeln!(out, "    Sample nodes:")?;
            for node in nodes.iter().take(3) {
                writeln!(out, "      • {}: {}", node.path, node.summary())?;
            }
        }
        writeln!(out)?;
    }
    Ok(())
}

/// The relative directory of a file inside `--debug-output`, without `..`
/// or root components so every file gets its own place.
fn mirrored_dir(display: &str) -> PathBuf {
    Path::new(display)
        .parent()
        .map(|parent| {
            parent
                .components()
                .filter_map(|c| match c {
                    Component::Normal(part) => Some(part.to_os_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_debug_output(
    out: &mut dyn Write,
    cwd: &Path,
    dir: &str,
    display: &str,
    flat: &Flat,
    model: &ViewModel,
    rules: Option<&Rules>,
) -> io::Result<()> {
    let relative_dir = Path::new(dir).join(mirrored_dir(display));
    let target = cwd.join(&relative_dir);
    std::fs::create_dir_all(&target)?;
    let stem = Path::new(display)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "view".to_owned());
    let coverage_json: serde_json::Map<String, Value> = rules
        .map(|rules| {
            coverage(model, rules)
                .into_iter()
                .map(|(name, targets, count)| {
                    let types: Vec<&str> = targets.iter().map(|t| t.as_str()).collect();
                    (
                        name,
                        json!({"node_count": count, "target_types": if types.is_empty() { json!("all") } else { json!(types) }}),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let stats = json!({
        "total_nodes": model.node_count(),
        "node_type_counts": model.type_counts().into_iter().map(|(t, n)| (t.as_str().to_owned(), json!(n))).collect::<serde_json::Map<_, _>>(),
        "components_by_type": model.components_by_type().into_iter().map(|(t, n)| (t, json!(n))).collect::<serde_json::Map<_, _>>(),
        "rule_coverage": coverage_json,
        "model_keys": model.collections().iter().map(|(name, _)| *name).collect::<Vec<_>>(),
    });
    let files = [
        ("flattened", flat.to_sorted_json()),
        ("model", model.to_json()),
        ("stats", stats),
    ];
    writeln!(out, "✅ Debug files saved:")?;
    for (label, value) in files {
        let name = format!("{stem}_{label}.json");
        let text =
            serde_json::to_string_pretty(&model::sort_keys(&value)).map_err(io::Error::other)?;
        std::fs::write(target.join(&name), text)?;
        let label = match label {
            "flattened" => "Flattened JSON",
            "model" => "Model",
            _ => "Statistics",
        };
        writeln!(out, "   - {label}: {}", relative_dir.join(name).display())?;
    }
    Ok(())
}
