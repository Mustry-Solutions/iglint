//! Differential tests against recorded output of IA's Jython 2.7.3.
//!
//! Every `tests/oracle/<name>.cases` file holds small sources (see
//! `support/cases.rs`); `<name>.expected` holds what Jython's
//! `ast.dump(tree)` and `ast.dump(tree, include_attributes=True)` print for
//! each of them, or the line and column of the `SyntaxError` it raises.
//! Regenerate the expectations with `oracle/record-expected.sh`.
//!
//! The `#[ignore]`d tests run the same comparison on larger corpora that are
//! not part of the repository; see their documentation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod support;

use support::cases::{ast_record, one_line, split_cases};

/// One record of an oracle output file.
#[derive(Debug, PartialEq, Eq)]
enum Record {
    Ok { dump: String, dump_attrs: String },
    Error { line: i32, column: i32 },
}

fn parse_records(text: &str) -> Vec<(String, Record)> {
    let mut records = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(name) = line.strip_prefix("### ") else {
            continue;
        };
        let status = lines.next().unwrap_or_default();
        let record = if status == "ok" {
            let dump = lines.next().unwrap_or_default().to_owned();
            let dump_attrs = lines.next().unwrap_or_default().to_owned();
            Record::Ok { dump, dump_attrs }
        } else {
            let mut parts = status.split(' ').skip(1);
            let line = parts
                .next()
                .and_then(|part| part.parse().ok())
                .unwrap_or(-99);
            let column = parts
                .next()
                .and_then(|part| part.parse().ok())
                .unwrap_or(-99);
            // The message line.
            lines.next();
            Record::Error { line, column }
        };
        records.push((name.to_owned(), record));
    }
    records
}

/// Compares one source with Jython's record; returns a description of the
/// difference, if any.
fn compare(source: &[u8], expected: &Record) -> Option<String> {
    let ours = ast_record(source);
    let actual = parse_records(&format!("### x\n{ours}"))
        .pop()
        .map(|(_, record)| record)?;
    if &actual == expected {
        return None;
    }
    Some(match (expected, &actual) {
        (Record::Ok { dump_attrs: a, .. }, Record::Ok { dump_attrs: b, .. }) => {
            let at = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
            let from = at.saturating_sub(80);
            format!(
                "dump differs at {at}:\n  jython: ...{}\n  iglint: ...{}",
                &a[from..(at + 80).min(a.len())],
                &b[from..(at + 80).min(b.len())]
            )
        }
        _ => format!("jython: {expected:?}\n  iglint: {}", ours.trim_end()),
    })
}

fn oracle_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle")
}

#[test]
fn hand_written_cases_match_jython() {
    let mut total = 0;
    let mut failures = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(oracle_dir())
        .expect("oracle directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "cases"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for path in files {
        let data = std::fs::read(&path).expect("cases readable");
        let expected_path = path.with_extension("expected");
        let expected_text = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
        let expected: BTreeMap<String, Record> =
            parse_records(&expected_text).into_iter().collect();
        for case in split_cases(&data) {
            total += 1;
            let key = one_line(&case.name);
            let Some(record) = expected.get(&key) else {
                failures.push(format!(
                    "{}: {}: no expectation recorded",
                    path.display(),
                    case.name
                ));
                continue;
            };
            if let Some(difference) = compare(&case.source, record) {
                failures.push(format!("{}: {}: {difference}", path.display(), case.name));
            }
        }
    }
    println!("{total} cases");
    assert!(
        failures.is_empty(),
        "{} of {total} cases differ from Jython:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Compares every source listed in an oracle output file (produced by
/// `oracle/run-oracle.sh ROOT LIST OUT`) with the files under `ROOT`.
///
/// `IGLINT_ORACLE_ROOT` and `IGLINT_ORACLE_RESULTS` select the corpus, for
/// example the standard library extracted by `oracle/extract-stdlib.sh`.
#[test]
#[ignore = "needs IGLINT_ORACLE_ROOT and IGLINT_ORACLE_RESULTS"]
fn corpus_matches_jython() {
    let root = PathBuf::from(std::env::var("IGLINT_ORACLE_ROOT").expect("IGLINT_ORACLE_ROOT"));
    let results = std::env::var("IGLINT_ORACLE_RESULTS").expect("IGLINT_ORACLE_RESULTS");
    let text = std::fs::read_to_string(results).expect("results readable");
    let mut total = 0;
    let mut valid = 0;
    let mut failures = Vec::new();
    for (name, record) in parse_records(&text) {
        let data = std::fs::read(root.join(&name)).expect("source readable");
        total += 1;
        if matches!(record, Record::Ok { .. }) {
            valid += 1;
        }
        if let Some(difference) = compare(&data, &record) {
            failures.push(format!("{name}: {difference}"));
        }
    }
    println!(
        "{total} files ({valid} accepted by Jython), {} differences",
        failures.len()
    );
    for failure in failures.iter().take(30) {
        println!("{failure}");
    }
    assert!(failures.is_empty());
}
