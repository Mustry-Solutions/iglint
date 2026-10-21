//! Output formats for diagnostics.

use std::io::{self, Write};
use std::path::Path;

use iglint_linter::Diagnostic;
use iglint_linter::engine::FixMode;
use iglint_linter::settings::ProjectSettings;

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

/// Writes `report.diagnostics` in `format`.
pub fn render(format: OutputFormat, report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let _ = (format, report, out);
    unimplemented!()
}

/// The closing summary of the human-readable formats: `Found 3 errors.`,
/// fix hints, or `All checks passed!`. `fixed` is the number of fixes
/// applied in this run.
pub fn render_summary(report: &Report<'_>, fixed: usize, out: &mut dyn Write) -> io::Result<()> {
    let _ = (report, fixed, out);
    unimplemented!()
}

/// `--statistics`: one line per rule with counts, as a table (`Text` and
/// other human formats) or JSON (`Json`, `JsonLines`).
pub fn render_statistics(
    format: OutputFormat,
    report: &Report<'_>,
    out: &mut dyn Write,
) -> io::Result<()> {
    let _ = (format, report, out);
    unimplemented!()
}
