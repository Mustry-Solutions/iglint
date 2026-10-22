//! SARIF 2.1.0.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

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
    let mut by_file: BTreeMap<&Path, Vec<Value>> = BTreeMap::new();
    for edit in &fix.edits {
        let range = edit.edit.range;
        by_file.entry(edit.path.as_path()).or_default().push(json!({
            "deletedRegion": {
                "byteOffset": range.start().to_u32(),
                "byteLength": range.len().to_u32(),
            },
            "insertedContent": { "text": edit.edit.content },
        }));
    }
    let changes: Vec<Value> = by_file
        .into_iter()
        .map(|(path, replacements)| {
            json!({
                "artifactLocation": artifact_location(report, path),
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
                        "artifactLocation": artifact_location(report, diagnostic.file.path()),
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
                                "artifactLocation": artifact_location(report, label.file.path()),
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
fn file_uri(dir: &Path) -> String {
    let mut uri = absolute_file_uri(dir);
    if !uri.ends_with('/') {
        uri.push('/');
    }
    uri
}

/// `file:///...` URI of an absolute path.
fn absolute_file_uri(path: &Path) -> String {
    let path = super::display_path(path, Path::new(""));
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for (i, segment) in path.split('/').enumerate() {
        if i > 0 {
            uri.push('/');
        }
        // A Windows drive (`C:`) keeps its colon.
        let drive = i <= 1
            && segment.len() == 2
            && segment.as_bytes()[0].is_ascii_alphabetic()
            && segment.as_bytes()[1] == b':';
        if drive {
            uri.push_str(segment);
        } else {
            uri.push_str(&encode_segment(segment));
        }
    }
    uri
}

/// RFC 3986 percent-encoding of one path segment: everything but the
/// unreserved characters.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The `artifactLocation` of a file: a percent-encoded URI relative to
/// `%SRCROOT%` (the working directory) for files below it, else an
/// absolute `file:` URI.
pub(super) fn artifact_location(report: &Report<'_>, path: &Path) -> Value {
    match path.strip_prefix(report.cwd) {
        Ok(relative) => {
            let relative = iglint_linter::discovery::slash_path(relative);
            let uri: Vec<String> = relative.split('/').map(encode_segment).collect();
            json!({ "uri": uri.join("/"), "uriBaseId": "%SRCROOT%" })
        }
        Err(_) => json!({ "uri": absolute_file_uri(path) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(
            encode_segment("project#1 with space"),
            "project%231%20with%20space"
        );
        assert_eq!(encode_segment("é:%?"), "%C3%A9%3A%25%3F");
        assert_eq!(
            absolute_file_uri(Path::new("/a b/c#d")),
            "file:///a%20b/c%23d"
        );
        assert_eq!(file_uri(Path::new("/x y")), "file:///x%20y/");
    }
}
