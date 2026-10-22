//! Sets `IGLINT_VERSION` for `iglint --version`: the package version, plus
//! the commit and its date when built from a git checkout, e.g.
//! `0.1.0 (1a2b3c4d5 2026-10-22)`.
//!
//! `IGLINT_COMMIT_HASH` (and optionally `IGLINT_COMMIT_DATE`) override git,
//! for builds without a usable checkout (containers, source archives).

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(std::env::var_os("CARGO_MANIFEST_DIR")?)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

/// Reruns the script when the checked-out commit changes.
fn watch_git_head() {
    for path in ["HEAD", "packed-refs"] {
        if let Some(file) = git(&["rev-parse", "--path-format=absolute", "--git-path", path]) {
            println!("cargo:rerun-if-changed={file}");
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(file) = git(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &reference,
        ])
        && Path::new(&file).exists()
    {
        println!("cargo:rerun-if-changed={file}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=IGLINT_COMMIT_HASH");
    println!("cargo:rerun-if-env-changed=IGLINT_COMMIT_DATE");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let from_env = std::env::var("IGLINT_COMMIT_HASH")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|hash| {
            let short: String = hash.chars().take(9).collect();
            (short, std::env::var("IGLINT_COMMIT_DATE").ok())
        });
    let commit = from_env.or_else(|| {
        // Only our own checkout: a source archive unpacked inside another
        // repository must not report that repository's commit.
        watch_git_head();
        git(&["ls-files", "--error-unmatch", "build.rs"])?;
        let hash = git(&["rev-parse", "--short=9", "HEAD"])?;
        Some((hash, git(&["log", "-1", "--format=%cs", "HEAD"])))
    });
    let long = match commit {
        Some((hash, Some(date))) if !date.is_empty() => format!("{version} ({hash} {date})"),
        Some((hash, _)) => format!("{version} ({hash})"),
        None => version,
    };
    println!("cargo:rustc-env=IGLINT_VERSION={long}");
}
