//! ign-lint's text layouts: per-file results on stdout, the
//! `--results-output` report, the `--timing-output` report, and the naming
//! scheme for reports written by parallel pre-commit batches (sections 9.1
//! to 9.3).

use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Findings of one rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleBlock {
    /// The ign-lint rule name.
    pub rule: String,
    pub messages: Vec<String>,
    /// pylint messages (`PylintScriptRule`), shown grouped by category:
    /// the category letter and the message.
    pub pylint: Vec<(char, String)>,
}

impl RuleBlock {
    pub fn len(&self) -> usize {
        self.messages.len() + self.pylint.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// ign-lint's grouped pylint output: per category F, E, W, C, R, an
    /// empty line, `    Pylint - {name} ({letter}):` and one
    /// `      • {message}` per message. Other categories are counted but
    /// not shown, like in ign-lint.
    pub fn pylint_text(&self) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        for (letter, name) in [
            ('F', "Fatal"),
            ('E', "Error"),
            ('W', "Warning"),
            ('C', "Convention"),
            ('R', "Refactor"),
        ] {
            let messages: Vec<&String> = self
                .pylint
                .iter()
                .filter(|(c, _)| *c == letter)
                .map(|(_, m)| m)
                .collect();
            if messages.is_empty() {
                continue;
            }
            lines.push(format!("\n    Pylint - {name} ({letter}):"));
            lines.extend(messages.into_iter().map(|m| format!("      • {m}")));
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

/// Phase durations of one file.
#[derive(Clone, Debug, Default)]
pub struct FileTiming {
    pub read: Duration,
    pub flatten: Duration,
    pub model: Duration,
    pub rules: Duration,
}

impl FileTiming {
    pub fn total(&self) -> Duration {
        self.read + self.flatten + self.model + self.rules
    }
}

/// The results of one processed file.
#[derive(Clone, Debug, Default)]
pub struct FileReport {
    /// As displayed (relative paths stay relative).
    pub path: String,
    pub errors: Vec<RuleBlock>,
    pub warnings: Vec<RuleBlock>,
    pub timing: FileTiming,
}

impl FileReport {
    pub fn error_count(&self) -> usize {
        self.errors.iter().map(RuleBlock::len).sum()
    }

    pub fn warning_count(&self) -> usize {
        self.warnings.iter().map(RuleBlock::len).sum()
    }

    pub fn has_issues(&self) -> bool {
        self.error_count() + self.warning_count() > 0
    }
}

/// Prints a file's findings on stdout (section 9.1).
pub fn print_file_results(out: &mut dyn Write, report: &FileReport) -> io::Result<()> {
    if !report.has_issues() {
        return writeln!(out, "✅ No issues found");
    }
    let blocks = |out: &mut dyn Write, blocks: &[RuleBlock]| -> io::Result<()> {
        for block in blocks {
            writeln!(out, "\n  {}:", block.rule)?;
            for message in block.messages.iter().filter(|m| !m.is_empty()) {
                writeln!(out, "    • {message}")?;
            }
            if let Some(text) = block.pylint_text() {
                writeln!(out, "{text}")?;
            }
            writeln!(out)?;
        }
        Ok(())
    };
    if report.error_count() > 0 {
        writeln!(out, "\n❌ Found {} errors:", report.error_count())?;
        blocks(out, &report.errors)?;
    }
    if report.warning_count() > 0 {
        writeln!(out, "\n⚠️  Found {} warnings:", report.warning_count())?;
        blocks(out, &report.warnings)?;
    }
    Ok(())
}

/// Totals of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: usize,
    pub warnings: usize,
    pub errors: usize,
    pub files_with_issues: usize,
}

impl Totals {
    pub fn clean(&self) -> usize {
        self.files.saturating_sub(self.files_with_issues)
    }
}

const WIDE_EQ: &str = "========================================================================================================================";
const WIDE_DASH: &str = "------------------------------------------------------------------------------------------------------------------------";
const EQ: &str = "================================================================================";
const DASH: &str =
    "--------------------------------------------------------------------------------";

/// The `--results-output` report (section 9.2).
pub fn results_text(totals: &Totals, whitelisted: &[String], files: &[FileReport]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{WIDE_EQ}\nIGNITION-LINT RESULTS\n{WIDE_EQ}\n");
    let _ = writeln!(s, "SUMMARY\n{WIDE_DASH}");
    let _ = writeln!(s, "Files processed: {}", totals.files);
    let _ = writeln!(s, "Total warnings:  {}", totals.warnings);
    let _ = writeln!(s, "Total errors:    {}", totals.errors);
    let _ = writeln!(s, "Files with issues: {}", totals.files_with_issues);
    let _ = writeln!(s, "Clean files:     {}", totals.clean());
    if !whitelisted.is_empty() {
        let _ = writeln!(s, "Files whitelisted: {}", whitelisted.len());
        let _ = writeln!(s, "\nWHITELISTED FILES (SKIPPED)\n{WIDE_DASH}");
        let _ = writeln!(
            s,
            "The following {} file(s) were skipped due to whitelist:\n",
            whitelisted.len()
        );
        for path in whitelisted {
            let _ = writeln!(s, "  🔒 {path}");
        }
    }
    let _ = writeln!(s, "\nPER-FILE RESULTS\n{WIDE_EQ}\n");
    for file in files {
        let icon = if file.error_count() > 0 {
            "❌"
        } else if file.warning_count() > 0 {
            "⚠️"
        } else {
            "✅"
        };
        let _ = writeln!(s, "{WIDE_DASH}\n{icon} {}\n{WIDE_DASH}\n", file.path);
        let mut section = |title: &str, count: usize, blocks: &[RuleBlock]| {
            if count == 0 {
                return;
            }
            let _ = writeln!(s, "{title} ({count} total):\n");
            for block in blocks {
                let _ = writeln!(s, "  {}:", block.rule);
                for message in block.messages.iter().filter(|m| !m.is_empty()) {
                    let _ = writeln!(s, "    • {message}");
                }
                if let Some(text) = block.pylint_text() {
                    let _ = writeln!(s, "{text}");
                }
                let _ = writeln!(s);
            }
            let _ = writeln!(s);
        };
        section("ERRORS", file.error_count(), &file.errors);
        section("WARNINGS", file.warning_count(), &file.warnings);
        if !file.has_issues() {
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(s, "{WIDE_EQ}\nEND OF RESULTS\n{WIDE_EQ}");
    s
}

/// Parses the SUMMARY numbers of a results report.
pub fn parse_summary(text: &str) -> Option<Totals> {
    let number = |label: &str| -> Option<usize> {
        text.lines()
            .find_map(|l| l.strip_prefix(label))
            .and_then(|rest| rest.trim().parse().ok())
    };
    Some(Totals {
        files: number("Files processed:")?,
        warnings: number("Total warnings:")?,
        errors: number("Total errors:")?,
        files_with_issues: number("Files with issues:")?,
    })
}

/// The aggregated summary over parallel batch reports.
pub fn aggregated_text(batches: &[(String, Totals)], now: SystemTime) -> String {
    let mut total = Totals::default();
    for (_, t) in batches {
        total.files += t.files;
        total.warnings += t.warnings;
        total.errors += t.errors;
        total.files_with_issues += t.files_with_issues;
    }
    let mut s = String::new();
    let _ = writeln!(s, "{EQ}\nAGGREGATED IGNITION-LINT RESULTS SUMMARY\n{EQ}");
    let _ = writeln!(s, "Generated: {}", timestamp(now));
    let _ = writeln!(s, "Total batches: {}\n", batches.len());
    let _ = writeln!(s, "Files processed: {}", total.files);
    let _ = writeln!(s, "Total warnings:  {}", total.warnings);
    let _ = writeln!(s, "Total errors:    {}", total.errors);
    let _ = writeln!(s, "Files with issues: {}", total.files_with_issues);
    let _ = writeln!(s, "Clean files:     {}\n", total.clean());
    let _ = writeln!(s, "BREAKDOWN BY BATCH\n{DASH}");
    for (name, t) in batches {
        let _ = writeln!(
            s,
            "{name:<45} Files: {:>3}  Warnings: {:>4}  Errors: {:>4}  Issues: {:>3}  Clean: {:>3}",
            t.files,
            t.warnings,
            t.errors,
            t.files_with_issues,
            t.clean()
        );
    }
    let _ = writeln!(s, "{EQ}");
    s
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The `--timing-output` report (section 9.3). iglint runs all rules in one
/// pass per file, so rule time is reported as one row, `iglint (all rules)`.
pub fn timing_text(files: &[FileReport], wall: Duration, now: SystemTime) -> String {
    const ALL_RULES: &str = "iglint (all rules)";
    let n = files.len();
    let avg = |d: Duration| if n == 0 { 0.0 } else { ms(d) / n as f64 };
    let sum = |f: fn(&FileTiming) -> Duration| files.iter().map(|r| f(&r.timing)).sum::<Duration>();
    let read = sum(|t| t.read);
    let flatten = sum(|t| t.flatten);
    let model = sum(|t| t.model);
    let rules = sum(|t| t.rules);
    let mut s = String::new();
    let _ = writeln!(s, "{EQ}\nIGNITION-LINT PERFORMANCE TIMING REPORT\n{EQ}");
    let _ = writeln!(s, "Report Generated: {}\n", timestamp(now));
    let _ = writeln!(s, "OVERALL SUMMARY\n{DASH}");
    let _ = writeln!(
        s,
        "Total Duration: {:.2} ms ({:.2}s)",
        ms(wall),
        wall.as_secs_f64()
    );
    let _ = writeln!(s, "Files Processed: {n}");
    let _ = writeln!(s, "Average Per File: {:.2} ms\n", avg(wall));
    let _ = writeln!(s, "PHASE BREAKDOWN (Totals)\n{DASH}");
    let _ = writeln!(s, "File Reading:    {:>10.2} ms", ms(read));
    let _ = writeln!(s, "JSON Flattening: {:>10.2} ms", ms(flatten));
    let _ = writeln!(s, "Model Building:  {:>10.2} ms", ms(model));
    let _ = writeln!(s, "Rule Execution:  {:>10.2} ms\n", ms(rules));
    let _ = writeln!(s, "PHASE BREAKDOWN (Averages per file)\n{DASH}");
    let _ = writeln!(s, "File Reading:    {:>10.2} ms", avg(read));
    let _ = writeln!(s, "JSON Flattening: {:>10.2} ms", avg(flatten));
    let _ = writeln!(s, "Model Building:  {:>10.2} ms", avg(model));
    let _ = writeln!(s, "Rule Execution:  {:>10.2} ms\n", avg(rules));
    let _ = writeln!(s, "RULE EXECUTION SUMMARY\n{DASH}");
    let _ = writeln!(
        s,
        "{:<40} {:>12} {:>12} {:>12}",
        "Rule Name", "Total (ms)", "Avg (ms)", "Executions"
    );
    let _ = writeln!(s, "{DASH}");
    if n > 0 {
        let _ = writeln!(
            s,
            "{ALL_RULES:<40} {:>12.2} {:>12.2} {:>12}",
            ms(rules),
            avg(rules),
            n
        );
    }
    let _ = writeln!(s, "\nPER-FILE TIMING DETAILS\n{EQ}");
    for file in files {
        let t = &file.timing;
        let _ = writeln!(s, "\nFile: {}", file.path);
        let _ = writeln!(s, "Total: {:.2} ms", ms(t.total()));
        let _ = writeln!(s, "  - File Read:      {:>10.2} ms", ms(t.read));
        let _ = writeln!(s, "  - JSON Flatten:   {:>10.2} ms", ms(t.flatten));
        let _ = writeln!(s, "  - Model Build:    {:>10.2} ms", ms(t.model));
        let _ = writeln!(s, "  - Rule Execution: {:>10.2} ms", ms(t.rules));
        let _ = writeln!(s, "  Rule breakdown:");
        let _ = writeln!(s, "    - {ALL_RULES}: {:.2} ms", ms(t.rules));
    }
    let _ = writeln!(s, "\n{EQ}\nEND OF REPORT\n{EQ}");
    s
}

/// `YYYY-MM-DD HH:MM:SS` in the local time zone, like ign-lint's
/// `datetime.now().strftime("%Y-%m-%d %H:%M:%S")`.
pub fn timestamp(now: SystemTime) -> String {
    timestamp_in(now, &jiff::tz::TimeZone::system())
}

fn timestamp_in(now: SystemTime, zone: &jiff::tz::TimeZone) -> String {
    let instant = jiff::Timestamp::try_from(now).unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    instant
        .to_zoned(zone.clone())
        .strftime("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Reports younger than this belong to a parallel batch of the same run.
const BATCH_WINDOW: Duration = Duration::from_secs(5);

fn age(path: &Path, now: SystemTime) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(now.duration_since(modified).unwrap_or_default())
}

fn split_name(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let suffix = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (stem, suffix)
}

/// Files in the report's directory whose name starts with `prefix` and ends
/// with `suffix`.
fn siblings(path: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect();
    out.sort();
    out
}

/// Start-up clean-up: removes a stale report, stale batch reports of other
/// processes and a stale aggregated summary (all at least 5 s old).
pub fn clean_up(path: &Path, now: SystemTime) {
    let old = |p: &Path| age(p, now).is_some_and(|a| a >= BATCH_WINDOW);
    if old(path) {
        let _ = std::fs::remove_file(path);
    }
    let (stem, suffix) = split_name(path);
    let own = format!("_pid{}_", std::process::id());
    for batch in siblings(path, &stem, &suffix) {
        let name = batch
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.contains("batch") && !name.contains(&own) && old(&batch) {
            let _ = std::fs::remove_file(&batch);
        }
    }
    let aggregated = aggregated_path(path);
    if old(&aggregated) {
        let _ = std::fs::remove_file(aggregated);
    }
}

fn aggregated_path(path: &Path) -> PathBuf {
    let (stem, _) = split_name(path);
    path.with_file_name(format!("{stem}_AGGREGATED_SUMMARY.txt"))
}

/// Where to write a report: `path`, or `{stem}_pid{PID}_batch{n}{suffix}`
/// when `path` was written less than 5 s ago by a parallel batch.
pub fn report_path(path: &Path, now: SystemTime) -> (PathBuf, bool) {
    if !age(path, now).is_some_and(|a| a < BATCH_WINDOW) {
        return (path.to_path_buf(), false);
    }
    let (stem, suffix) = split_name(path);
    let pid = std::process::id();
    let mut n = 1;
    loop {
        let candidate = path.with_file_name(format!("{stem}_pid{pid}_batch{n}{suffix}"));
        if !candidate.exists() {
            return (candidate, true);
        }
        n += 1;
    }
}

/// Rewrites the aggregated summary from the main report and every batch
/// report next to it. Returns its path.
pub fn write_aggregated(path: &Path, now: SystemTime) -> io::Result<PathBuf> {
    let (stem, suffix) = split_name(path);
    let mut batches = Vec::new();
    let mut candidates = vec![path.to_path_buf()];
    candidates.extend(siblings(path, &format!("{stem}_pid"), &suffix));
    for candidate in candidates {
        if let Some(totals) = std::fs::read_to_string(&candidate)
            .ok()
            .and_then(|text| parse_summary(&text))
        {
            let name = candidate
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            batches.push((name, totals));
        }
    }
    let target = aggregated_path(path);
    std::fs::write(&target, aggregated_text(&batches, now))?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        use jiff::tz::{Offset, TimeZone};
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_761_000_000);
        assert_eq!(timestamp_in(t, &TimeZone::UTC), "2025-10-20 22:40:00");
        assert_eq!(
            timestamp_in(SystemTime::UNIX_EPOCH, &TimeZone::UTC),
            "1970-01-01 00:00:00"
        );
        // Local time, not UTC: Brussels is UTC+2 in October 2025 (summer
        // time) and UTC+1 in January.
        let brussels = TimeZone::fixed(Offset::constant(2));
        assert_eq!(timestamp_in(t, &brussels), "2025-10-21 00:40:00");
        if let Ok(zone) = TimeZone::get("Europe/Brussels") {
            assert_eq!(timestamp_in(t, &zone), "2025-10-21 00:40:00");
            let winter = SystemTime::UNIX_EPOCH + Duration::from_secs(1_768_000_000);
            assert_eq!(timestamp_in(winter, &zone), "2026-01-10 00:06:40");
        }
    }

    #[test]
    fn summary_round_trip() {
        let totals = Totals {
            files: 3,
            warnings: 2,
            errors: 1,
            files_with_issues: 2,
        };
        let text = results_text(&totals, &[], &[]);
        assert_eq!(parse_summary(&text), Some(totals));
    }

    #[test]
    fn batch_naming() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("results.txt");
        let now = SystemTime::now();
        assert_eq!(report_path(&path, now), (path.clone(), false));
        std::fs::write(&path, "x").unwrap();
        let (batch, is_batch) = report_path(&path, now);
        assert!(is_batch);
        assert_eq!(
            batch.file_name().unwrap().to_string_lossy(),
            format!("results_pid{}_batch1.txt", std::process::id())
        );
        // Ten seconds later the existing report is stale and gets replaced.
        let later = now + Duration::from_secs(10);
        assert_eq!(report_path(&path, later), (path.clone(), false));
        clean_up(&path, later);
        assert!(!path.exists());
    }

    /// ign-lint's layout for `PylintScriptRule`: plain messages first, then
    /// the pylint messages grouped by category in F, E, W, C, R order.
    #[test]
    fn pylint_messages_are_grouped_by_category() {
        let block = RuleBlock {
            rule: "PylintScriptRule".to_owned(),
            messages: vec!["a: native".to_owned()],
            pylint: vec![
                ('C', "a: Line 2: c1 (C0303)".to_owned()),
                ('E', "a: Line 1: e1 (E0602)".to_owned()),
                ('C', "a: Line 3: c2 (C0303)".to_owned()),
                ('I', "a: Line 3: hidden (I0011)".to_owned()),
            ],
        };
        let report = FileReport {
            path: "view.json".to_owned(),
            errors: vec![block],
            ..FileReport::default()
        };
        let mut out = Vec::new();
        print_file_results(&mut out, &report).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "\n❌ Found 5 errors:\n\n  PylintScriptRule:\n    • a: native\n\n    Pylint - Error (E):\n      • a: Line 1: e1 (E0602)\n\n    Pylint - Convention (C):\n      • a: Line 2: c1 (C0303)\n      • a: Line 3: c2 (C0303)\n\n"
        );
        let text = results_text(&Totals::default(), &[], &[report]);
        assert!(
            text.contains("ERRORS (5 total):\n\n  PylintScriptRule:\n    • a: native\n\n    Pylint - Error (E):\n      • a: Line 1: e1 (E0602)\n\n    Pylint - Convention (C):\n      • a: Line 2: c1 (C0303)\n      • a: Line 3: c2 (C0303)\n\n\n"),
            "{text}"
        );
    }
}
