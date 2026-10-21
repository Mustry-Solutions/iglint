//! In-process tests of the `iglint` command line, with the fake rules of
//! `iglint_linter::testing` (see that module for the trigger words).

use std::path::{Path, PathBuf};

use iglint::{Environment, ExitStatus, Io};
use iglint_linter::testing;

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn run_with_stdin(dir: &Path, args: &[&str], stdin: &str) -> Run {
    let environment = Environment {
        registry: &testing::REGISTRY,
        analyzer: Some(testing::analyzer()),
        cwd: dir.to_path_buf(),
        vars: Vec::new(),
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut input = stdin.as_bytes();
    let mut io = Io {
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin: &mut input,
        stdout_is_terminal: false,
    };
    let mut argv = vec!["iglint".into()];
    argv.extend(args.iter().map(std::ffi::OsString::from));
    let status = iglint::run(argv, &environment, &mut io);
    Run {
        status,
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

fn run(dir: &Path, args: &[&str]) -> Run {
    run_with_stdin(dir, args, "")
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "iglint.toml", "");
    write(
        dir.path(),
        "projects/p/ignition/script-python/lib/code.py",
        "def f():\n    unused_var = undefined_thing\n",
    );
    write(
        dir.path(),
        "projects/p/ignition/script-python/lib/resource.json",
        "{}\n",
    );
    dir
}

#[test]
fn default_command_reports_findings() {
    let dir = project();
    let explicit = run(dir.path(), &["check", "--no-cache"]);
    let implicit = run(dir.path(), &["--no-cache"]);
    assert_eq!(explicit.stdout, implicit.stdout);
    assert_eq!(explicit.status, ExitStatus::Failure);
    insta::assert_snapshot!("default_text", explicit.stdout);

    let concise = run(
        dir.path(),
        &["--no-cache", "--output-format", "concise", "projects"],
    );
    assert_eq!(
        concise.stdout,
        "projects/p/ignition/script-python/lib/code.py:2:5: SCR102 [warning] `unused_var` triggers unused-variable\n\
         projects/p/ignition/script-python/lib/code.py:2:18: SCR101 [error] `undefined_thing` triggers undefined-name\n\
         Found 1 error and 1 warning.\n\
         [*] 1 fixable with the `--fix` option.\n"
    );
}

#[test]
fn exit_status_threshold() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "project.json", "{}\n");
    write(dir.path(), "a.py", "unused_var\nbad_prop\n");
    // A warning fails by default.
    assert_eq!(run(dir.path(), &["--isolated"]).status, ExitStatus::Failure);
    assert_eq!(
        run(dir.path(), &["--isolated", "--error-on", "error"]).status,
        ExitStatus::Success
    );
    assert_eq!(
        run(dir.path(), &["--isolated", "--ignore-warnings"]).status,
        ExitStatus::Success
    );
    assert_eq!(
        run(dir.path(), &["--isolated", "--exit-zero"]).status,
        ExitStatus::Success
    );
    // Info findings only fail with --error-on info.
    let info = ["--isolated", "--select", "NAME002"];
    assert_eq!(run(dir.path(), &info).status, ExitStatus::Success);
    let mut strict = info.to_vec();
    strict.extend(["--error-on", "info"]);
    assert_eq!(run(dir.path(), &strict).status, ExitStatus::Failure);
    // Usage errors.
    assert_eq!(
        run(dir.path(), &["--error-on", "fatal"]).status,
        ExitStatus::Error
    );
    let missing = run(dir.path(), &["--isolated", "nope.py"]);
    assert_eq!(missing.status, ExitStatus::Error);
    assert!(
        missing
            .stderr
            .contains("error: nope.py: No such file or directory"),
        "{}",
        missing.stderr
    );
}

#[test]
fn fixes() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "project.json", "{}\n");
    let file = write(dir.path(), "a.py", "unused_var\nunusedProp\n");

    let diff = run(dir.path(), &["--isolated", "--diff"]);
    assert_eq!(diff.status, ExitStatus::Failure);
    assert_eq!(
        diff.stdout,
        "--- a/a.py\n+++ b/a.py\n@@ -1,2 +1,2 @@\n-unused_var\n+_\n unusedProp\n\nWould fix 1 issue(s) in 1 file(s).\n"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "unused_var\nunusedProp\n"
    );

    let fixed = run(
        dir.path(),
        &[
            "--isolated",
            "--fix",
            "--exit-non-zero-on-fix",
            "--output-format",
            "concise",
        ],
    );
    assert_eq!(fixed.status, ExitStatus::Failure);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "_\nunusedProp\n");
    assert!(fixed.stdout.contains("Fixed 1 issue."), "{}", fixed.stdout);
    assert!(
        fixed.stdout.contains("a.py:2:1: VIEW001"),
        "{}",
        fixed.stdout
    );

    let only = run(dir.path(), &["--isolated", "--fix-only", "--unsafe-fixes"]);
    assert_eq!(only.status, ExitStatus::Success);
    assert_eq!(only.stdout, "Fixed 1 issue(s) (0 remaining).\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "_\nusedProp\n");
}

#[test]
fn unsafe_fix_hint() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "project.json", "{}\n");
    write(dir.path(), "a.py", "unusedProp\n");
    let result = run(dir.path(), &["--isolated", "--output-format", "concise"]);
    assert!(
        result.stdout.ends_with(
            "No fixes available (1 hidden fix can be enabled with the `--unsafe-fixes` option).\n"
        ),
        "{}",
        result.stdout
    );
}

#[test]
fn statistics_json_and_output_file() {
    let dir = project();
    let stats = run(dir.path(), &["--statistics"]);
    assert!(stats.stdout.contains("SCR101"), "{}", stats.stdout);
    assert!(stats.stdout.contains("undefined-name"), "{}", stats.stdout);

    let json = run(
        dir.path(),
        &[
            "--output-format",
            "json",
            "--output-file",
            "out/report.json",
        ],
    );
    assert_eq!(json.status, ExitStatus::Failure);
    assert_eq!(json.stdout, "");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("out/report.json")).unwrap())
            .unwrap();
    assert_eq!(report.as_array().unwrap().len(), 2);
    assert_eq!(report[0]["code"], "SCR102");
    assert_eq!(
        report[0]["filename"],
        "projects/p/ignition/script-python/lib/code.py"
    );

    // output-format from the configuration.
    write(dir.path(), "iglint.toml", "output-format = \"github\"\n");
    let github = run(dir.path(), &[]);
    assert!(
        github.stdout.starts_with(
            "::warning file=projects/p/ignition/script-python/lib/code.py,line=2,col=5,"
        ),
        "{}",
        github.stdout
    );
}

#[test]
fn configuration_errors_exit_2_with_position() {
    let dir = project();
    write(
        dir.path(),
        "iglint.toml",
        "[rules.SCR102]\nignore-name = []\n",
    );
    let result = run(dir.path(), &[]);
    assert_eq!(result.status, ExitStatus::Error);
    let path = dir.path().join("iglint.toml");
    assert_eq!(
        result.stderr,
        format!(
            "error: {}:2:1: unknown option `ignore-name` for rule SCR102 (unused-variable); did you mean `ignore-names`?\n",
            path.display()
        )
    );
}

#[test]
fn selection_from_config_and_cli() {
    let dir = project();
    write(dir.path(), "iglint.toml", "ignore = [\"SCR101\"]\n");
    let result = run(dir.path(), &["--output-format", "concise"]);
    assert!(!result.stdout.contains("SCR101"), "{}", result.stdout);
    let result = run(
        dir.path(),
        &["--output-format", "concise", "--select", "SCR101"],
    );
    assert!(result.stdout.contains("SCR101"));
    assert!(!result.stdout.contains("SCR102"));
    let result = run(dir.path(), &["--output-format", "concise", "--isolated"]);
    assert!(result.stdout.contains("SCR101"));
}

#[test]
fn show_settings_and_files() {
    let dir = project();
    write(
        dir.path(),
        "iglint.toml",
        "exclude = [\"tmp\"]\n[rules.SCR102]\nignore-names = [\"x\"]\nexclude = [\"legacy/**\"]\n",
    );
    write(dir.path(), "tmp/x.py", "");
    let settings = run(
        dir.path(),
        &[
            "--show-settings",
            "projects/p/ignition/script-python/lib/code.py",
        ],
    );
    assert_eq!(settings.status, ExitStatus::Success);
    let expected = format!(
        "Configuration: iglint.toml\nRoot: {}\nSettings for: projects/p/ignition/script-python/lib/code.py\n  strict = false\n  respect-gitignore = true\n  exclude = [\"tmp\"]\n  overrides = 0\n  enabled rules (4):\n    NAME001 component-name [warning]\n    SCR101 undefined-name [error]\n    SCR102 unused-variable [warning]\n      ignore-names = [\"x\"]\n      exclude = [\"legacy/**\"]\n    VIEW001 unused-custom-property [warning]\n",
        dir.path().display()
    );
    assert_eq!(settings.stdout, expected);

    let files = run(dir.path(), &["--show-files"]);
    assert_eq!(
        files.stdout,
        "projects/p/ignition/script-python/lib/code.py\nprojects/p/ignition/script-python/lib/resource.json\n"
    );
}

#[test]
fn walks_only_lint_python_inside_ignition_trees() {
    let dir = project();
    write(dir.path(), "tools/x.py", "unused_var\n");
    write(dir.path(), "projects/p/project.json", "{}\n");
    write(dir.path(), "projects/p/scripts/stray.py", "");
    let files = run(dir.path(), &["--show-files"]);
    assert_eq!(
        files.stdout,
        "projects/p/ignition/script-python/lib/code.py\nprojects/p/ignition/script-python/lib/resource.json\nprojects/p/project.json\nprojects/p/scripts/stray.py\n"
    );
    let named = run(dir.path(), &["--show-files", "tools/x.py"]);
    assert_eq!(named.stdout, "tools/x.py\n");
    let linted = run(
        dir.path(),
        &["--output-format", "concise", "--no-cache", "tools/x.py"],
    );
    assert_eq!(linted.status, ExitStatus::Failure);
    assert!(
        linted.stdout.contains("tools/x.py:1:1: SCR102"),
        "{}",
        linted.stdout
    );
}

#[test]
fn baseline_workflow() {
    let dir = project();
    let root = dir.path();
    let created = run(root, &["baseline"]);
    assert_eq!(created.status, ExitStatus::Success);
    assert_eq!(
        created.stderr,
        "Wrote 2 finding(s) to the baseline .iglint-baseline.json.\n"
    );
    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".iglint-baseline.json")).unwrap())
            .unwrap();
    assert_eq!(baseline["version"], 1);
    assert_eq!(baseline["entries"].as_array().unwrap().len(), 2);

    // Known findings no longer fail; the baseline is found automatically.
    let clean = run(root, &[]);
    assert_eq!(clean.status, ExitStatus::Success, "{}", clean.stdout);
    assert_eq!(clean.stdout, "All checks passed!\n");
    assert_eq!(
        clean.stderr,
        "2 finding(s) hidden by the baseline .iglint-baseline.json.\n"
    );

    // A new finding fails; a fixed one is reported as stale.
    write(
        root,
        "projects/p/ignition/script-python/lib/code.py",
        "def f():\n    unused_var = 1\n    undefined_thing\n    undefined_thing\n",
    );
    let changed = run(root, &["--output-format", "concise"]);
    assert_eq!(changed.status, ExitStatus::Failure);
    assert!(
        changed.stdout.contains(":4:5: SCR101"),
        "{}",
        changed.stdout
    );
    assert!(
        !changed.stdout.contains(":2:5: SCR102"),
        "{}",
        changed.stdout
    );

    write(
        root,
        "projects/p/ignition/script-python/lib/code.py",
        "def f():\n    undefined_thing\n",
    );
    let stale = run(root, &[]);
    assert_eq!(stale.status, ExitStatus::Success);
    assert!(
        stale.stderr.contains("note: 1 baseline entry in .iglint-baseline.json no longer match a finding; run with --prune-baseline to remove it."),
        "{}",
        stale.stderr
    );
    let pruned = run(root, &["--prune-baseline"]);
    assert!(
        pruned
            .stderr
            .contains("Removed 1 fixed finding(s) from the baseline"),
        "{}",
        pruned.stderr
    );
    let baseline: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(".iglint-baseline.json")).unwrap())
            .unwrap();
    assert_eq!(baseline["entries"].as_array().unwrap().len(), 1);

    // An explicit baseline that does not exist is an error.
    let missing = run(root, &["--baseline", "nope.json"]);
    assert_eq!(missing.status, ExitStatus::Error);
    assert!(
        missing
            .stderr
            .starts_with("error: cannot read baseline nope.json"),
        "{}",
        missing.stderr
    );
}

#[test]
fn stdin_filename() {
    let dir = project();
    let fixed = run_with_stdin(
        dir.path(),
        &[
            "--stdin-filename",
            "projects/p/x.py",
            "--fix",
            "--output-format",
            "concise",
        ],
        "unused_var\nundefined_thing\n",
    );
    assert_eq!(fixed.stdout, "_\nundefined_thing\n");
    assert!(
        fixed.stderr.contains("projects/p/x.py:2:1: SCR101"),
        "{}",
        fixed.stderr
    );
    assert_eq!(fixed.status, ExitStatus::Failure);
    assert!(!dir.path().join("projects/p/x.py").exists());

    let report = run_with_stdin(
        dir.path(),
        &[
            "--stdin-filename",
            "projects/p/x.py",
            "--output-format",
            "json",
        ],
        "undefined_thing\n",
    );
    let value: serde_json::Value = serde_json::from_str(&report.stdout).unwrap();
    assert_eq!(value[0]["filename"], "projects/p/x.py");
}

#[test]
fn legacy_configuration_is_used_with_a_deprecation_warning() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "rule_config.json",
        r#"{"PollingIntervalRule": {"enabled": false}}"#,
    );
    write(
        dir.path(),
        "views/A/view.json",
        "{\"x\": \"now(1) unusedProp\"}\n",
    );
    let result = run(dir.path(), &["--output-format", "concise"]);
    assert!(
        result
            .stderr
            .starts_with("warning: using the ign-lint configuration"),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("run `iglint migrate`"));
    assert!(!result.stdout.contains("BIND001"), "{}", result.stdout);
    assert!(result.stdout.contains("VIEW001"), "{}", result.stdout);
}

#[test]
fn migrate_writes_config_and_baseline_from_the_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "rule_config.json",
        r#"{"_comment": "CI rules", "PollingIntervalRule": {"kwargs": {"minimum_interval": 5000}}}"#,
    );
    write(
        root,
        ".ignition-lint-whitelist.txt",
        "views/Old/view.json\nviews/Gone/view.json\n",
    );
    write(
        root,
        "views/Old/view.json",
        "{\"a\": \"now(1)\", \"b\": \"unusedProp\"}\n",
    );
    write(root, "views/New/view.json", "{}\n");

    let dry = run(root, &["migrate", "--dry-run"]);
    assert_eq!(dry.status, ExitStatus::Success);
    assert!(dry.stdout.contains("[rules.BIND001]\n"), "{}", dry.stdout);
    assert!(!root.join("iglint.toml").exists());

    let migrated = run(root, &["migrate"]);
    assert_eq!(migrated.status, ExitStatus::Success, "{}", migrated.stderr);
    assert_eq!(
        migrated.stdout,
        "Wrote iglint.toml from rule_config.json.\n  NamePatternRule -> NAME001, NAME002\n  PollingIntervalRule -> BIND001\n  PylintScriptRule: no iglint equivalent, not migrated\n  BadComponentReferenceRule: no iglint equivalent, not migrated\n  ComponentReferenceValidationRule: no iglint equivalent, not migrated\n  UnusedCustomPropertiesRule -> VIEW001\n  ExcessiveContextDataRule: no iglint equivalent, not migrated\n  PropertyAccessRule: no iglint equivalent, not migrated\n  PropertyPersistenceRule: no iglint equivalent, not migrated\nWrote .iglint-baseline.json with 2 finding(s) in the 1 file(s) of the whitelist .ignition-lint-whitelist.txt.\n  1 whitelisted file(s) no longer exist and were dropped.\nThe whitelist is no longer needed: whitelisted files are linted, and only new findings fail.\n"
    );
    let toml = std::fs::read_to_string(root.join("iglint.toml")).unwrap();
    assert!(toml.contains("# CI rules\n"), "{toml}");
    assert!(toml.contains("minimum-interval = 5000\n"), "{toml}");

    // The migrated project passes: the old view's findings are baselined.
    let check = run(root, &[]);
    assert_eq!(
        check.status,
        ExitStatus::Success,
        "{}{}",
        check.stdout,
        check.stderr
    );

    // Existing files are not overwritten without --force.
    let again = run(root, &["migrate"]);
    assert_eq!(again.status, ExitStatus::Error);
    assert_eq!(
        again.stderr,
        "error: iglint.toml already exists; use --force to overwrite it\n"
    );
    assert_eq!(
        run(root, &["migrate", "--force"]).status,
        ExitStatus::Success
    );
}

#[test]
fn rule_documentation() {
    let dir = tempfile::tempdir().unwrap();
    let doc = run(dir.path(), &["rule", "unused-variable"]);
    assert_eq!(doc.status, ExitStatus::Success);
    insta::assert_snapshot!("rule_doc", doc.stdout);
    let by_code = run(dir.path(), &["rule", "scr102"]);
    assert_eq!(by_code.stdout, doc.stdout);
    let json = run(dir.path(), &["rule", "--all", "--output-format", "json"]);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert_eq!(value.as_array().unwrap().len(), testing::REGISTRY.len());

    let unknown = run(dir.path(), &["rule", "unused-varible"]);
    assert_eq!(unknown.status, ExitStatus::Error);
    assert_eq!(
        unknown.stderr,
        "error: unknown rule `unused-varible`; did you mean `unused-variable`?\n"
    );
}

#[test]
fn rule_list() {
    let dir = tempfile::tempdir().unwrap();
    let all = run(dir.path(), &["rules"]);
    insta::assert_snapshot!("rules_table", all.stdout);
    let names = run(
        dir.path(),
        &["rules", "--category", "NAME", "--format", "json"],
    );
    let value: serde_json::Value = serde_json::from_str(&names.stdout).unwrap();
    let codes: Vec<_> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, ["NAME001", "NAME002"]);
    let scripts = run(dir.path(), &["rules", "--category", "scripts"]);
    assert!(scripts.stdout.contains("SCR101"));
    let bad = run(dir.path(), &["rules", "--category", "nope"]);
    assert_eq!(bad.status, ExitStatus::Error);
}

#[test]
fn help_version_and_ign_lint_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let version = run(dir.path(), &["--version"]);
    assert_eq!(version.status, ExitStatus::Success);
    assert_eq!(
        version.stdout,
        format!("iglint {}\n", env!("CARGO_PKG_VERSION"))
    );
    let help = run(dir.path(), &["--help"]);
    assert!(help.stdout.contains("Exit status: 0 when"));
    let bad = run(dir.path(), &["check", "--no-such-flag"]);
    assert_eq!(bad.status, ExitStatus::Error);
    assert!(bad.stderr.contains("--no-such-flag"));

    let ign_lint = run(dir.path(), &["ign-lint", "--version"]);
    assert_eq!(ign_lint.status, ExitStatus::Success);
    assert!(
        ign_lint.stdout.starts_with("ign-lint "),
        "{}",
        ign_lint.stdout
    );
}

#[test]
fn colour_and_quiet_modes() {
    let dir = project();
    let coloured = run(dir.path(), &["--color", "always"]);
    assert!(coloured.stdout.contains('\u{1b}'));
    let plain = run(dir.path(), &["--color", "never"]);
    assert!(!plain.stdout.contains('\u{1b}'));
    let quiet = run(dir.path(), &["-q", "--output-format", "concise"]);
    assert!(!quiet.stdout.contains("Found"), "{}", quiet.stdout);
    let silent = run(dir.path(), &["-s"]);
    assert_eq!(silent.stdout, "");
    assert_eq!(silent.status, ExitStatus::Failure);
    let timing = run(dir.path(), &["--timing", "-q"]);
    assert!(
        timing.stderr.contains("timing: analysis"),
        "{}",
        timing.stderr
    );
}

#[test]
fn cache_is_written_to_the_project_root() {
    let dir = project();
    let first = run(dir.path(), &["--output-format", "concise"]);
    assert!(dir.path().join(".iglint_cache").is_dir());
    let tag = std::fs::read_to_string(dir.path().join(".iglint_cache/CACHEDIR.TAG")).unwrap();
    assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55\n"));
    let second = run(dir.path(), &["--output-format", "concise"]);
    assert_eq!(first.stdout, second.stdout);
    let other = run(dir.path(), &["--cache-dir", "elsewhere"]);
    assert_eq!(other.status, ExitStatus::Failure);
    assert!(dir.path().join("elsewhere").is_dir());
}
