//! JUnit XML: one test suite per file, one failing test case per finding.

use std::io::{self, Write};

use iglint_linter::Diagnostic;

use super::{Report, span};

/// Escapes text for XML attributes and content. Characters XML 1.0 does not
/// allow are replaced.
pub(super) fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

pub(super) fn junit(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let total = report.diagnostics.len();
    writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
    writeln!(
        out,
        r#"<testsuites name="iglint" tests="{}" failures="{total}" errors="0">"#,
        total.max(1)
    )?;
    if report.diagnostics.is_empty() {
        writeln!(
            out,
            r#"  <testsuite name="iglint" tests="1" failures="0" errors="0">"#
        )?;
        writeln!(out, r#"    <testcase name="iglint" classname="iglint"/>"#)?;
        writeln!(out, "  </testsuite>")?;
    }
    let mut groups: Vec<(&std::path::Path, Vec<&Diagnostic>)> = Vec::new();
    for diagnostic in report.diagnostics {
        match groups.last_mut() {
            Some((path, items)) if *path == diagnostic.file.path() => items.push(diagnostic),
            _ => groups.push((diagnostic.file.path(), vec![diagnostic])),
        }
    }
    for (path, items) in groups {
        let path = xml_escape(&report.display_path(path));
        writeln!(
            out,
            r#"  <testsuite name="{path}" tests="{0}" failures="{0}" errors="0">"#,
            items.len()
        )?;
        for diagnostic in items {
            let (start, _) = span(diagnostic);
            let message = xml_escape(&diagnostic.message);
            writeln!(
                out,
                r#"    <testcase name="org.iglint.{}" classname="{path}" file="{path}" line="{}">"#,
                xml_escape(diagnostic.rule.code()),
                start.line
            )?;
            writeln!(
                out,
                r#"      <failure message="{message}" type="{}">{path}:{}:{}: {message}</failure>"#,
                diagnostic.severity, start.line, start.column
            )?;
            writeln!(out, "    </testcase>")?;
        }
        writeln!(out, "  </testsuite>")?;
    }
    writeln!(out, "</testsuites>")
}
