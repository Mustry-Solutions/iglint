//! Differential test of compile-time errors against IA's Jython.
//!
//! `tests/oracle/compile.cases` holds small sources; `compile.expected` is
//! what Jython's full `compile()` makes of each (recorded with
//! `oracle/run-oracle.sh --compile`). For every case the parser and
//! [`compile_errors`] together must agree with Jython: no error where
//! Jython compiles, and Jython's error (message and position) where it
//! does not. Jython's internal compiler failures (reported without a
//! position) must be errors whose kind is an internal crash
//! ([`CompileErrorKind::is_internal_crash`]).

use iglint_python::Locator;
use iglint_python_semantic::{SemanticModel, SemanticOptions, compile_errors};

fn cases(text: &str) -> Vec<(String, String)> {
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

/// Jython's error: line, column, message.
type JythonError = (i32, i32, String);

/// `name -> None` (ok) or `Some(error)`.
fn expected(text: &str) -> Vec<(String, Option<JythonError>)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(name) = lines[i].strip_prefix("### ") else {
            i += 1;
            continue;
        };
        if lines[i + 1] == "ok" {
            out.push((name.to_owned(), None));
            i += 2;
        } else {
            let mut parts = lines[i + 1].split(' ');
            parts.next();
            let line: i32 = parts.next().unwrap().parse().unwrap();
            let column: i32 = parts.next().unwrap().parse().unwrap();
            out.push((
                name.to_owned(),
                Some((line, column, lines[i + 2].to_owned())),
            ));
            i += 3;
        }
    }
    out
}

/// Compares every case of a cases file with Jython's recorded outcome;
/// returns how many cases the semantic layer checked and the failures.
fn compare(cases_text: &str, expected_text: &str) -> (usize, Vec<String>) {
    let sources = cases(cases_text);
    let expected = expected(expected_text);
    assert_eq!(sources.len(), expected.len());
    let mut failures = Vec::new();
    let mut checked = 0;
    for ((name, source), (expected_name, outcome)) in sources.iter().zip(&expected) {
        assert_eq!(name, expected_name);
        let parsed = iglint_python::parse_module(source);
        if !parsed.errors.is_empty() {
            // Parse errors are the parser's business (its own oracle tests).
            continue;
        }
        checked += 1;
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        let errors = compile_errors(&model);
        let locator = Locator::new(source);
        match outcome {
            None => {
                if !errors.is_empty() {
                    failures.push(format!("{name}: Jython compiles it, we report {errors:?}"));
                }
            }
            Some((0, 0, message)) if message.starts_with("internal error") => {
                if !errors.iter().any(|e| e.kind.is_internal_crash()) {
                    failures.push(format!(
                        "{name}: Jython fails internally, we report {errors:?}"
                    ));
                }
            }
            Some((line, column, message)) => {
                let found = errors.iter().any(|e| {
                    let at = e.anchor.map(|a| locator.anchor_location(a));
                    e.message == *message
                        && at.is_some_and(|at| at.line == *line && at.column == *column)
                });
                if !found {
                    let ours: Vec<_> = errors
                        .iter()
                        .map(|e| {
                            (
                                e.anchor.map(|a| locator.anchor_location(a)),
                                e.message.clone(),
                            )
                        })
                        .collect();
                    failures.push(format!(
                        "{name}: Jython reports {line}:{column} {message}, we report {ours:?}"
                    ));
                }
            }
        }
    }
    (checked, failures)
}

#[test]
fn compile_errors_match_jython() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let read = |name: &str| std::fs::read_to_string(format!("{dir}/{name}")).unwrap();
    let (checked, failures) = compare(&read("compile.cases"), &read("compile.expected"));
    eprintln!("{checked} cases compiled by the semantic layer");
    assert!(checked > 300, "only {checked} cases parsed");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A stratified sample of `oracle/compile_cross.py`: every statement that
/// ends a block or makes code unreachable, before every construct Jython
/// rejects, in every nesting context (review round 13, M2).
#[test]
fn compile_errors_after_control_flow_match_jython() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let read = |name: &str| std::fs::read_to_string(format!("{dir}/{name}")).unwrap();
    let (checked, failures) = compare(
        &read("compile_cross.cases"),
        &read("compile_cross.expected"),
    );
    eprintln!("{checked} control flow cases");
    assert!(checked > 500, "only {checked} cases parsed");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The full cross product: `IGLINT_COMPILE_CASES` and
/// `IGLINT_COMPILE_EXPECTED` name the files (`oracle/compile_cross.py all`,
/// then `oracle/run-oracle.sh --compile`).
#[test]
#[ignore]
fn compile_errors_match_jython_full_run() {
    let cases = std::env::var("IGLINT_COMPILE_CASES").expect("IGLINT_COMPILE_CASES");
    let expected = std::env::var("IGLINT_COMPILE_EXPECTED").expect("IGLINT_COMPILE_EXPECTED");
    let (checked, failures) = compare(
        &std::fs::read_to_string(cases).unwrap(),
        &std::fs::read_to_string(expected).unwrap(),
    );
    eprintln!("{checked} cases checked, {} differences", failures.len());
    for f in failures.iter().take(40) {
        eprintln!("{f}");
    }
    assert!(failures.is_empty(), "{} differences", failures.len());
}
