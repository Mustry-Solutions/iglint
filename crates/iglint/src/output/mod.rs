//! Output formats for diagnostics.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use iglint_linter::diagnostic::{Applicability, SourceFile};
use iglint_linter::engine::FixMode;
use iglint_linter::settings::ProjectSettings;
use iglint_linter::{Diagnostic, Severity};
use iglint_text::{LineColumn, TextSize};

mod ci;
mod json;
mod junit;
mod sarif;
mod style;
mod text;

#[cfg(test)]
mod tests;

pub use style::Style;

/// `--output-format` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Concise,
    Grouped,
    Json,
    JsonLines,
    Sarif,
    Github,
    Azure,
    Junit,
    Pylint,
    Gitlab,
}

impl OutputFormat {
    /// Formats meant for people, which get a summary line.
    pub const fn is_human(self) -> bool {
        matches!(
            self,
            OutputFormat::Text | OutputFormat::Concise | OutputFormat::Grouped
        )
    }
}

/// Everything a renderer needs.
pub struct Report<'a> {
    /// Sorted by path and position.
    pub diagnostics: &'a [Diagnostic],
    /// Paths are shown relative to this directory when below it.
    pub cwd: &'a Path,
    /// Fingerprints use paths relative to `settings.root`.
    pub settings: &'a ProjectSettings,
    pub color: bool,
    /// For "fixable with --fix" hints.
    pub fix_mode: FixMode,
    pub unsafe_fixes: bool,
    pub files_checked: usize,
}

impl Report<'_> {
    /// `path` relative to `cwd` when below it, `/`-separated.
    pub fn display_path(&self, path: &Path) -> String {
        display_path(path, self.cwd)
    }

    /// The baseline fingerprint of `diagnostic`, as 16 hex digits.
    pub fn fingerprint(&self, diagnostic: &Diagnostic) -> String {
        let relative = self.settings.relative_path(diagnostic.file.path());
        format!("{:016x}", diagnostic.fingerprint(&relative))
    }

    /// How the fix of `diagnostic` relates to the current mode.
    pub fn fix_state(&self, diagnostic: &Diagnostic) -> FixState {
        match diagnostic.fix.as_ref().map(|f| f.applicability) {
            None => FixState::None,
            Some(Applicability::Safe) => FixState::Applicable,
            Some(Applicability::Unsafe) if self.unsafe_fixes => FixState::Applicable,
            Some(Applicability::Unsafe) => FixState::HiddenUnsafe,
            Some(Applicability::DisplayOnly) => FixState::DisplayOnly,
        }
    }
}

/// Whether a diagnostic's fix would be applied with `--fix`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixState {
    None,
    /// Applied by `--fix` in the current mode.
    Applicable,
    /// Unsafe and `--unsafe-fixes` is off.
    HiddenUnsafe,
    /// A suggestion only.
    DisplayOnly,
}

/// `path` relative to `cwd` when below it, always `/`-separated.
pub fn display_path(path: &Path, cwd: &Path) -> String {
    iglint_linter::discovery::slash_path(path.strip_prefix(cwd).unwrap_or(path))
}

/// One-based line and column of `offset` in `file`.
pub fn position(file: &SourceFile, offset: TextSize) -> LineColumn {
    file.line_index().line_column(offset, file.text())
}

/// Start and end positions of a diagnostic.
pub fn span(diagnostic: &Diagnostic) -> (LineColumn, LineColumn) {
    (
        position(&diagnostic.file, diagnostic.range.start()),
        position(&diagnostic.file, diagnostic.range.end()),
    )
}

/// Writes `report.diagnostics` in `format`.
pub fn render(format: OutputFormat, report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    match format {
        OutputFormat::Text => text::full(report, out),
        OutputFormat::Concise => text::concise(report, out),
        OutputFormat::Grouped => text::grouped(report, out),
        OutputFormat::Pylint => text::pylint(report, out),
        OutputFormat::Json => json::json(report, out),
        OutputFormat::JsonLines => json::json_lines(report, out),
        OutputFormat::Gitlab => json::gitlab(report, out),
        OutputFormat::Sarif => sarif::sarif(report, out),
        OutputFormat::Github => ci::github(report, out),
        OutputFormat::Azure => ci::azure(report, out),
        OutputFormat::Junit => junit::junit(report, out),
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Joins `["a", "b", "c"]` as `a, b and c`.
fn join_and(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The closing summary of the human-readable formats: `Found 3 errors.`,
/// fix hints, or `All checks passed!`. `fixed` is the number of fixes
/// applied in this run.
pub fn render_summary(report: &Report<'_>, fixed: usize, out: &mut dyn Write) -> io::Result<()> {
    let style = Style::new(report.color);
    if fixed > 0 {
        writeln!(out, "Fixed {}.", plural(fixed, "issue", "issues"))?;
    }
    if report.diagnostics.is_empty() {
        return writeln!(out, "{}", style.green("All checks passed!"));
    }
    let count = |severity| {
        report
            .diagnostics
            .iter()
            .filter(|d| d.severity == severity)
            .count()
    };
    let mut parts = Vec::new();
    for (severity, singular, many) in [
        (Severity::Error, "error", "errors"),
        (Severity::Warning, "warning", "warnings"),
        (Severity::Info, "info", "info"),
    ] {
        let n = count(severity);
        if n > 0 {
            parts.push(plural(n, singular, many));
        }
    }
    writeln!(out, "Found {}.", join_and(&parts))?;
    if report.fix_mode != FixMode::None {
        return Ok(());
    }
    let applicable = report
        .diagnostics
        .iter()
        .filter(|d| report.fix_state(d) == FixState::Applicable)
        .count();
    let hidden = report
        .diagnostics
        .iter()
        .filter(|d| report.fix_state(d) == FixState::HiddenUnsafe)
        .count();
    let hidden_note = (hidden > 0).then(|| {
        format!(
            "({} can be enabled with the `--unsafe-fixes` option)",
            plural(hidden, "hidden fix", "hidden fixes")
        )
    });
    match (applicable, hidden_note) {
        (0, None) => Ok(()),
        (0, Some(note)) => writeln!(out, "No fixes available {note}."),
        (n, None) => writeln!(
            out,
            "{} {n} fixable with the `--fix` option.",
            style.cyan("[*]")
        ),
        (n, Some(note)) => writeln!(
            out,
            "{} {n} fixable with the `--fix` option {note}.",
            style.cyan("[*]")
        ),
    }
}

struct RuleStatistic {
    code: &'static str,
    name: &'static str,
    count: usize,
    fixable: bool,
}

fn statistics(report: &Report<'_>) -> Vec<RuleStatistic> {
    let mut by_rule: BTreeMap<&'static str, RuleStatistic> = BTreeMap::new();
    for diagnostic in report.diagnostics {
        let entry = by_rule
            .entry(diagnostic.rule.code())
            .or_insert_with(|| RuleStatistic {
                code: diagnostic.rule.code(),
                name: diagnostic.rule.name(),
                count: 0,
                fixable: false,
            });
        entry.count += 1;
        entry.fixable |= report.fix_state(diagnostic) == FixState::Applicable;
    }
    let mut rows: Vec<RuleStatistic> = by_rule.into_values().collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.code.cmp(b.code)));
    rows
}

/// `--statistics`: one line per rule with counts, as a table (`Text` and
/// other human formats) or JSON (`Json`, `JsonLines`).
pub fn render_statistics(
    format: OutputFormat,
    report: &Report<'_>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let rows = statistics(report);
    match format {
        OutputFormat::Json | OutputFormat::JsonLines => {
            let values: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    serde_json::json!({
                        "code": row.code,
                        "name": row.name,
                        "count": row.count,
                        "fixable": row.fixable,
                    })
                })
                .collect();
            if format == OutputFormat::Json {
                serde_json::to_writer_pretty(&mut *out, &values)?;
                writeln!(out)
            } else {
                for value in values {
                    serde_json::to_writer(&mut *out, &value)?;
                    writeln!(out)?;
                }
                Ok(())
            }
        }
        _ => {
            let style = Style::new(report.color);
            let count_width = rows
                .iter()
                .map(|r| r.count.to_string().len())
                .max()
                .unwrap_or(0);
            let code_width = rows.iter().map(|r| r.code.len()).max().unwrap_or(0);
            let any_fixable = rows.iter().any(|r| r.fixable);
            for row in &rows {
                let code = format!("{:<code_width$}", row.code);
                let marker = match (any_fixable, row.fixable) {
                    (false, _) => String::new(),
                    (true, true) => format!("{} ", style.cyan("[*]")),
                    (true, false) => "    ".to_owned(),
                };
                writeln!(
                    out,
                    "{:>count_width$}\t{}\t{marker}{}",
                    row.count,
                    style.bold(&code),
                    row.name
                )?;
            }
            Ok(())
        }
    }
}
