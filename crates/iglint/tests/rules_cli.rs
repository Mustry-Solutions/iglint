//! `iglint check` and `iglint ign-lint` with the real rules (no fake
//! analyzer), on hand-written projects and on the ign-lint fixtures.

use std::path::Path;

use iglint::{Environment, ExitStatus, Io};
use iglint_linter::rules::REGISTRY;

struct Run {
    status: ExitStatus,
    stdout: String,
}

fn environment(cwd: &Path) -> Environment {
    Environment {
        registry: &REGISTRY,
        analyzer: None,
        cwd: cwd.to_path_buf(),
        vars: Vec::new(),
    }
}

fn run_with(cwd: &Path, args: &[&str], ign_lint: bool) -> Run {
    let environment = environment(cwd);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin: &[u8] = &[];
    let mut io = Io {
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin: &mut stdin,
        stdout_is_terminal: false,
    };
    let mut argv = vec![std::ffi::OsString::from(if ign_lint {
        "ign-lint"
    } else {
        "iglint"
    })];
    argv.extend(args.iter().map(std::ffi::OsString::from));
    let status = if ign_lint {
        iglint::ign_lint::run(argv, &environment, &mut io)
    } else {
        iglint::run(argv, &environment, &mut io)
    };
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(!stderr.contains("panicked"), "{stderr}");
    Run {
        status,
        stdout: String::from_utf8(stdout).unwrap(),
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const MANIFEST: &str = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;

/// A project with one view that references a missing view, a missing
/// property and has an unused custom property.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(
        d,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    let views = "projects/p/com.inductiveautomation.perspective/views";
    write(d, &format!("{views}/Main/resource.json"), MANIFEST);
    write(
        d,
        &format!("{views}/Main/view.json"),
        r#"{
  "custom": {
    "unused": 1
  },
  "root": {
    "children": [
      {
        "meta": {
          "name": "Embed"
        },
        "props": {
          "path": "Missing/View"
        },
        "type": "ia.display.view"
      },
      {
        "meta": {
          "name": "Label"
        },
        "propConfig": {
          "props.text": {
            "binding": {
              "config": {
                "expression": "{view.params.nope}"
              },
              "type": "expr"
            }
          }
        },
        "type": "ia.display.label"
      }
    ],
    "meta": {
      "name": "root"
    },
    "type": "ia.container.flex"
  }
}"#,
    );
    dir
}

#[test]
fn check_reports_real_findings() {
    let dir = project();
    let run = run_with(
        dir.path(),
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--output-format",
            "concise",
        ],
        false,
    );
    assert_eq!(run.status, ExitStatus::Failure, "{}", run.stdout);
    for code in ["REF001", "BIND005", "VIEW015"] {
        assert!(run.stdout.contains(code), "{code} missing:\n{}", run.stdout);
    }
    // Every finding points into the view file with a position.
    for line in run.stdout.lines().filter(|l| l.contains("view.json:")) {
        assert!(
            line.split(':')
                .nth(1)
                .is_some_and(|n| n.parse::<u32>().is_ok()),
            "{line}"
        );
    }
}

#[test]
fn check_fix_removes_unused_custom_property() {
    let dir = project();
    let run = run_with(
        dir.path(),
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--select",
            "VIEW015",
            "--fix",
        ],
        false,
    );
    assert_eq!(run.status, ExitStatus::Success, "{}", run.stdout);
    let text = std::fs::read_to_string(
        dir.path()
            .join("projects/p/com.inductiveautomation.perspective/views/Main/view.json"),
    )
    .unwrap();
    assert!(!text.contains("unused"), "{text}");
    assert!(
        text.starts_with("{\n  \"custom\": {},\n  \"root\": {"),
        "{text}"
    );
    // Re-running finds nothing.
    let again = run_with(
        dir.path(),
        &["check", "--isolated", "--no-cache", "--select", "VIEW015"],
        false,
    );
    assert_eq!(again.status, ExitStatus::Success, "{}", again.stdout);
}

#[test]
fn ign_lint_mode_prints_ign_lint_paths_and_wording() {
    let dir = tempfile::tempdir().unwrap();
    let case = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iglint_linter/tests/fixtures/ign-lint/cases/PascalCase/view.json");
    write(
        dir.path(),
        "view.json",
        &std::fs::read_to_string(case).unwrap(),
    );
    write(dir.path(), "rule_config.json", "{}");
    let run = run_with(dir.path(), &["view.json"], true);
    assert_eq!(run.status, ExitStatus::Failure);
    assert!(
        run.stdout.contains(
            "custom.CustomViewParam: custom property 'CustomViewParam' is defined but never referenced"
        ),
        "{}",
        run.stdout
    );
}
