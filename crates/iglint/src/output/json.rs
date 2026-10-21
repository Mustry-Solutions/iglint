//! JSON, JSON Lines and GitLab Code Quality.

use std::collections::HashMap;
use std::hash::Hasher;
use std::io::{self, Write};

use iglint_linter::diagnostic::{Applicability, Fnv64};
use iglint_linter::{Diagnostic, Severity};
use iglint_text::LineColumn;
use serde_json::{Value, json};

use super::{Report, position, span};

fn location(at: LineColumn) -> Value {
    json!({ "row": at.line, "column": at.column })
}

pub(super) fn applicability_name(applicability: Applicability) -> &'static str {
    match applicability {
        Applicability::Safe => "safe",
        Applicability::Unsafe => "unsafe",
        Applicability::DisplayOnly => "display-only",
    }
}

/// The JSON object of one diagnostic.
pub(super) fn diagnostic_value(report: &Report<'_>, diagnostic: &Diagnostic) -> Value {
    let (start, end) = span(diagnostic);
    let fix = diagnostic.fix.as_ref().map(|fix| {
        let edits: Vec<Value> = fix
            .edits
            .iter()
            .map(|edit| {
                // Positions are only known for edits in the diagnostic's
                // own file; other files report byte offsets.
                if edit.path == diagnostic.file.path() {
                    json!({
                        "filename": report.display_path(&edit.path),
                        "content": edit.edit.content,
                        "location": location(position(&diagnostic.file, edit.edit.range.start())),
                        "end_location": location(position(&diagnostic.file, edit.edit.range.end())),
                    })
                } else {
                    json!({
                        "filename": report.display_path(&edit.path),
                        "content": edit.edit.content,
                        "offset": edit.edit.range.start().to_u32(),
                        "end_offset": edit.edit.range.end().to_u32(),
                    })
                }
            })
            .collect();
        json!({
            "applicability": applicability_name(fix.applicability),
            "message": fix.message,
            "edits": edits,
        })
    });
    json!({
        "code": diagnostic.rule.code(),
        "name": diagnostic.rule.name(),
        "severity": diagnostic.severity.as_str(),
        "message": diagnostic.message,
        "filename": report.display_path(diagnostic.file.path()),
        "location": location(start),
        "end_location": location(end),
        "node_path": diagnostic.node_path,
        "fingerprint": report.fingerprint(diagnostic),
        "fix": fix,
        "url": Value::Null,
    })
}

pub(super) fn json(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let values: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|d| diagnostic_value(report, d))
        .collect();
    serde_json::to_writer_pretty(&mut *out, &values)?;
    writeln!(out)
}

pub(super) fn json_lines(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in report.diagnostics {
        serde_json::to_writer(&mut *out, &diagnostic_value(report, diagnostic))?;
        writeln!(out)?;
    }
    Ok(())
}

/// GitLab Code Quality report. Fingerprints must be unique per item, so
/// identical findings get an occurrence counter mixed in.
pub(super) fn gitlab(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let mut seen: HashMap<String, u64> = HashMap::new();
    let values: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let base = report.fingerprint(diagnostic);
            let occurrence = seen.entry(base.clone()).or_default();
            let fingerprint = if *occurrence == 0 {
                base
            } else {
                let mut hasher = Fnv64::default();
                hasher.write(base.as_bytes());
                hasher.write(&occurrence.to_le_bytes());
                format!("{:016x}", hasher.finish())
            };
            *occurrence += 1;
            let (start, end) = span(diagnostic);
            json!({
                "description": format!("{}: {}", diagnostic.rule.code(), diagnostic.message),
                "check_name": diagnostic.rule.code(),
                "fingerprint": fingerprint,
                "severity": match diagnostic.severity {
                    Severity::Error => "major",
                    Severity::Warning => "minor",
                    Severity::Info => "info",
                },
                "location": {
                    "path": report.display_path(diagnostic.file.path()),
                    "lines": { "begin": start.line, "end": end.line },
                },
            })
        })
        .collect();
    serde_json::to_writer_pretty(&mut *out, &values)?;
    writeln!(out)
}
