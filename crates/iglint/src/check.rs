//! `iglint check` and `iglint baseline`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::ValueEnum;
use iglint_config::{CliOverrides, Configuration, LoadOptions, absolutize};
use iglint_linter::baseline::{Baseline, DEFAULT_BASELINE_FILE};
use iglint_linter::discovery::{self, SkipReason};
use iglint_linter::engine::{self, CheckOptions, FixMode};
use iglint_linter::{Diagnostic, Severity};

use crate::args::{CheckArgs, ColorChoice, Level};
use crate::output::{self, OutputFormat, Report};
use crate::{Environment, ExitStatus, Io};

macro_rules! out {
    ($io:expr, $($arg:tt)*) => {
        let _ = writeln!($io.stdout, $($arg)*);
    };
}

macro_rules! err {
    ($io:expr, $($arg:tt)*) => {
        let _ = writeln!($io.stderr, $($arg)*);
    };
}

/// Whether to colour output written to standard output.
pub fn use_color(choice: ColorChoice, environment: &Environment, io: &Io<'_>) -> bool {
    match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto => {
            if environment.var("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                false
            } else if environment
                .var("CLICOLOR_FORCE")
                .is_some_and(|v| !v.is_empty() && v != "0")
            {
                true
            } else {
                io.stdout_is_terminal
            }
        }
    }
}

fn load_configuration(
    args: &CheckArgs,
    environment: &Environment,
) -> Result<Configuration, iglint_config::ConfigError> {
    let options = LoadOptions {
        cwd: environment.cwd.clone(),
        start: args
            .stdin_filename
            .clone()
            .or_else(|| args.paths.first().cloned()),
        config_file: args.config.clone(),
        isolated: args.isolated,
        cli: CliOverrides {
            select: args.select.clone(),
            extend_select: args.extend_select.clone(),
            ignore: args.ignore.clone(),
            extend_ignore: args.extend_ignore.clone(),
            strict: args.strict.then_some(true),
            whitelist: args.whitelist.clone(),
        },
    };
    iglint_config::load(&options, environment.registry)
}

fn fix_mode(args: &CheckArgs) -> FixMode {
    if args.diff {
        FixMode::DiffOnly {
            unsafe_fixes: args.unsafe_fixes,
        }
    } else if args.fix || args.fix_only {
        if args.unsafe_fixes {
            FixMode::Unsafe
        } else {
            FixMode::Safe
        }
    } else {
        FixMode::None
    }
}

fn threshold(args: &CheckArgs) -> Severity {
    if args.ignore_warnings {
        return Severity::Error;
    }
    match args.error_on {
        Some(Level::Info) => Severity::Info,
        Some(Level::Error) => Severity::Error,
        Some(Level::Warning) | None => Severity::Warning,
    }
}

fn show_settings(
    configuration: &Configuration,
    paths: &[PathBuf],
    environment: &Environment,
    out: &mut dyn Write,
) -> io::Result<()> {
    let settings = &configuration.settings;
    let shown = |p: &Path| output::display_path(p, &environment.cwd);
    match configuration.source.path() {
        Some(path) => writeln!(out, "Configuration: {}", shown(path))?,
        None => writeln!(out, "Configuration: defaults")?,
    }
    writeln!(out, "Root: {}", settings.root.display())?;
    let file = paths
        .first()
        .cloned()
        .unwrap_or_else(|| settings.root.clone());
    writeln!(out, "Settings for: {}", shown(&file))?;
    let resolved = settings.for_file(&file);
    writeln!(out, "  strict = {}", resolved.strict)?;
    writeln!(out, "  respect-gitignore = {}", settings.respect_gitignore)?;
    writeln!(out, "  exclude = {:?}", settings.exclude)?;
    if !settings.whitelist.is_empty() {
        writeln!(out, "  whitelisted files = {}", settings.whitelist.len())?;
    }
    if let Some(baseline) = &configuration.baseline {
        writeln!(out, "  baseline = {}", shown(baseline))?;
    }
    if !settings.unfixable.is_empty() {
        let codes: Vec<_> = settings.unfixable.iter().map(|r| r.code()).collect();
        writeln!(out, "  unfixable = {codes:?}")?;
    }
    writeln!(out, "  overrides = {}", settings.overrides.len())?;
    writeln!(out, "  enabled rules ({}):", resolved.rules.len())?;
    for (rule, rule_settings) in &resolved.rules {
        writeln!(
            out,
            "    {} {} [{}]",
            rule.code(),
            rule.name(),
            rule_settings.severity
        )?;
        for (name, value) in &rule_settings.options {
            writeln!(
                out,
                "      {name} = {}",
                iglint_config::migrate::toml_value(value)
            )?;
        }
        if let Some(globs) = settings.rule_exclude.get(rule) {
            writeln!(out, "      exclude = {globs:?}")?;
        }
    }
    Ok(())
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&diff_name("a/", path), &diff_name("b/", path))
        .to_string()
}

/// A file name for a `---` / `+++` line that `patch` and `git apply` both
/// read back: quoted like git when it holds `"`, `\` or control
/// characters, followed by a tab (as `diff -u` writes before the
/// timestamp) when it holds spaces, which otherwise end the name for GNU
/// patch.
fn diff_name(prefix: &str, path: &str) -> String {
    let name = format!("{prefix}{path}");
    if name
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_ascii_control())
    {
        let mut quoted = String::from("\"");
        for c in name.chars() {
            match c {
                '"' => quoted.push_str("\\\""),
                '\\' => quoted.push_str("\\\\"),
                '\t' => quoted.push_str("\\t"),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                c if c.is_ascii_control() => quoted.push_str(&format!("\\{:03o}", c as u32)),
                c => quoted.push(c),
            }
        }
        quoted.push('"');
        quoted
    } else if name.contains(' ') {
        format!("{name}\t")
    } else {
        name
    }
}

/// Runs `iglint check` (or `iglint baseline` when `write_baseline`).
pub fn run(
    args: &CheckArgs,
    environment: &Environment,
    io: &mut Io<'_>,
    write_baseline: bool,
) -> ExitStatus {
    let start = Instant::now();
    let update_baseline = write_baseline || args.update_baseline;
    let configuration = match load_configuration(args, environment) {
        Ok(configuration) => configuration,
        Err(error) => {
            err!(io, "error: {error}");
            return ExitStatus::Error;
        }
    };
    let config_time = start.elapsed();
    if !args.quiet && !args.silent {
        for notice in &configuration.notices {
            err!(io, "warning: {notice}");
        }
    }
    let settings = &configuration.settings;
    let cwd = &environment.cwd;

    let stdin = match &args.stdin_filename {
        Some(name) => {
            let mut text = String::new();
            if let Err(error) = io.stdin.read_to_string(&mut text) {
                err!(io, "error: cannot read standard input: {error}");
                return ExitStatus::Error;
            }
            Some((absolutize(cwd, name), text))
        }
        None => None,
    };
    let paths: Vec<PathBuf> = if args.paths.is_empty() {
        vec![cwd.clone()]
    } else {
        args.paths.iter().map(|p| absolutize(cwd, p)).collect()
    };

    if args.show_settings {
        let shown_paths: Vec<PathBuf> = match &stdin {
            Some((path, _)) => vec![path.clone()],
            None if args.paths.is_empty() => Vec::new(),
            None => paths.clone(),
        };
        let _ = show_settings(&configuration, &shown_paths, environment, io.stdout);
        return ExitStatus::Success;
    }
    if args.show_files {
        let found = discovery::discover(settings, &paths);
        for (path, _) in &found.files {
            out!(io, "{}", output::display_path(path, cwd));
        }
        for (path, message) in &found.errors {
            err!(io, "error: {}: {message}", output::display_path(path, cwd));
        }
        return if found.errors.is_empty() {
            ExitStatus::Success
        } else {
            ExitStatus::Error
        };
    }

    let output_format = match args.output_format {
        Some(format) => format,
        None => match configuration.output_format.as_deref() {
            Some(name) => OutputFormat::from_str(name, true).unwrap_or(OutputFormat::Text),
            None => OutputFormat::Text,
        },
    };

    let mode = fix_mode(args);
    let cache_dir = (!args.no_cache && stdin.is_none()).then(|| {
        args.cache_dir
            .as_ref()
            .map(|d| absolutize(cwd, d))
            .or_else(|| configuration.cache_dir.clone())
            .unwrap_or_else(|| settings.root.join(".iglint_cache"))
    });
    let mut options = CheckOptions {
        fix_mode: mode,
        threads: args.threads,
        cache_dir,
        stdin: stdin.clone(),
        // The process exits after the report.
        release_memory: false,
        ..CheckOptions::default()
    };
    if let Some(analyzer) = &environment.analyzer {
        options.analyzer = Some(analyzer.clone());
    }
    let result = engine::check(settings, &paths, &options);

    // Baseline.
    let explicit_baseline = args.baseline.as_ref().map(|b| absolutize(cwd, b));
    let baseline_path = explicit_baseline
        .clone()
        .or_else(|| configuration.baseline.clone())
        .unwrap_or_else(|| settings.root.join(DEFAULT_BASELINE_FILE));
    let shown_baseline = output::display_path(&baseline_path, cwd);
    // Baseline entries this run can judge: files it linted, and files that
    // no longer exist. A run over some files (pre-commit, an editor) leaves
    // the entries of the other files alone.
    let linted: BTreeSet<String> = result
        .files
        .iter()
        .map(|f| settings.relative_path(&f.path))
        .collect();
    let in_scope = |path: &str| linted.contains(path) || !settings.root.join(path).exists();
    if update_baseline {
        let mut baseline = Baseline::from_diagnostics(settings, &result.diagnostics);
        if baseline_path.is_file()
            && let Ok(previous) = Baseline::load(&baseline_path)
        {
            baseline = baseline.with_entries_outside(&previous, in_scope);
        }
        if let Err(error) = baseline.save(&baseline_path) {
            err!(io, "error: cannot write baseline {shown_baseline}: {error}");
            return ExitStatus::Error;
        }
        if !args.silent {
            err!(
                io,
                "Wrote {} finding(s) to the baseline {shown_baseline}.",
                baseline.len()
            );
        }
        return ExitStatus::Success;
    }
    let baseline_required = explicit_baseline.is_some() || configuration.baseline.is_some();
    let mut diagnostics: Vec<Diagnostic> = result.diagnostics.clone();
    let mut suppressed = 0;
    if baseline_path.is_file() || baseline_required {
        match Baseline::load(&baseline_path) {
            Ok(baseline) => {
                let mut matched = baseline.filter(settings, diagnostics);
                matched.stale.retain(|e| in_scope(&e.path));
                diagnostics = matched.new;
                suppressed = matched.suppressed;
                let stale: usize = matched.stale.iter().map(|e| e.count as usize).sum();
                if stale > 0 && stdin.is_none() {
                    if args.prune_baseline {
                        let pruned = baseline.pruned(&matched.stale);
                        if let Err(error) = pruned.save(&baseline_path) {
                            err!(io, "error: cannot write baseline {shown_baseline}: {error}");
                            return ExitStatus::Error;
                        }
                        if !args.silent {
                            err!(
                                io,
                                "Removed {stale} fixed finding(s) from the baseline {shown_baseline}."
                            );
                        }
                    } else if !args.quiet && !args.silent {
                        err!(
                            io,
                            "note: {stale} baseline entr{} in {shown_baseline} no longer match a finding; run with --prune-baseline to remove {}.",
                            if stale == 1 { "y" } else { "ies" },
                            if stale == 1 { "it" } else { "them" }
                        );
                    }
                }
            }
            Err(error) => {
                err!(io, "error: cannot read baseline {shown_baseline}: {error}");
                return ExitStatus::Error;
            }
        }
    }

    let color = args.output_file.is_none() && use_color(args.color, environment, io);
    let report = Report {
        diagnostics: &diagnostics,
        cwd,
        settings,
        color,
        fix_mode: mode,
        unsafe_fixes: args.unsafe_fixes,
        files_checked: result.files_checked,
        errors: result.errors.len(),
    };
    let fixes_applied = result.fixes_applied();

    let mut file_output: Option<Vec<u8>> = args.output_file.as_ref().map(|_| Vec::new());
    let silent = args.silent;
    {
        let mut buffer = Vec::new();
        let target: &mut dyn Write = match file_output.as_mut() {
            Some(file) => file,
            None => &mut buffer,
        };
        let written = render_all(
            args,
            &report,
            output_format,
            &result,
            fixes_applied,
            cwd,
            target,
        );
        if let Err(error) = written {
            err!(io, "error: cannot write the report: {error}");
            return ExitStatus::Error;
        }
        if stdin.is_some() && mode.writes() {
            // The fixed text goes to standard output; findings to stderr.
            if let Some(text) = &result.stdin_output {
                let _ = io.stdout.write_all(text.as_bytes());
            }
            if !silent {
                let _ = io.stderr.write_all(&buffer);
            }
        } else if !silent {
            let _ = io.stdout.write_all(&buffer);
        }
    }
    if let (Some(path), Some(bytes)) = (&args.output_file, file_output) {
        let path = absolutize(cwd, path);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, bytes) {
            err!(
                io,
                "error: cannot write {}: {error}",
                output::display_path(&path, cwd)
            );
            return ExitStatus::Error;
        }
    }

    if !args.quiet && !args.silent {
        for notice in &result.notices {
            err!(io, "warning: {notice}");
        }
        // Files that were found but not linted, other than those the
        // configuration excludes.
        for (path, reason) in &result.skipped {
            if matches!(reason, SkipReason::NotModelled(_)) {
                err!(
                    io,
                    "note: {}: not linted: {}",
                    output::display_path(path, cwd),
                    reason.describe()
                );
            }
        }
        for summary in &result.fixed {
            for fix in summary.skipped.iter().filter(|f| f.harmful) {
                err!(
                    io,
                    "warning: {}: fix not applied: {} ({}): {}",
                    output::display_path(&summary.path, cwd),
                    fix.message,
                    fix.rule.code(),
                    fix.reason
                );
            }
        }
    }
    if !args.silent {
        for error in &result.errors {
            err!(
                io,
                "error: {}: {}",
                output::display_path(&error.path, cwd),
                error.message
            );
        }
        if suppressed > 0 && !args.quiet && output_format.is_human() {
            err!(
                io,
                "{suppressed} finding(s) hidden by the baseline {shown_baseline}."
            );
        }
    }
    if args.timing && !args.silent {
        let timings = &result.timings;
        err!(io, "timing: configuration {:>9.2?}", config_time);
        err!(io, "timing: discovery     {:>9.2?}", timings.discovery);
        err!(io, "timing: cache         {:>9.2?}", timings.cache);
        err!(io, "timing: analysis      {:>9.2?}", timings.analysis);
        err!(io, "timing: check total   {:>9.2?}", timings.total);
        err!(io, "timing: total         {:>9.2?}", start.elapsed());
        let cached = result.files.iter().filter(|f| f.cached).count();
        err!(
            io,
            "timing: {} file(s) checked, {cached} from the cache, {} skipped",
            result.files_checked,
            result.files_skipped
        );
    }

    if !result.errors.is_empty() {
        return ExitStatus::Error;
    }
    if args.exit_zero {
        return ExitStatus::Success;
    }
    if args.diff {
        return if result.fixed.iter().any(|f| f.changed()) {
            ExitStatus::Failure
        } else {
            ExitStatus::Success
        };
    }
    if args.exit_non_zero_on_fix && fixes_applied > 0 {
        return ExitStatus::Failure;
    }
    if args.fix_only {
        return ExitStatus::Success;
    }
    let limit = threshold(args);
    if diagnostics.iter().any(|d| d.severity >= limit) {
        ExitStatus::Failure
    } else {
        ExitStatus::Success
    }
}

fn render_all(
    args: &CheckArgs,
    report: &Report<'_>,
    format: OutputFormat,
    result: &engine::CheckResult,
    fixes_applied: usize,
    cwd: &Path,
    out: &mut dyn Write,
) -> io::Result<()> {
    if args.diff {
        let mut changed = BTreeSet::new();
        let mut count = 0;
        for summary in &result.fixed {
            if !summary.changed() {
                continue;
            }
            let shown = output::display_path(&summary.path, cwd);
            out.write_all(unified_diff(&shown, &summary.original, &summary.fixed).as_bytes())?;
            changed.insert(shown);
            count += summary.applied.len();
        }
        if !args.quiet {
            if count == 0 {
                writeln!(out, "No fixes available.")?;
            } else {
                writeln!(
                    out,
                    "\nWould fix {count} issue(s) in {} file(s).",
                    changed.len()
                )?;
            }
        }
        return Ok(());
    }
    if args.fix_only {
        if !args.quiet {
            writeln!(
                out,
                "Fixed {fixes_applied} issue(s) ({} remaining).",
                report.diagnostics.len()
            )?;
        }
        return Ok(());
    }
    if args.statistics {
        return output::render_statistics(format, report, out);
    }
    output::render(format, report, out)?;
    if format.is_human() && !args.quiet {
        output::render_summary(report, fixes_applied, out)?;
    }
    Ok(())
}
