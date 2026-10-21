#![allow(dead_code)]

use std::path::PathBuf;

use iglint_project::Workspace;

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn workspace(name: &str) -> Workspace {
    let root = fixture(name);
    Workspace::discover(&[root.join("projects"), root.join("services")])
}
