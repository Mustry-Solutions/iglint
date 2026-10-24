//! Differential test of the reads of a module's globals, the
//! interpreter's implicit ones included, against IA's Jython.
//!
//! `tests/oracle/implicit_reads.cases` holds small sources; the expected
//! file records which names running each looks up in its module namespace
//! (recorded with `oracle/run-oracle.sh --reads`, a namespace that logs
//! every lookup). Every name Jython looks up must be a reference of the
//! model that reaches the module's globals: module code, a function's
//! global or unresolved names, a class body's names (with the fallback
//! past the class namespace), and the implicit reads
//! ([`ReferenceKind::Implicit`]). The model may list more (code that does
//! not run, and a class body's flow-insensitive fallback), never fewer.

use std::collections::BTreeSet;

use iglint_python_semantic::{ReferenceKind, Resolution, ScopeId, SemanticModel, SemanticOptions};

fn records(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    let mut body = String::new();
    for line in text.split_inclusive('\n') {
        if let Some(n) = line.strip_prefix("### ") {
            if let Some(prev) = name.take() {
                out.push((prev, std::mem::take(&mut body)));
            }
            name = Some(n.trim_end().to_owned());
        } else {
            body.push_str(line);
        }
    }
    if let Some(n) = name {
        out.push((n, body));
    }
    out
}

/// The names the model says running `source` may look up in the module's
/// globals.
fn model_reads(source: &str) -> BTreeSet<String> {
    let parsed = iglint_python::parse_module(source);
    assert!(parsed.errors.is_empty(), "{source}");
    let options = SemanticOptions {
        module_has_name: true,
        ..SemanticOptions::default()
    };
    let model = SemanticModel::build(&parsed.module, &options);
    model
        .references()
        .filter(|(_, r)| r.kind != ReferenceKind::Del)
        .filter(|(_, r)| {
            r.scope == ScopeId::MODULE
                || r.fallback.is_some()
                || matches!(
                    r.resolution,
                    Resolution::Global
                        | Resolution::Builtin
                        | Resolution::Injected
                        | Resolution::Unresolved
                        | Resolution::Dynamic
                )
        })
        .map(|(_, r)| r.name.to_owned())
        .collect()
}

#[test]
fn implicit_reads_match_jython() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let sources = records(&std::fs::read_to_string(format!("{dir}/implicit_reads.cases")).unwrap());
    let expected =
        records(&std::fs::read_to_string(format!("{dir}/implicit_reads.expected")).unwrap());
    assert_eq!(sources.len(), expected.len());
    let mut failures = Vec::new();
    let mut exact = 0;
    for ((name, source), (expected_name, outcome)) in sources.iter().zip(&expected) {
        assert_eq!(name, expected_name);
        let line = outcome.trim_end();
        let reads = line.strip_prefix("reads").expect(line);
        let jython: BTreeSet<String> = reads
            .split_whitespace()
            .filter(|n| !n.starts_with('!') && !n.starts_with("_["))
            .map(str::to_owned)
            .collect();
        let ours = model_reads(source);
        let missed: Vec<&String> = jython.difference(&ours).collect();
        if !missed.is_empty() {
            failures.push(format!(
                "{name}: Jython reads {missed:?}, the model does not"
            ));
        }
        exact += usize::from(jython == ours);
    }
    eprintln!("{} cases, {exact} exact", sources.len());
    assert!(sources.len() >= 30);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
