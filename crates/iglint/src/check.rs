//! `iglint check` and `iglint baseline`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::ValueEnum;
use iglint_config::{CliOverrides, Configuration, LoadOptions, absolutize};
use iglint_linter::baseline::{Baseline, BaselineOrigin};
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
    // The directories whose projects and gateway collections form the
    // closed world of a run over these paths.
    let linted = if paths.is_empty() {
        vec![environment.cwd.clone()]
    } else {
        paths.to_vec()
    };
    let workspace = discovery::workspace_roots(settings, &linted);
    for root in &workspace.roots {
        writeln!(out, "Workspace root: {}", root.display())?;
    }
    for path in &workspace.unknown {
        writeln!(
            out,
            "Workspace root: none found for {} (the closed world is unknown)",
            shown(path)
        )?;
    }
    writeln!(
        out,
        "Baseline paths relative to: {}",
        BaselineOrigin::new(settings, &linted).root().display()
    )?;
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
    if settings.assume_no_workspace_file_reads {
        writeln!(
            out,
            "  fix.assume-scripts-do-not-read-workspace-files = true"
        )?;
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

/// A unified diff (`diff -u` format, three lines of context) from `old` to
/// `new`, empty when they are equal. Only the line changes come from
/// `similar`; hunks and their ranges are computed here, by counting the
/// lines: `similar` 2.7 can report a deletion at the wrong position in the
/// new file, so its unified diff writer (and the ranges of its grouped
/// operations) can give a header that does not match the hunk's lines
/// (reviewer round 13: `@@ -1,3 +2,2 @@` for a hunk with three old and
/// three new lines), which `git apply` and `patch` reject. A line without
/// a final newline is followed by `\ No newline at end of file`.
fn unified_diff(path: &str, old: &str, new: &str) -> String {
    use similar::ChangeTag;
    use std::fmt::Write as _;
    const CONTEXT: usize = 3;
    let diff = similar::TextDiff::from_lines(old, new);
    let lines: Vec<(ChangeTag, &str)> = diff
        .iter_all_changes()
        .map(|c| (c.tag(), c.value()))
        .collect();
    let changed: Vec<usize> = (0..lines.len())
        .filter(|i| lines[*i].0 != ChangeTag::Equal)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    // Hunks as ranges of `lines`: the changes with their context, merged
    // when the context of two changes touches.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let start = i.saturating_sub(CONTEXT);
        let end = (i + 1 + CONTEXT).min(lines.len());
        match hunks.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => hunks.push((start, end)),
        }
    }
    let mut out = format!(
        "--- {}\n+++ {}\n",
        diff_name("a/", path),
        diff_name("b/", path)
    );
    // `start,len` with a 1-based start; an empty range names the line
    // before it.
    let range = |before: usize, len: usize| {
        if len == 0 {
            format!("{before},0")
        } else {
            format!("{},{len}", before + 1)
        }
    };
    let (mut old_line, mut new_line, mut at) = (0usize, 0usize, 0usize);
    for (start, end) in hunks {
        for (tag, _) in &lines[at..start] {
            old_line += usize::from(*tag != ChangeTag::Insert);
            new_line += usize::from(*tag != ChangeTag::Delete);
        }
        let hunk = &lines[start..end];
        let old_len = hunk.iter().filter(|(t, _)| *t != ChangeTag::Insert).count();
        let new_len = hunk.iter().filter(|(t, _)| *t != ChangeTag::Delete).count();
        let _ = writeln!(
            out,
            "@@ -{} +{} @@",
            range(old_line, old_len),
            range(new_line, new_len)
        );
        for (tag, line) in hunk {
            out.push(match tag {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            });
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
        old_line += old_len;
        new_line += new_len;
        at = end;
    }
    out
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
    let mut result = engine::check(settings, &paths, &options);
    // A directory with nothing to lint: say so instead of only "All checks
    // passed!". Python files are linted as Ignition scripts only inside an
    // Ignition project (or when named on the command line, as standalone
    // scripts), so a directory of loose `.py` files has none. Not an error
    // (exit status 0), like an empty directory.
    if stdin.is_none() {
        for dir in paths.iter().filter(|p| p.is_dir()) {
            let found = result.files.iter().any(|f| f.path.starts_with(dir))
                || result.skipped.iter().any(|(p, _)| p.starts_with(dir))
                || result.errors.iter().any(|e| e.path.starts_with(dir));
            if !found {
                result.notices.push(format!(
                    "no Ignition project or lintable files found under {} (Python files outside an \
                     Ignition project are linted only when named on the command line)",
                    output::display_path(dir, cwd)
                ));
            }
        }
    }

    // Baseline. Its paths and fingerprints are relative to an origin that
    // does not change with the working directory (the workspace root of a
    // run without a configuration file).
    let origin = BaselineOrigin::new(settings, &paths);
    let explicit_baseline = args.baseline.as_ref().map(|b| absolutize(cwd, b));
    let baseline_path = explicit_baseline
        .clone()
        .or_else(|| configuration.baseline.clone())
        .unwrap_or_else(|| origin.default_file());
    let shown_baseline = output::display_path(&baseline_path, cwd);
    // Baseline entries this run can judge: files it linted, and files that
    // no longer exist. A run over some files (pre-commit, an editor) leaves
    // the entries of the other files alone.
    let linted: BTreeSet<String> = result
        .files
        .iter()
        .flat_map(|f| {
            std::iter::once(origin.relative_path(&f.path)).chain(origin.legacy_path(&f.path))
        })
        .collect();
    let in_scope = |path: &str| linted.contains(path) || origin.is_gone(path);
    if update_baseline {
        let mut baseline = Baseline::from_diagnostics(&origin, &result.diagnostics);
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
                let mut matched = baseline.filter(&origin, diagnostics);
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

#[cfg(test)]
mod tests {
    use super::unified_diff;
    use proptest::prelude::*;

    /// Applies `patch` with `tool` in a directory holding `a.py` = `old`,
    /// and returns the patched file (`None` when the tool is missing).
    fn apply(tool: &str, old: &str, patch: &str) -> Option<Result<String, String>> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.py"), old).unwrap();
        let args: &[&str] = match tool {
            "git" => &["apply", "--unidiff-zero", "-"],
            _ => &["-p1", "-s", "-t", "--binary"],
        };
        let mut child = std::process::Command::new(tool)
            .args(args)
            .current_dir(dir.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(patch.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        if !out.status.success() {
            return Some(Err(format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )));
        }
        Some(Ok(std::fs::read_to_string(dir.path().join("a.py")).unwrap()))
    }

    /// Checks every hunk header against the files exactly (the tools
    /// accept a hunk at an offset): the old side's lines are at its start
    /// in `old`, the new side's at its start in `new`.
    fn strict(old: &str, new: &str, patch: &str) -> Result<(), String> {
        let old: Vec<&str> = old.split_inclusive('\n').collect();
        let new: Vec<&str> = new.split_inclusive('\n').collect();
        let mut lines = patch.split_inclusive('\n').skip(2).peekable();
        while let Some(header) = lines.next() {
            let parse = |part: &str| -> (usize, usize) {
                let (start, len) = part.get(1..).unwrap().split_once(',').unwrap();
                (start.parse().unwrap(), len.parse().unwrap())
            };
            let mut parts = header.trim_end().split(' ');
            let (os, ol) = parse(parts.nth(1).unwrap());
            let (ns, nl) = parse(parts.next().unwrap());
            let (mut a, mut b) = (Vec::new(), Vec::new());
            while let Some(line) = lines.next_if(|l| !l.starts_with("@@")) {
                let (sign, text) = line.split_at(1);
                let text = if lines.peek().is_some_and(|l| l.starts_with('\\')) {
                    lines.next();
                    text.strip_suffix('\n').unwrap()
                } else {
                    text
                };
                if sign != "+" {
                    a.push(text);
                }
                if sign != "-" {
                    b.push(text);
                }
            }
            let at = |start: usize, len: usize| if len == 0 { start } else { start - 1 };
            if a.len() != ol || b.len() != nl {
                return Err(format!("counts differ from {header}"));
            }
            if old.get(at(os, ol)..at(os, ol) + ol) != Some(&a[..])
                || new.get(at(ns, nl)..at(ns, nl) + nl) != Some(&b[..])
            {
                return Err(format!("lines differ at {header}"));
            }
        }
        Ok(())
    }

    fn check(old: &str, new: &str) -> Result<(), TestCaseError> {
        let patch = unified_diff("a.py", old, new);
        if old == new {
            prop_assert!(patch.is_empty());
            return Ok(());
        }
        if let Err(e) = strict(old, new, &patch) {
            prop_assert!(false, "{}\n{}", e, patch);
        }
        for tool in ["git", "patch"] {
            match apply(tool, old, &patch) {
                None => {}
                Some(Ok(fixed)) => prop_assert_eq!(&fixed, new, "{}:\n{}", tool, patch),
                Some(Err(e)) => prop_assert!(false, "{} rejects:\n{}\n{}", tool, e, patch),
            }
        }
        Ok(())
    }

    /// The most common `SCR102` safe fix: the import's line is emptied
    /// before a blank line.
    #[test]
    fn blanked_line_before_a_blank_line() {
        let patch = unified_diff("a.py", "import math\n\nx = 1\n", "\n\nx = 1\n");
        assert_eq!(
            patch,
            "--- a/a.py\n+++ b/a.py\n@@ -1,3 +1,3 @@\n-import math\n \n+\n x = 1\n"
        );
        check("import math\n\nx = 1\n", "\n\nx = 1\n").unwrap();
    }

    /// A file of lines from a small alphabet, so edits produce blank-line
    /// runs, repeated lines and hunks that merge or stay apart.
    fn file() -> impl Strategy<Value = Vec<String>> {
        prop::collection::vec(
            prop::sample::select(vec![
                "",
                "",
                "x = 1",
                "y = 2",
                "import os",
                "pass",
                "  ",
                "#",
            ]),
            0..30,
        )
        .prop_map(|v| v.into_iter().map(str::to_owned).collect())
    }

    /// Lines joined with `\n` or `\r\n`, with or without a final line end.
    fn render(lines: &[String], crlf: bool, final_newline: bool) -> String {
        let end = if crlf { "\r\n" } else { "\n" };
        let mut s = lines.join(end);
        if final_newline && !lines.is_empty() {
            s.push_str(end);
        }
        s
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Any pair of files: the diff applies with `git apply` and GNU
        /// `patch -p1` and reproduces the new file exactly (blank-line
        /// edits, CRLF line ends, no final newline, edits at the start and
        /// the end, several hunks).
        #[test]
        fn diffs_apply_with_git_and_patch(
            old in file(),
            edits in prop::collection::vec((0usize..40, 0usize..3, file()), 0..5),
            crlf in any::<bool>(),
            old_final in any::<bool>(),
            new_final in any::<bool>(),
        ) {
            let mut new = old.clone();
            for (at, remove, insert) in edits {
                let at = at.min(new.len());
                let end = (at + remove).min(new.len());
                new.splice(at..end, insert.into_iter().take(3));
            }
            check(&render(&old, crlf, old_final), &render(&new, crlf, new_final))?;
        }
    }
}
