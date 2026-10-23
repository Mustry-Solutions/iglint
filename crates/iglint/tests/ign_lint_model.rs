//! ign-lint's view model against the parity fixtures: every node path that
//! ign-lint reported a node-based finding for must be a node of our model.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use iglint::ign_lint::model::{NodeType, ViewModel, flatten};
use serde_json::Value;

fn fixture_views() -> Vec<PathBuf> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../iglint_linter/tests/fixtures/ign-lint");
    let mut views = Vec::new();
    for group in ["cases", "probes"] {
        for entry in std::fs::read_dir(root.join(group)).unwrap() {
            let view = entry.unwrap().path().join("view.json");
            if view.is_file() {
                views.push(view);
            }
        }
    }
    views.sort();
    views
}

#[test]
fn every_fixture_view_models() {
    let views = fixture_views();
    assert_eq!(views.len(), 24);
    let mut checked = 0;
    for view in views {
        let text = std::fs::read_to_string(&view).unwrap();
        let data = iglint_json::parse(&text).unwrap();
        let model = ViewModel::build(&flatten(&data));
        assert!(
            model.components.iter().any(|c| c.path == "root.root"),
            "{}: root component",
            view.display()
        );
        let paths: BTreeSet<&str> = model.nodes().map(|n| n.path.as_str()).collect();
        let variants: Value = serde_json::from_str(
            &std::fs::read_to_string(view.with_file_name("expected-variants.json")).unwrap(),
        )
        .unwrap();
        for (variant, result) in variants.as_object().unwrap() {
            for finding in result["findings"].as_array().unwrap() {
                let rule = finding["rule"].as_str().unwrap();
                let Some(node_path) = finding["node_path"].as_str() else {
                    continue;
                };
                let node_based = matches!(
                    rule,
                    "NamePatternRule"
                        | "BadComponentReferenceRule"
                        | "ComponentReferenceValidationRule"
                ) || (rule == "PylintScriptRule" && !node_path.contains("::"));
                if !node_based {
                    continue;
                }
                assert!(
                    paths.contains(node_path),
                    "{} [{variant}] {rule}: {node_path} is not a model node",
                    view.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "only {checked} findings checked");
}

#[test]
fn node_types_round_trip() {
    for t in NodeType::ALL {
        assert_eq!(NodeType::parse(t.as_str()), Some(t));
    }
}
