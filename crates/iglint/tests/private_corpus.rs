//! Runs every rule on the private corpus named by `IGLINT_PRIVATE_CORPUS`
//! and prints aggregates only (file count, findings per rule, time).
//!
//! `IGLINT_PRIVATE_CORPUS=<path> cargo test --release -p iglint --test private_corpus -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use iglint_linter::engine::{CheckOptions, check};
use iglint_linter::rules::ALL;
use iglint_linter::settings::{LinterSettings, ProjectSettings, RuleSettings};
use iglint_linter::{RuleCode, RuleMeta};

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn all_rules_on_private_corpus() {
    let Some(root) = std::env::var_os("IGLINT_PRIVATE_CORPUS").map(PathBuf::from) else {
        eprintln!("IGLINT_PRIVATE_CORPUS is not set");
        return;
    };
    let rules = ALL
        .iter()
        .map(|meta: &&'static RuleMeta| {
            (
                RuleCode::new(meta),
                RuleSettings {
                    severity: meta.default_severity,
                    options: BTreeMap::new(),
                },
            )
        })
        .collect();
    let settings = ProjectSettings {
        root: root.clone(),
        base: Arc::new(LinterSettings {
            rules,
            strict: false,
        }),
        ..ProjectSettings::default()
    };
    let start = Instant::now();
    let result = check(&settings, &[root], &CheckOptions::default());
    let elapsed = start.elapsed();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &result.diagnostics {
        *counts.entry(d.rule.code()).or_default() += 1;
    }
    println!(
        "files: {}, findings: {}, errors: {}, time: {elapsed:.2?}",
        result.files_checked,
        result.diagnostics.len(),
        result.errors.len()
    );
    for (code, n) in counts {
        println!("{code}: {n}");
    }
    assert!(result.files_checked > 0);
    // Every finding points inside its file.
    for d in &result.diagnostics {
        assert!(d.range.end().to_usize() <= d.file.text().len());
    }
}
