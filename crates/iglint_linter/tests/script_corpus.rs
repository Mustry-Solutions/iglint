//! Runs the Python semantic layer and the script rules over a private
//! Ignition repository. Run with
//! `IGLINT_PRIVATE_CORPUS=/path cargo test -p iglint_linter --release --test script_corpus -- --ignored --nocapture`.
//! Prints aggregate numbers only (counts and timings, never names).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use iglint_linter::engine::{CheckOptions, check};
use iglint_linter::settings::{LinterSettings, ProjectSettings, RuleSettings};
use iglint_project::{Workspace, build_model};
use iglint_python_semantic::{SemanticModel, SemanticOptions, compile_errors};
use rayon::prelude::*;

fn root() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("IGLINT_PRIVATE_CORPUS")?))
}

/// Scripts by kind, scripts, parsed, compile errors, bytes.
type ResourceStats = (BTreeMap<&'static str, usize>, usize, usize, usize, usize);

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn semantic_layer_on_every_script() {
    let Some(root) = root() else {
        eprintln!("IGLINT_PRIVATE_CORPUS not set");
        return;
    };
    let ws = Workspace::discover(&[root.join("projects"), root.join("services")]);
    let start = Instant::now();
    let per_resource: Vec<ResourceStats> = ws
        .resources
        .par_iter()
        .map(|r| {
            let loaded = ws.load_resource(r.id);
            let model = build_model(&ws, &loaded);
            let mut kinds = BTreeMap::new();
            let (mut parsed_ok, mut compile, mut bytes) = (0, 0, 0);
            let mut scripts = 0;
            for (_, script) in model.scripts() {
                scripts += 1;
                *kinds.entry(script.kind.name()).or_default() += 1;
                bytes += script.source.len();
                let parsed = iglint_python::parse_module(&script.source);
                if !parsed.errors.is_empty() {
                    let blank = if script.is_blank() {
                        "blank"
                    } else {
                        "non-blank"
                    };
                    let enabled = if script.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    let key: &'static str = match (blank, enabled) {
                        ("blank", "enabled") => "parse-error blank enabled",
                        ("blank", _) => "parse-error blank disabled",
                        (_, "enabled") => "parse-error non-blank enabled",
                        _ => "parse-error non-blank disabled",
                    };
                    *kinds.entry(key).or_default() += 1;
                    continue;
                }
                parsed_ok += 1;
                let semantic = SemanticModel::build(&parsed.module, &SemanticOptions::default());
                compile += compile_errors(&semantic).len();
            }
            (kinds, scripts, parsed_ok, compile, bytes)
        })
        .collect();
    let elapsed = start.elapsed();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut scripts, mut ok, mut compile, mut bytes) = (0, 0, 0, 0);
    for (k, s, o, c, b) in per_resource {
        for (name, n) in k {
            *kinds.entry(name).or_default() += n;
        }
        scripts += s;
        ok += o;
        compile += c;
        bytes += b;
    }
    eprintln!("scripts: {scripts} ({bytes} bytes), parse ok: {ok}, compile errors: {compile}");
    eprintln!("by kind: {kinds:?}");
    eprintln!("load + model + parse + semantic, parallel: {elapsed:?}");
}

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn script_rules_per_rule_counts() {
    let Some(root) = root() else {
        eprintln!("IGLINT_PRIVATE_CORPUS not set");
        return;
    };
    let mut rules = BTreeMap::new();
    for rule in iglint_linter::rules::REGISTRY.iter() {
        if rule.code().starts_with("SCR") {
            rules.insert(
                rule,
                RuleSettings {
                    severity: rule.meta().default_severity,
                    options: BTreeMap::new(),
                },
            );
        }
    }
    let settings = ProjectSettings {
        root: root.clone(),
        base: std::sync::Arc::new(LinterSettings {
            rules,
            strict: false,
        }),
        respect_gitignore: false,
        ..ProjectSettings::default()
    };
    let start = Instant::now();
    let result = check(
        &settings,
        &[root.join("projects"), root.join("services")],
        &CheckOptions::default(),
    );
    let elapsed = start.elapsed();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &result.diagnostics {
        *counts.entry(d.rule.code()).or_default() += 1;
    }
    eprintln!("findings per rule: {counts:?}");
    eprintln!("files: {}, check: {elapsed:?}", result.files_checked);
}
