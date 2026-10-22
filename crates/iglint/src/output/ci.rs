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

/// Escapes the message of an Azure Pipelines logging command: the agent
/// unescapes `%AZP25` (`%`), `%0D` and `%0A` in it (and `%3B`, `%5D`,
/// which an escaped `%` can no longer form). `;` and `]` need no escaping
/// after the closing `]` of the properties.
fn azure_data(text: &str) -> String {
    text.replace('%', "%AZP25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a property value of an Azure Pipelines logging command: also
/// `;` (separates properties) and `]` (ends them).
fn azure_property(text: &str) -> String {
    azure_data(text).replace(';', "%3B").replace(']', "%5D")
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
            azure_property(&report.display_path(diagnostic.file.path())),
            start.line,
            start.column,
            azure_property(diagnostic.rule.code()),
            azure_data(&diagnostic.message)
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_escaping() {
        assert_eq!(github_data("a%b\r\nc:d,e"), "a%25b%0D%0Ac:d,e");
        assert_eq!(github_property("p%1:2,3\n"), "p%251%3A2%2C3%0A");
    }

    #[test]
    fn azure_escaping() {
        assert_eq!(azure_data("a;b]c%0A\r\n"), "a;b]c%AZP250A%0D%0A");
        assert_eq!(azure_property("x;y]z%\n"), "x%3By%5Dz%AZP25%0A");
    }
}
