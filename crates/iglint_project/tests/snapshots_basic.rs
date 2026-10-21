//! Snapshot of every resource model in the fixtures.

mod common;

use iglint_project::dump::dump_model;
use iglint_project::{Container, build_model};

fn snapshot_fixture(name: &str) {
    let ws = common::workspace(name);
    let mut containers: Vec<(String, Container)> = ws
        .projects
        .iter()
        .map(|p| (format!("project-{}", p.name), Container::Project(p.id)))
        .chain(ws.collections.iter().map(|c| {
            (
                format!("collection-{}", c.name),
                Container::Collection(c.id),
            )
        }))
        .collect();
    containers.sort_by(|a, b| a.0.cmp(&b.0));
    for (label, container) in containers {
        let mut out = String::new();
        for r in ws.resources.iter().filter(|r| r.container == container) {
            let loaded = ws.load_resource(r.id);
            let model = build_model(&ws, &loaded);
            out.push_str(&dump_model(&ws, &model));
            out.push('\n');
        }
        insta::assert_snapshot!(format!("{name}-{label}"), out);
    }
}

#[test]
fn basic_models() {
    snapshot_fixture("basic");
}

#[test]
fn edge_models() {
    snapshot_fixture("edge");
}
