//! CI annotations: GitHub Actions workflow commands and Azure Pipelines
//! logging commands.

use std::io::{self, Write};

use iglint_linter::Severity;

use super::{Report, span};

/// Escapes the message part of a GitHub workflow command.
fn github_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a property value of a GitHub workflow command.
fn github_property(text: &str) -> String {
    github_data(text).replace(':', "%3A").replace(',', "%2C")
}

pub(super) fn github(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in report.diagnostics {
        let (start, end) = span(diagnostic);
        let command = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "notice",
        };
        writeln!(
            out,
            "::{command} file={},line={},col={},endLine={},endColumn={},title={}::{}",
            github_property(&report.display_path(diagnostic.file.path())),
            start.line,
            start.column,
            end.line,
            end.column,
            github_property(diagnostic.rule.code()),
            github_data(&diagnostic.message)
        )?;
    }
    Ok(())
}

/// Escapes text in an Azure Pipelines logging command.
fn azure_escape(text: &str) -> String {
    text.replace('%', "%AZP25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(';', "%3B")
        .replace(']', "%5D")
}

pub(super) fn azure(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in report.diagnostics {
        let (start, _) = span(diagnostic);
        let kind = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning | Severity::Info => "warning",
        };
        writeln!(
            out,
            "##vso[task.logissue type={kind};sourcepath={};linenumber={};columnnumber={};code={}]{}",
            azure_escape(&report.display_path(diagnostic.file.path())),
            start.line,
            start.column,
            azure_escape(diagnostic.rule.code()),
            azure_escape(&diagnostic.message)
        )?;
    }
    Ok(())
}
