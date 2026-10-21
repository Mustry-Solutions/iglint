//! `iglint ign-lint` end to end, in-process, with a test rule registry and a
//! fake analyzer:
//!
//! * `TST001` (ign-lint `UnusedCustomPropertiesRule`, error): every `"UNUSED"`
//!   key, with an unsafe fix renaming it to `"USED"`;
//! * `TST002` (ign-lint `NamePatternRule`, warning): every `"name"` value
//!   starting with a lowercase letter (except `root`), with a safe fix that
//!   capitalises it.

use std::path::Path;
use std::sync::Arc;

use iglint::{Environment, ExitStatus, Io};
use iglint_linter::registry::{Category, FixAvailability, RuleMeta};
use iglint_linter::settings::LinterSettings;
use iglint_linter::{
    Diagnostic, FileEdit, FileKind, Fix, Registry, RuleCode, Severity, SourceFile,
};
use iglint_text::{TextEdit, TextRange, TextSize};

static UNUSED: RuleMeta = RuleMeta {
    code: "TST001",
    name: "unused-thing",
    category: Category::View,
    default_severity: Severity::Error,
    fix: FixAvailability::Always,
    ign_lint_names: &["UnusedCustomPropertiesRule"],
    ..RuleMeta::DEFAULT
};

static NAMING: RuleMeta = RuleMeta {
    code: "TST002",
    name: "lower-name",
    category: Category::Name,
    default_severity: Severity::Warning,
    fix: FixAvailability::Always,
    ign_lint_names: &["NamePatternRule"],
    ..RuleMeta::DEFAULT
};

static RULES: &[&RuleMeta] = &[&UNUSED, &NAMING];
static REGISTRY: Registry = Registry::new(RULES);

fn range(start: usize, len: usize) -> TextRange {
    TextRange::at(TextSize::from(start), TextSize::from(len))
}

fn analyze(file: &SourceFile, kind: FileKind, settings: &LinterSettings) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if kind != FileKind::View {
        return out;
    }
    let text = file.text();
    let unused = RuleCode::new(&UNUSED);
    if let Some(rule) = settings.rule(unused) {
        for (at, _) in text.match_indices("\"UNUSED\"") {
            out.push(
                Diagnostic::new(
                    unused,
                    rule.severity,
                    "custom property 'UNUSED' is defined but never referenced",
                    file.clone(),
                    range(at + 1, 6),
                )
                .with_node_path("custom.UNUSED")
                .with_fix(Fix::unsafe_edit(
                    "Make UNUSED used",
                    vec![FileEdit {
                        path: file.path().to_path_buf(),
                        edit: TextEdit::replace(range(at + 1, 6), "USED"),
                    }],
                )),
            );
        }
    }
    let naming = RuleCode::new(&NAMING);
    if let Some(rule) = settings.rule(naming) {
        for (at, _) in text.match_indices("\"name\": \"") {
            let start = at + "\"name\": \"".len();
            let end = start + text[start..].find('"').unwrap();
            let name = &text[start..end];
            if name == "root" || !name.starts_with(|c: char| c.is_ascii_lowercase()) {
                continue;
            }
            let fixed = format!("{}{}", name[..1].to_ascii_uppercase(), &name[1..]);
            out.push(
                Diagnostic::new(
                    naming,
                    rule.severity,
                    format!("Name '{name}' doesn't follow PascalCase for component"),
                    file.clone(),
                    range(start, name.len()),
                )
                .with_node_path(format!("root.root.children[0].{name}"))
                .with_fix(Fix::safe(
                    format!("Rename component '{name}' to '{fixed}'"),
                    vec![FileEdit {
                        path: file.path().to_path_buf(),
                        edit: TextEdit::replace(range(start, name.len()), fixed.clone()),
                    }],
                )),
            );
        }
    }
    out
}

const VIEW_BAD: &str = r#"{
  "custom": {"UNUSED": 1},
  "root": {"meta": {"name": "root"}, "children": [{"meta": {"name": "badName"}}]}
}
"#;
const VIEW_CLEAN: &str = r#"{"root": {"meta": {"name": "root"}}}"#;
const VIEW_WARN: &str =
    r#"{"root": {"meta": {"name": "root"}, "children": [{"meta": {"name": "lower"}}]}}"#;

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_in(cwd: &Path, args: &[&str]) -> Run {
    let environment = Environment {
        registry: &REGISTRY,
        analyzer: Some(Arc::new(analyze)),
        cwd: cwd.to_path_buf(),
        vars: Vec::new(),
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin: &[u8] = &[];
    let mut io = Io {
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin: &mut stdin,
        stdout_is_terminal: false,
    };
    let mut argv = vec![std::ffi::OsString::from("ign-lint")];
    argv.extend(args.iter().map(std::ffi::OsString::from));
    let status = iglint::ign_lint::run(argv, &environment, &mut io);
    Run {
        status,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    dir
}

fn standard() -> tempfile::TempDir {
    project(&[
        ("rule_config.json", "{}"),
        ("views/a/view.json", VIEW_BAD),
        ("views/b/view.json", VIEW_CLEAN),
        ("views/c/view.json", VIEW_WARN),
    ])
}

#[test]
fn default_run_layout() {
    let dir = standard();
    let run = run_in(dir.path(), &[]);
    assert_eq!(run.status, ExitStatus::Failure);
    assert_eq!(run.stderr, "");
    let expected = "\
🔧 Loaded configuration from rule_config.json

📄 Evaluating file:
    views/a/view.json

❌ Found 1 errors:

  UnusedCustomPropertiesRule:
    • custom.UNUSED: custom property 'UNUSED' is defined but never referenced


⚠️  Found 1 warnings:

  NamePatternRule:
    • root.root.children[0].badName: Name 'badName' doesn't follow PascalCase for component


📄 Evaluating file:
    views/b/view.json
✅ No issues found

📄 Evaluating file:
    views/c/view.json

⚠️  Found 1 warnings:

  NamePatternRule:
    • root.root.children[0].lower: Name 'lower' doesn't follow PascalCase for component


📈 Summary:
  Files processed: 3
  ⚠️  Total warnings: 2
  ❌ Total errors: 1
  📁 Files with issues: 2
  📁 Clean files: 1
";
    assert_eq!(run.stdout, expected);
}

#[test]
fn ignore_warnings_and_exit_codes() {
    let dir = project(&[("rule_config.json", "{}"), ("v/view.json", VIEW_WARN)]);
    let run = run_in(dir.path(), &[]);
    assert_eq!(run.status, ExitStatus::Failure);
    let run = run_in(dir.path(), &["--ignore-warnings"]);
    assert_eq!(run.status, ExitStatus::Success);
    assert!(
        run.stdout
            .ends_with("  ✅ No errors found (ignoring warnings)\n")
    );

    let clean = project(&[("rule_config.json", "{}"), ("v/view.json", VIEW_CLEAN)]);
    let run = run_in(clean.path(), &[]);
    assert_eq!(run.status, ExitStatus::Success);
    assert!(
        run.stdout.ends_with(
            "\n📈 Summary:\n  Files processed: 1\n  ✅ No style inconsistencies found!\n"
        )
    );

    // A disabled rule does not run.
    let dir = project(&[
        (
            "rule_config.json",
            r#"{"NamePatternRule": {"enabled": false}}"#,
        ),
        ("v/view.json", VIEW_WARN),
    ]);
    assert_eq!(run_in(dir.path(), &[]).status, ExitStatus::Success);
}

#[test]
fn configuration_errors() {
    let dir = project(&[("v/view.json", VIEW_CLEAN)]);
    let run = run_in(dir.path(), &[]);
    assert_eq!(run.status, ExitStatus::Failure);
    assert!(
        run.stdout
            .starts_with("Error loading config file rule_config.json: ")
    );
    assert!(run.stdout.ends_with("❌ No valid configuration found\n"));

    let all_off: String = format!(
        "{{{}}}",
        iglint_config::compat::IGN_LINT_RULES
            .iter()
            .map(|r| format!("\"{}\": {{\"enabled\": false}}", r.name))
            .collect::<Vec<_>>()
            .join(",")
    );
    let dir = project(&[("c.json", &all_off), ("v/view.json", VIEW_CLEAN)]);
    let run = run_in(dir.path(), &["--conf", "c.json"]);
    assert_eq!(run.status, ExitStatus::Failure);
    assert_eq!(
        run.stdout,
        "🔧 Loaded configuration from c.json\n❌ No valid rules configured\n"
    );

    let dir = project(&[
        (
            "rule_config.json",
            r#"{"Bogus": {}, "PollingIntervalRule": {"kwargs": {"x": 1}}}"#,
        ),
        ("v/view.json", VIEW_CLEAN),
    ]);
    let run = run_in(dir.path(), &[]);
    assert!(run.stdout.starts_with(
        "🔧 Loaded configuration from rule_config.json\n\
         Unknown rule in config: Bogus\n\
         Error creating rule PollingIntervalRule: PollingIntervalRule.__init__() got an unexpected keyword argument 'x'\n"
    ));
}

#[test]
fn no_files() {
    let dir = project(&[("rule_config.json", "{}")]);
    let run = run_in(dir.path(), &[]);
    assert_eq!(run.status, ExitStatus::Success);
    assert!(run.stdout.ends_with("❌ No files specified or found\n"));
}

#[test]
fn invalid_json_is_reported_and_the_run_continues() {
    let dir = project(&[
        ("rule_config.json", "{}"),
        ("a/view.json", "{not json"),
        ("b/view.json", VIEW_CLEAN),
        ("c/view.json", "{}"),
    ]);
    let run = run_in(dir.path(), &[]);
    assert_eq!(run.status, ExitStatus::Failure);
    let abs = dir.path().join("a/view.json");
    assert!(
        run.stderr
            .starts_with(&format!("ERROR: Invalid JSON in {}: ", abs.display())),
        "{}",
        run.stderr
    );
    assert!(
        run.stdout
            .contains("    c/view.json\n❌ Failed to read file, skipping\n")
    );
    // b and the empty c count as processed.
    assert!(run.stdout.contains("  Files processed: 2\n"));
}

#[test]
fn explicit_files_and_deprecated_files_form() {
    let dir = standard();
    let run = run_in(
        dir.path(),
        &[
            "--files",
            "views/c/view.json",
            "views/b/view.json",
            "missing.json",
        ],
    );
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert!(lines[1].starts_with("⚠️  Deprecation:"), "{}", run.stdout);
    assert_eq!(lines[2], "Warning: File missing.json does not exist");
    let c = run.stdout.find("    views/c/view.json").unwrap();
    let b = run.stdout.find("    views/b/view.json").unwrap();
    assert!(c < b, "the --files value is linted first");
    assert!(!run.stdout.contains("views/a/view.json"));

    // Comma-separated globs; results not named view.json are dropped.
    let run = run_in(dir.path(), &["--files", "views/a/*.json, rule_config.json"]);
    assert!(run.stdout.contains("  Files processed: 1\n"));
}

#[test]
fn whitelist_skips_and_results_report() {
    let dir = standard();
    std::fs::write(dir.path().join("wl.txt"), "# legacy\nviews/a/view.json\n\n").unwrap();
    let run = run_in(
        dir.path(),
        &[
            "--whitelist",
            "wl.txt",
            "--results-output",
            "out/results.txt",
        ],
    );
    assert!(
        run.stdout
            .contains("🔒 Skipped (whitelisted): views/a/view.json\n")
    );
    assert!(
        run.stdout
            .contains("\n📝 Results written to: out/results.txt\n\n📈 Summary:")
    );
    let wide_eq = "=".repeat(120);
    let wide_dash = "-".repeat(120);
    let expected = format!(
        "{wide_eq}\nIGNITION-LINT RESULTS\n{wide_eq}\n\nSUMMARY\n{wide_dash}\n\
Files processed: 2\nTotal warnings:  1\nTotal errors:    0\nFiles with issues: 1\nClean files:     1\n\
Files whitelisted: 1\n\nWHITELISTED FILES (SKIPPED)\n{wide_dash}\n\
The following 1 file(s) were skipped due to whitelist:\n\n  🔒 views/a/view.json\n\n\
PER-FILE RESULTS\n{wide_eq}\n\n\
{wide_dash}\n✅ views/b/view.json\n{wide_dash}\n\n\n\
{wide_dash}\n⚠️ views/c/view.json\n{wide_dash}\n\n\
WARNINGS (1 total):\n\n  NamePatternRule:\n    • root.root.children[0].lower: Name 'lower' doesn't follow PascalCase for component\n\n\n\
{wide_eq}\nEND OF RESULTS\n{wide_eq}\n"
    );
    let written = std::fs::read_to_string(dir.path().join("out/results.txt")).unwrap();
    assert_eq!(written, expected);

    // A second run within 5 seconds is a parallel batch.
    let run = run_in(dir.path(), &["--results-output", "out/results.txt"]);
    let batch = format!("out/results_pid{}_batch1.txt", std::process::id());
    assert!(
        run.stdout
            .contains(&format!("📝 Results written to: {batch}")),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("📊 Aggregated summary written to: out/results_AGGREGATED_SUMMARY.txt")
    );
    let aggregated =
        std::fs::read_to_string(dir.path().join("out/results_AGGREGATED_SUMMARY.txt")).unwrap();
    assert!(aggregated.contains("Total batches: 2\n"));
    assert!(aggregated.contains("Files processed: 5\n"));

    // --no-whitelist ignores --whitelist.
    let run = run_in(dir.path(), &["--whitelist", "wl.txt", "--no-whitelist"]);
    assert!(!run.stdout.contains("Skipped (whitelisted)"));
}

#[test]
fn generate_whitelist() {
    let dir = standard();
    let run = run_in(
        dir.path(),
        &["--generate-whitelist", "views/**/view.json", "--dry-run"],
    );
    assert_eq!(run.status, ExitStatus::Success);
    assert_eq!(
        run.stdout,
        "🔍 Would add 3 files to whitelist:\n  views/a/view.json\n  views/b/view.json\n  views/c/view.json\n"
    );
    let run = run_in(
        dir.path(),
        &[
            "--generate-whitelist",
            "views/a/*",
            "--whitelist-output",
            "lists/wl.txt",
        ],
    );
    assert_eq!(
        run.stdout,
        "✓ Whitelist generated: lists/wl.txt\n  Total files: 1\n"
    );
    let run = run_in(
        dir.path(),
        &[
            "--generate-whitelist",
            "views/b/view.json",
            "views/a/view.json",
            "--whitelist-output",
            "lists/wl.txt",
            "--append",
        ],
    );
    assert_eq!(
        run.stdout,
        "✓ Whitelist appended to: lists/wl.txt\n  Total files: 2\n  New files: 1\n  Existing files: 1\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("lists/wl.txt")).unwrap(),
        "# Ignition-lint whitelist - files to ignore during linting\n\
         # Lines starting with # are comments\n\
         # One file path per line (relative to repository root)\n\
         # Generated: 1 files added\n\
         # Existing: 1 files\n\
         \n\
         views/a/view.json\n\
         views/b/view.json\n"
    );
}

#[test]
fn fix_applies_safe_fixes_and_relints() {
    let dir = standard();
    let run = run_in(dir.path(), &["--fix", "views/a/view.json"]);
    let expected = "\
🔧 Loaded configuration from rule_config.json

📄 Evaluating file:
    views/a/view.json

Applying fixes to views/a/view.json:
  Applied: Rename component 'badName' to 'BadName'
  File updated: views/a/view.json
  Skipped: Make UNUSED used (unsafe fix (use --fix-unsafe))

Applied: 1 fix(es) | Skipped: 1 fix(es)

❌ Found 1 errors:
";
    assert!(run.stdout.starts_with(expected), "{}", run.stdout);
    assert!(!run.stdout.contains("Found 1 warnings"));
    let fixed = std::fs::read_to_string(dir.path().join("views/a/view.json")).unwrap();
    // Only the name changed; formatting and the final newline are kept.
    assert_eq!(fixed, VIEW_BAD.replace("badName", "BadName"));

    let run = run_in(dir.path(), &["--fix-unsafe", "views/a/view.json"]);
    assert!(run.stdout.contains("  Applied: Make UNUSED used\n"));
    assert_eq!(run.status, ExitStatus::Success);
}

#[test]
fn fix_dry_run_prints_proposed_fixes() {
    let dir = standard();
    let run = run_in(dir.path(), &["--fix-dry-run", "views/a/view.json"]);
    let wide_eq = "=".repeat(120);
    let expected = format!(
        "
{wide_eq}
Proposed Fixes for views/a/view.json:
{wide_eq}

  Fix 1 (UnusedCustomPropertiesRule) [UNSAFE]:
    Make UNUSED used
    Operations:
      REPLACE in views/a/view.json:2:15:
              'UNUSED' -> 'USED'

  Fix 2 (NamePatternRule) [SAFE]:
    Rename component 'badName' to 'BadName'
    Operations:
      REPLACE in views/a/view.json:3:70:
              'badName' -> 'BadName'

Summary: 2 fixes (1 safe, 1 unsafe)
  --fix would apply: 1 safe fix(es)
  --fix --fix-unsafe would apply: 2 fix(es)
"
    );
    assert!(run.stdout.contains(&expected), "{}", run.stdout);
    // Nothing was written.
    assert_eq!(
        std::fs::read_to_string(dir.path().join("views/a/view.json")).unwrap(),
        VIEW_BAD
    );
}

#[test]
fn fix_rules_and_allow_fix() {
    let dir = project(&[
        (
            "rule_config.json",
            r#"{"NamePatternRule": {"allow_fix": false}}"#,
        ),
        ("v/view.json", VIEW_BAD),
    ]);
    // allow_fix false: no fixes for the rule, not even proposed.
    let run = run_in(dir.path(), &["--fix-dry-run"]);
    assert!(run.stdout.contains("Summary: 1 fixes (0 safe, 1 unsafe)"));
    let run = run_in(
        dir.path(),
        &[
            "--fix-dry-run",
            "--fix-rules",
            "NamePatternRule,Bogus,PollingIntervalRule",
        ],
    );
    assert!(run.stdout.contains(
        "⚠️  --fix-rules: 'Bogus' does not match any loaded rule; its fixes cannot apply\n"
    ));
    assert!(
        run.stdout
            .contains("⚠️  --fix-rules: 'PollingIntervalRule' does not support auto-fix\n")
    );
    assert!(run.stdout.contains("Summary: 1 fixes (1 safe, 0 unsafe)\n  Filtered out: 1 (not in --fix-rules)\n  Would apply: 1 fix(es)\n"));
    let run = run_in(
        dir.path(),
        &["--fix-unsafe", "--fix-rules", "NamePatternRule"],
    );
    assert!(
        run.stdout
            .contains("  Applied: Rename component 'badName' to 'BadName'\n")
    );
    assert!(
        run.stdout
            .contains("  Skipped: Make UNUSED used (filtered by --fix-rules)\n")
    );
}

#[test]
fn version_help_and_usage_errors() {
    let dir = standard();
    let run = run_in(dir.path(), &["--version"]);
    assert_eq!(run.status, ExitStatus::Success);
    assert_eq!(
        run.stdout,
        format!("ign-lint {}\n", env!("CARGO_PKG_VERSION"))
    );
    let run = run_in(dir.path(), &["--bogus-flag"]);
    assert_eq!(run.status, ExitStatus::Error);
    assert!(run.stderr.contains("--bogus-flag"));
    let run = run_in(dir.path(), &["--help"]);
    assert_eq!(run.status, ExitStatus::Success);
    for flag in [
        "--config",
        "--files",
        "--verbose",
        "--stats-only",
        "--debug-nodes",
        "--analyze-rules",
        "--debug-output",
        "--ignore-warnings",
        "--timing-output",
        "--results-output",
        "--whitelist",
        "--no-whitelist",
        "--generate-whitelist",
        "--whitelist-output",
        "--append",
        "--dry-run",
        "--fix",
        "--fix-unsafe",
        "--fix-dry-run",
        "--fix-rules",
    ] {
        assert!(run.stdout.contains(flag), "help lacks {flag}");
    }
}

#[test]
fn stats_debug_and_analysis() {
    let dir = standard();
    let run = run_in(
        dir.path(),
        &[
            "--stats-only",
            "--timing-output",
            "t.txt",
            "views/c/view.json",
        ],
    );
    assert_eq!(run.status, ExitStatus::Success);
    assert!(!run.stdout.contains("Loaded configuration"));
    assert!(run.stdout.contains(
        "\n📊 Model statistics for views/c/view.json:\n  Total nodes: 2\n  Node types found:\n    component: 2\n  Components by type:\n    unknown: 2\n"
    ), "{}", run.stdout);
    assert!(
        run.stdout
            .ends_with("  Files processed: 1\n  📊 Statistics analysis complete\n")
    );
    let timing = std::fs::read_to_string(dir.path().join("t.txt")).unwrap();
    for heading in [
        "IGNITION-LINT PERFORMANCE TIMING REPORT",
        "OVERALL SUMMARY",
        "PHASE BREAKDOWN (Totals)",
        "PHASE BREAKDOWN (Averages per file)",
        "RULE EXECUTION SUMMARY",
        "PER-FILE TIMING DETAILS",
        "File: views/c/view.json",
        "END OF REPORT",
    ] {
        assert!(timing.contains(heading), "timing report lacks {heading}");
    }

    let run = run_in(
        dir.path(),
        &[
            "-v",
            "--analyze-rules",
            "--debug-nodes",
            "component",
            "bogus",
            "--",
            "views/c/view.json",
        ],
    );
    assert!(run.stdout.contains(
        "Warning: Unknown node type 'bogus'. Available types: ['component', 'expression_binding'"
    ));
    assert!(run.stdout.contains(
        "\n🔧 Debug info for node types: component\n  1. root.root (component)\n  2. root.root.children[0].lower (component)\n"
    ));
    assert!(run.stdout.contains("  Rule coverage:\n"));
    assert!(
        run.stdout
            .contains("    NamePatternRule: 2 nodes (component)\n")
    );
    assert!(run.stdout.contains(
        "  📋 NamePatternRule:\n    Targets: component\n    Will process: 2 nodes\n    Sample nodes:\n      • root.root: Component 'root' of type 'unknown'\n"
    ));
    assert!(run.stdout.contains("📋 Rules loaded: 9"));
    assert!(run.stdout.contains("  • NamePatternRule -> TST002\n"));
    assert!(run.stdout.contains("📁 Processing 1 files"));

    let run = run_in(
        dir.path(),
        &[
            "--debug-output",
            "dbg",
            "views/a/view.json",
            "views/c/view.json",
        ],
    );
    assert!(
        run.stdout.contains(
            "✅ Debug files saved:\n   - Flattened JSON: dbg/views/a/view_flattened.json\n"
        )
    );
    for file in [
        "views/a/view_flattened.json",
        "views/a/view_model.json",
        "views/c/view_stats.json",
    ] {
        let text = std::fs::read_to_string(dir.path().join("dbg").join(file)).unwrap();
        let _: serde_json::Value = serde_json::from_str(&text).unwrap();
    }
    let stats: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("dbg/views/c/view_stats.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(stats["total_nodes"], 2);
    assert_eq!(stats["rule_coverage"]["NamePatternRule"]["node_count"], 2);
}
