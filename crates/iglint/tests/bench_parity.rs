//! Runs `bench/parity.py` on the bundled ign-lint fixtures and requires
//! every difference, of findings and of severities, to be explained by a
//! documented category.
//!
//! Needs `python3` and an ign-lint 0.6.4 virtualenv: `$IGN_LINT`, else
//! `$BENCH_HOME/venv/bin/ign-lint` (default `~/.cache/iglint-bench`), as
//! `bench/parity.py` finds it (bench/README.md). Skipped when either is
//! missing.
//!
//! `cargo test -p iglint --test bench_parity -- --nocapture`

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn ign_lint() -> Option<PathBuf> {
    let path = match std::env::var_os("IGN_LINT") {
        Some(p) => PathBuf::from(p),
        None => std::env::var_os("BENCH_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache/iglint-bench"))
            })?
            .join("venv/bin/ign-lint"),
    };
    path.is_file().then_some(path)
}

#[test]
fn parity_on_fixtures_has_no_unexplained_differences() {
    let Some(ign_lint) = ign_lint() else {
        eprintln!("ign-lint virtualenv not found: skipped");
        return;
    };
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tempfile::tempdir().unwrap();
    let config = repo.join("bench/configs/rules-only.json");
    let output = match Command::new("python3")
        .arg(repo.join("bench/parity.py"))
        .arg(repo.join("crates/iglint_linter/tests/fixtures/ign-lint"))
        .arg("--out")
        .arg(out.path())
        .arg("--list")
        .arg("--ign-lint")
        .arg(&ign_lint)
        .arg("--iglint")
        .arg(env!("CARGO_BIN_EXE_iglint"))
        .arg("--")
        .arg("--config")
        .arg(&config)
        .args(["--files", "cases/**/view.json"])
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            eprintln!("python3 not found: skipped");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "parity.py failed:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: Value =
        serde_json::from_str(&std::fs::read_to_string(out.path().join("summary.json")).unwrap())
            .unwrap();
    assert!(summary["identical"].as_u64().unwrap() > 0, "{stdout}");
    assert_eq!(summary["unexplained"]["ign-lint-only"], 0, "{stdout}");
    assert_eq!(summary["unexplained"]["iglint-only"], 0, "{stdout}");
    assert!(
        summary["severity"]["matched"].as_u64().unwrap() > 0,
        "{stdout}"
    );
    assert_eq!(summary["severity"]["unexplained"], 0, "{stdout}");
}
