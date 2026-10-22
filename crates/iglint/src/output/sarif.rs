//! SARIF 2.1.0.

use std::collections::BTreeMap;
use std::io::{self, Write};

use iglint_linter::registry::RuleCode;
use iglint_linter::{Diagnostic, Severity};
use serde_json::{Value, json};

use super::{Report, span};

pub(super) const SCHEMA: &str = "https://json.schemastore.org/sarif-2.1.0.json";

fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "note",
    }
}

fn rule_descriptor(rule: RuleCode) -> Value {
    let meta = rule.meta();
    let mut descriptor = json!({
        "id": meta.code,
        "name": meta.name,
        "shortDescription": { "text": meta.summary },
        "helpUri": rule.docs_url(),
        "defaultConfiguration": { "level": level(meta.default_severity) },
        "properties": { "category": meta.category.title() },
    });
    if !meta.explanation.is_empty() {
        descriptor["fullDescription"] = json!({ "text": meta.explanation });
        descriptor["help"] = json!({ "text": meta.summary, "markdown": meta.explanation });
    }
    descriptor
}

fn fixes(report: &Report<'_>, diagnostic: &Diagnostic) -> Option<Value> {
    let fix = diagnostic.fix.as_ref()?;
    let mut by_file: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for edit in &fix.edits {
        let range = edit.edit.range;
        by_file
            .entry(report.display_path(&edit.path))
            .or_default()
            .push(json!({
                "deletedRegion": {
                    "byteOffset": range.start().to_u32(),
                    "byteLength": range.len().to_u32(),
                },
                "insertedContent": { "text": edit.edit.content },
            }));
    }
    let changes: Vec<Value> = by_file
        .into_iter()
        .map(|(uri, replacements)| {
            json!({
                "artifactLocation": { "uri": uri, "uriBaseId": "%SRCROOT%" },
                "replacements": replacements,
            })
        })
        .collect();
    Some(json!([{
        "description": { "text": fix.message },
        "artifactChanges": changes,
    }]))
}

pub(super) fn sarif(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let mut rules: Vec<RuleCode> = report.diagnostics.iter().map(|d| d.rule).collect();
    rules.sort();
    rules.dedup();
    let results: Vec<Value> = report
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let (start, end) = span(diagnostic);
            let index = rules
                .binary_search(&diagnostic.rule)
                .expect("every rule is listed");
            let mut result = json!({
                "ruleId": diagnostic.rule.code(),
                "ruleIndex": index,
                "level": level(diagnostic.severity),
                "message": { "text": diagnostic.message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": report.display_path(diagnostic.file.path()),
                            "uriBaseId": "%SRCROOT%",
                        },
                        "region": {
                            "startLine": start.line,
                            "startColumn": start.column,
                            "endLine": end.line,
                            "endColumn": end.column,
                        },
                    },
                }],
                "partialFingerprints": { "iglint/v1": report.fingerprint(diagnostic) },
            });
            if let Some(node_path) = &diagnostic.node_path {
                result["locations"][0]["logicalLocations"] =
                    json!([{ "fullyQualifiedName": node_path }]);
            }
            if !diagnostic.labels.is_empty() {
                result["relatedLocations"] = diagnostic
                    .labels
                    .iter()
                    .enumerate()
                    .map(|(id, label)| {
                        let start = super::position(&label.file, label.range.start());
                        let end = super::position(&label.file, label.range.end());
                        json!({
                            "id": id,
                            "message": { "text": label.message },
                            "physicalLocation": {
                                "artifactLocation": {
                                    "uri": report.display_path(label.file.path()),
                                    "uriBaseId": "%SRCROOT%",
                                },
                                "region": {
                                    "startLine": start.line,
                                    "startColumn": start.column,
                                    "endLine": end.line,
                                    "endColumn": end.column,
                                },
                            },
                        })
                    })
                    .collect();
            }
            if let Some(fixes) = fixes(report, diagnostic) {
                result["fixes"] = fixes;
            }
            result
        })
        .collect();
    let root_uri = file_uri(report.cwd);
    let log = json!({
        "$schema": SCHEMA,
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "iglint",
                    "informationUri": "https://github.com/Mustry-Solutions/iglint",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules.iter().map(|r| rule_descriptor(*r)).collect::<Vec<_>>(),
                },
            },
            "originalUriBaseIds": { "%SRCROOT%": { "uri": root_uri } },
            "columnKind": "unicodeCodePoints",
            "results": results,
        }],
    });
    serde_json::to_writer_pretty(&mut *out, &log)?;
    writeln!(out)
}

/// `file:///...` URI of a directory, with a trailing slash as SARIF requires
/// for base URIs.
fn file_uri(dir: &std::path::Path) -> String {
    let path = super::display_path(dir, std::path::Path::new(""));
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(byte as char)
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    if !uri.ends_with('/') {
        uri.push('/');
    }
    uri
}
