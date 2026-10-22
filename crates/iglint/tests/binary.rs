//! Tests of the installed binary: process exit codes, argv[0] dispatch.

use assert_cmd::Command;
use predicates::prelude::*;

fn iglint(dir: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("iglint").unwrap();
    command
        .current_dir(dir)
        .env_remove("CLICOLOR_FORCE")
        .env("NO_COLOR", "1");
    command
}

#[test]
fn clean_project_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("views/A")).unwrap();
    std::fs::write(dir.path().join("views/A/view.json"), "{}").unwrap();
    iglint(dir.path())
        .arg("--no-cache")
        .assert()
        .success()
        .stdout("All checks passed!\n");
    iglint(dir.path())
        .args(["check", "--show-files"])
        .assert()
        .success()
        .stdout("views/A/view.json\n");
}

#[test]
fn usage_and_configuration_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    iglint(dir.path())
        .args(["--select", "NOPE"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown rule selector `NOPE`"));
    iglint(dir.path()).arg("--bogus").assert().code(2);
    std::fs::write(dir.path().join("iglint.toml"), "selct = []\n").unwrap();
    iglint(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "iglint.toml:1:1: unknown key `selct`; did you mean `select`?",
        ));
}

#[test]
fn help_and_version() {
    let dir = tempfile::tempdir().unwrap();
    iglint(dir.path())
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("iglint {}\n", env!("CARGO_PKG_VERSION")));
    iglint(dir.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ign-lint"));
    iglint(dir.path()).arg("rules").assert().success();
}

#[cfg(unix)]
#[test]
fn ign_lint_symlink_dispatches_to_compat_mode() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("ign-lint");
    std::os::unix::fs::symlink(assert_cmd::cargo::cargo_bin("iglint"), &link).unwrap();
    std::process::Command::new(&link)
        .arg("--version")
        .current_dir(dir.path())
        .output()
        .map(|output| {
            assert!(output.status.success());
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.starts_with("ign-lint "), "{stdout}");
        })
        .unwrap();
    // argparse-style usage errors exit 2.
    let output = std::process::Command::new(&link)
        .arg("--no-such-option")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// `iglint server` speaks LSP over the standard streams: the whole session
/// (initialize, shutdown, exit) arrives at once and every answer comes back.
#[test]
fn server_speaks_lsp_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{body}", body.len());
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ]
    .map(frame)
    .concat();
    iglint(dir.path())
        .arg("server")
        .write_stdin(input)
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .success()
        .stdout(
            predicate::str::contains(r#""serverInfo":{"name":"iglint""#)
                .and(predicate::str::contains(r#""id":2,"result":null"#)),
        );
}
