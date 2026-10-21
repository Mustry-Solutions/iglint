//! Stamps the build with `IGLINT_BUILD_ID`: the git commit (with `-dirty`
//! when tracked files have changes) and the compile time, so that each
//! rebuild gets a new id and invalidates the result cache.
//!
//! No `rerun-if-changed` lines are emitted on purpose: without them cargo
//! reruns this script whenever any file of the package changes, which is
//! exactly when the rules may have changed.

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(std::env::var_os("CARGO_MANIFEST_DIR")?)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let id = match git(&["rev-parse", "--short=12", "HEAD"]) {
        Some(commit) if !commit.is_empty() => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_none_or(|status| !status.is_empty());
            let marker = if dirty { "-dirty" } else { "" };
            format!("{commit}{marker}.{timestamp}")
        }
        _ => timestamp.to_string(),
    };
    println!("cargo:rustc-env=IGLINT_BUILD_ID={id}");
}
