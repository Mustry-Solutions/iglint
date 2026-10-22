//! Snapshot tests of every output format with synthetic diagnostics.

use std::path::{Path, PathBuf};

use iglint_linter::diagnostic::{FileEdit, Fix, Label, SourceFile};
use iglint_linter::engine::FixMode;
use iglint_linter::registry::{Category, RuleCode, RuleMeta};
use iglint_linter::settings::ProjectSettings;
use iglint_linter::{Diagnostic, Severity};
use iglint_text::{TextEdit, TextRange, TextSize};
use serde_json::Value;

use super::{OutputFormat, Report, render, render_statistics, render_summary};

static UNUSED: RuleMeta = RuleMeta {
    code: "TST001",
    name: "unused-thing",
    category: Category::View,
    default_severity: Severity::Error,
    summary: "Unused custom property",
    explanation: "## What it does\nFlags custom properties nobody reads.",
    ..RuleMeta::DEFAULT
};

static NAMING: RuleMeta = RuleMeta {
    code: "TST002",
    name: "bad-name",
    category: Category::Name,
    default_severity: Severity::Warning,
    summary: "Name does not follow the convention",
    ..RuleMeta::DEFAULT
};

static STRUCTURE: RuleMeta = RuleMeta {
    code: "TST003",
    name: "deep-structure",
    category: Category::View,
    default_severity: Severity::Info,
    summary: "Structure is deeply nested",
    ..RuleMeta::DEFAULT
};

const VIEW: &str = "/repo/projects/p/views/Main/view.json";
const VIEW_TEXT: &str = r#"{
  "custom": {
    "unused": 1,
    "naïve": "<a & 'b'>"
  },
  "root": {
    "meta": {"name": "root"}
  }
}
"#;
const SCRIPT: &str = "/repo/projects/p/ignition/script-python/a/code.py";
const SCRIPT_TEXT: &str = "def f():\r\n\tx = 1\r\n\treturn y\r\n";
const OUTSIDE: &str = "/other/lib.py";

fn range_of(text: &str, needle: &str) -> TextRange {
    let start = text.find(needle).expect("needle in text");
    TextRange::from(start..start + needle.len())
}

fn edit(path: &str, range: TextRange, content: &str) -> FileEdit {
    FileEdit {
        path: PathBuf::from(path),
        edit: TextEdit::replace(range, content),
    }
}

fn diagnostics() -> Vec<Diagnostic> {
    let view = SourceFile::new(VIEW, VIEW_TEXT);
    let script = SourceFile::new(SCRIPT, SCRIPT_TEXT);
    let outside = SourceFile::new(OUTSIDE, "import os\n");
    let unused_line = range_of(VIEW_TEXT, "    \"unused\": 1,\n");
    let root_start = VIEW_TEXT.find("\"root\": {").unwrap();
    let root_end = VIEW_TEXT.rfind("  }").unwrap() + 3;
    let mut all = vec![
        Diagnostic::new(
            RuleCode::new(&UNUSED),
            Severity::Error,
            "Custom property `unused` is never read",
            view.clone(),
            range_of(VIEW_TEXT, "\"unused\""),
        )
        .with_node_path("custom.unused")
        .with_fix(Fix::safe(
            "Remove the property",
            vec![edit(VIEW, unused_line, "")],
        )),
        Diagnostic::new(
            RuleCode::new(&NAMING),
            Severity::Warning,
            "Name \"naïve\" <is> not camelCase; 100% sure, [really]; a]b\nsecond line",
            view.clone(),
            range_of(VIEW_TEXT, "\"naïve\""),
        )
        .with_label(Label {
            file: view.clone(),
            range: range_of(VIEW_TEXT, "\"root\""),
            message: "the view root".to_owned(),
        })
        .with_fix(Fix::unsafe_edit(
            "Rename to `naive`",
            vec![edit(VIEW, range_of(VIEW_TEXT, "naïve"), "naive")],
        )),
        Diagnostic::new(
            RuleCode::new(&STRUCTURE),
            Severity::Info,
            "Root is nested",
            view,
            TextRange::from(root_start..root_end),
        ),
        Diagnostic::new(
            RuleCode::new(&UNUSED),
            Severity::Error,
            "Undefined name `y`",
            script.clone(),
            range_of(SCRIPT_TEXT, "y"),
        ),
        Diagnostic::new(
            RuleCode::new(&NAMING),
            Severity::Warning,
            "Variable `x` is unused",
            script,
            range_of(SCRIPT_TEXT, "x"),
        )
        .with_fix(Fix::display_only(
            "Remove the assignment",
            vec![edit(SCRIPT, range_of(SCRIPT_TEXT, "\tx = 1\r\n"), "")],
        )),
        Diagnostic::new(
            RuleCode::new(&STRUCTURE),
            Severity::Info,
            "Empty range at the start",
            outside,
            TextRange::empty(TextSize::new(0)),
        ),
    ];
    all.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    all
}

fn settings() -> ProjectSettings {
    ProjectSettings {
        root: PathBuf::from("/repo"),
        ..ProjectSettings::default()
    }
}

fn output(format: OutputFormat, diagnostics: &[Diagnostic], color: bool) -> String {
    let settings = settings();
    let report = Report {
        diagnostics,
        cwd: Path::new("/repo"),
        settings: &settings,
        color,
        fix_mode: FixMode::None,
        unsafe_fixes: false,
        files_checked: 3,
        errors: 0,
    };
    let mut out = Vec::new();
    render(format, &report, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

macro_rules! format_snapshot {
    ($test:ident, $format:expr) => {
        #[test]
        fn $test() {
            insta::assert_snapshot!(output($format, &diagnostics(), false));
        }
    };
}

format_snapshot!(text, OutputFormat::Text);
format_snapshot!(concise, OutputFormat::Concise);
format_snapshot!(grouped, OutputFormat::Grouped);
format_snapshot!(json, OutputFormat::Json);
format_snapshot!(json_lines, OutputFormat::JsonLines);
format_snapshot!(sarif, OutputFormat::Sarif);
format_snapshot!(github, OutputFormat::Github);
format_snapshot!(azure, OutputFormat::Azure);
format_snapshot!(junit, OutputFormat::Junit);
format_snapshot!(pylint, OutputFormat::Pylint);
format_snapshot!(gitlab, OutputFormat::Gitlab);

#[test]
fn text_with_colour() {
    let all = diagnostics();
    insta::assert_snapshot!(output(OutputFormat::Text, &all[all.len() - 3..], true));
}

#[test]
fn no_colour_means_no_escape_codes() {
    for format in all_formats() {
        assert!(!output(format, &diagnostics(), false).contains('\x1b'));
    }
}

fn all_formats() -> [OutputFormat; 11] {
    [
        OutputFormat::Text,
        OutputFormat::Concise,
        OutputFormat::Grouped,
        OutputFormat::Json,
        OutputFormat::JsonLines,
        OutputFormat::Sarif,
        OutputFormat::Github,
        OutputFormat::Azure,
        OutputFormat::Junit,
        OutputFormat::Pylint,
        OutputFormat::Gitlab,
    ]
}

#[test]
fn empty() {
    let mut all = String::new();
    for format in all_formats() {
        all.push_str(&format!("--- {format:?}\n{}", output(format, &[], false)));
    }
    insta::assert_snapshot!(all);
}

#[test]
fn unicode_and_crlf_columns() {
    let diagnostics = diagnostics();
    let json: Vec<Value> =
        serde_json::from_str(&output(OutputFormat::Json, &diagnostics, false)).unwrap();
    let naming = json
        .iter()
        .find(|d| d["code"] == "TST002" && d["filename"].as_str().unwrap().ends_with("view.json"))
        .unwrap();
    // `"naïve"` is 7 characters wide on line 4, starting at column 5.
    assert_eq!(naming["location"]["row"], 4);
    assert_eq!(naming["location"]["column"], 5);
    assert_eq!(naming["end_location"]["column"], 12);
    let undefined = json
        .iter()
        .find(|d| d["message"] == "Undefined name `y`")
        .unwrap();
    assert_eq!(undefined["location"]["row"], 3);
    assert_eq!(undefined["location"]["column"], 9);
}

#[test]
fn json_formats_parse() {
    let diagnostics = diagnostics();
    let json: Vec<Value> =
        serde_json::from_str(&output(OutputFormat::Json, &diagnostics, false)).unwrap();
    assert_eq!(json.len(), diagnostics.len());
    for item in &json {
        assert_eq!(item["fingerprint"].as_str().unwrap().len(), 16);
    }
    let lines = output(OutputFormat::JsonLines, &diagnostics, false);
    let parsed: Vec<Value> = lines
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(parsed, json);
    let gitlab: Vec<Value> =
        serde_json::from_str(&output(OutputFormat::Gitlab, &diagnostics, false)).unwrap();
    let mut fingerprints: Vec<&str> = gitlab
        .iter()
        .map(|i| i["fingerprint"].as_str().unwrap())
        .collect();
    fingerprints.sort_unstable();
    fingerprints.dedup();
    assert_eq!(fingerprints.len(), gitlab.len());
}

#[test]
fn gitlab_fingerprints_are_unique_for_identical_findings() {
    let mut twice = diagnostics()[..1].to_vec();
    twice.push(twice[0].clone());
    let gitlab: Vec<Value> =
        serde_json::from_str(&output(OutputFormat::Gitlab, &twice, false)).unwrap();
    assert_ne!(gitlab[0]["fingerprint"], gitlab[1]["fingerprint"]);
}

/// Checks the parts of the SARIF 2.1.0 schema that consumers (GitHub code
/// scanning, Azure DevOps) rely on.
#[test]
fn sarif_structure() {
    let diagnostics = diagnostics();
    let log: Value =
        serde_json::from_str(&output(OutputFormat::Sarif, &diagnostics, false)).unwrap();
    assert_eq!(log["version"], "2.1.0");
    assert_eq!(log["$schema"], super::sarif::SCHEMA);
    let runs = log["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    let driver = &run["tool"]["driver"];
    assert_eq!(driver["name"], "iglint");
    assert!(driver["version"].is_string());
    let rules = driver["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 3);
    for rule in rules {
        assert!(rule["id"].is_string());
        assert!(rule["shortDescription"]["text"].is_string());
        let level = rule["defaultConfiguration"]["level"].as_str().unwrap();
        assert!(["none", "note", "warning", "error"].contains(&level));
    }
    assert!(
        run["originalUriBaseIds"]["%SRCROOT%"]["uri"]
            .as_str()
            .unwrap()
            .ends_with('/')
    );
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), diagnostics.len());
    for result in results {
        assert!(result["message"]["text"].is_string());
        let index = result["ruleIndex"].as_u64().unwrap() as usize;
        assert_eq!(rules[index]["id"], result["ruleId"]);
        let level = result["level"].as_str().unwrap();
        assert!(["none", "note", "warning", "error"].contains(&level));
        let location = &result["locations"][0]["physicalLocation"];
        assert!(location["artifactLocation"]["uri"].is_string());
        let region = &location["region"];
        for key in ["startLine", "startColumn", "endLine", "endColumn"] {
            assert!(region[key].as_u64().unwrap() >= 1, "{key}");
        }
        assert!(region["endLine"].as_u64() >= region["startLine"].as_u64());
        assert!(result["partialFingerprints"]["iglint/v1"].is_string());
        if let Some(fixes) = result.get("fixes") {
            for fix in fixes.as_array().unwrap() {
                assert!(fix["description"]["text"].is_string());
                let changes = fix["artifactChanges"].as_array().unwrap();
                assert!(!changes.is_empty());
                for change in changes {
                    assert!(change["artifactLocation"]["uri"].is_string());
                    for replacement in change["replacements"].as_array().unwrap() {
                        assert!(replacement["deletedRegion"]["byteOffset"].is_u64());
                        assert!(replacement["insertedContent"]["text"].is_string());
                    }
                }
            }
        }
    }
}

#[test]
fn junit_is_well_formed_enough() {
    let xml = output(OutputFormat::Junit, &diagnostics(), false);
    assert_eq!(xml.matches("<testsuite ").count(), 3);
    assert_eq!(xml.matches("</testsuite>").count(), 3);
    assert_eq!(xml.matches("<failure ").count(), 6);
    // No raw markup characters leak from messages.
    assert!(!xml.contains("<is>"));
    assert!(xml.contains("&lt;is&gt;"));
}

fn summary(
    diagnostics: &[Diagnostic],
    fixed: usize,
    fix_mode: FixMode,
    unsafe_fixes: bool,
) -> String {
    let settings = settings();
    let report = Report {
        diagnostics,
        cwd: Path::new("/repo"),
        settings: &settings,
        color: false,
        fix_mode,
        unsafe_fixes,
        files_checked: 3,
        errors: 0,
    };
    let mut out = Vec::new();
    render_summary(&report, fixed, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn summaries() {
    let all = diagnostics();
    assert_eq!(
        summary(&[], 0, FixMode::None, false),
        "All checks passed!\n"
    );
    assert_eq!(
        summary(&[], 2, FixMode::Safe, false),
        "Fixed 2 issues.\nAll checks passed!\n"
    );
    let settings = settings();
    let report = Report {
        diagnostics: &[],
        cwd: Path::new("/repo"),
        settings: &settings,
        color: false,
        fix_mode: FixMode::None,
        unsafe_fixes: false,
        files_checked: 3,
        errors: 1,
    };
    let mut out = Vec::new();
    render_summary(&report, 0, &mut out).unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "No findings, but the check is incomplete: 1 error.\n"
    );
    assert_eq!(
        summary(&all, 0, FixMode::None, false),
        "Found 2 errors, 2 warnings and 2 info.\n[*] 1 fixable with the `--fix` option (1 hidden fix can be enabled with the `--unsafe-fixes` option).\n"
    );
    assert_eq!(
        summary(&all, 0, FixMode::None, true),
        "Found 2 errors, 2 warnings and 2 info.\n[*] 2 fixable with the `--fix` option.\n"
    );
    let one_error: Vec<Diagnostic> = all
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .take(1)
        .cloned()
        .collect();
    assert_eq!(
        summary(&one_error, 1, FixMode::Safe, false),
        "Fixed 1 issue.\nFound 1 error.\n"
    );
    let unsafe_only: Vec<Diagnostic> = all
        .iter()
        .filter(|d| d.rule.code() == "TST002")
        .cloned()
        .collect();
    assert_eq!(
        summary(&unsafe_only, 0, FixMode::None, false),
        "Found 2 warnings.\nNo fixes available (1 hidden fix can be enabled with the `--unsafe-fixes` option).\n"
    );
}

#[test]
fn statistics() {
    let diagnostics = diagnostics();
    let settings = settings();
    let report = Report {
        diagnostics: &diagnostics,
        cwd: Path::new("/repo"),
        settings: &settings,
        color: false,
        fix_mode: FixMode::None,
        unsafe_fixes: false,
        files_checked: 3,
        errors: 0,
    };
    let mut text = Vec::new();
    render_statistics(OutputFormat::Text, &report, &mut text).unwrap();
    let mut json = Vec::new();
    render_statistics(OutputFormat::Json, &report, &mut json).unwrap();
    let parsed: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 3);
    insta::assert_snapshot!(format!(
        "{}\n{}",
        String::from_utf8(text).unwrap(),
        String::from_utf8(json).unwrap()
    ));
}
