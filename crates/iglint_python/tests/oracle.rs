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
                String::from_utf8_lossy(&a.as_bytes()[from..(at + 80).min(a.len())]),
                String::from_utf8_lossy(&b.as_bytes()[from..(at + 80).min(b.len())])
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
        let (count, file_failures) = compare_case_file(&path, &path.with_extension("expected"));
        total += count;
        failures.extend(file_failures);
    }
    println!("{total} cases");
    assert!(
        failures.is_empty(),
        "{} of {total} cases differ from Jython:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Compares every case of a case file with Jython's records for it.
fn compare_case_file(cases_path: &Path, expected_path: &Path) -> (usize, Vec<String>) {
    let data = std::fs::read(cases_path).expect("cases readable");
    let expected_text = std::fs::read_to_string(expected_path)
        .unwrap_or_else(|_| panic!("missing {}", expected_path.display()));
    let expected = parse_records(&expected_text);
    let cases = split_cases(&data);
    assert_eq!(
        cases.len(),
        expected.len(),
        "{} is out of date; run oracle/record-expected.sh",
        expected_path.display()
    );
    let mut failures = Vec::new();
    for (case, (name, record)) in cases.iter().zip(&expected) {
        assert_eq!(&one_line(&case.name), name, "{}", expected_path.display());
        if let Some(difference) = compare(&case.source, record) {
            failures.push(format!(
                "{}: {}: {:?}\n  {difference}",
                cases_path.display(),
                case.name,
                String::from_utf8_lossy(&case.source)
            ));
        }
    }
    (cases.len(), failures)
}

/// Compares generated fuzz cases (`oracle/fuzz.sh`) with Jython's records.
#[test]
#[ignore = "needs IGLINT_FUZZ_CASES and IGLINT_FUZZ_EXPECTED"]
fn fuzz_cases_match_jython() {
    let cases = PathBuf::from(std::env::var("IGLINT_FUZZ_CASES").expect("IGLINT_FUZZ_CASES"));
    let expected =
        PathBuf::from(std::env::var("IGLINT_FUZZ_EXPECTED").expect("IGLINT_FUZZ_EXPECTED"));
    let (total, failures) = compare_case_file(&cases, &expected);
    println!("{total} fuzz cases, {} differences", failures.len());
    for failure in failures.iter().take(25) {
        println!("{failure}");
    }
    assert!(failures.is_empty());
}

/// Compares every source listed in an oracle output file (produced by
/// `oracle/run-oracle.sh ROOT LIST OUT`) with the files under `ROOT`.
///
/// `IGLINT_ORACLE_ROOT` and `IGLINT_ORACLE_RESULTS` select the corpus, for
/// example the standard library extracted by `oracle/extract-stdlib.sh`.
#[test]
#[ignore = "needs IGLINT_ORACLE_ROOT and IGLINT_ORACLE_RESULTS"]
fn corpus_matches_jython() {
    let summary = compare_corpus();
    println!(
        "{} files ({} accepted by Jython), {} differences",
        summary.total,
        summary.valid,
        summary.failures.len()
    );
    for failure in summary.failures.iter().take(30) {
        println!("{failure}");
    }
    assert!(summary.failures.is_empty());
}

/// The same comparison for a private corpus (`oracle/private-corpus.sh`):
/// prints aggregate numbers only, never names or contents.
#[test]
#[ignore = "needs IGLINT_ORACLE_ROOT and IGLINT_ORACLE_RESULTS of a private corpus"]
fn private_corpus_matches_jython() {
    let summary = compare_corpus();
    println!(
        "{} sources ({} accepted by Jython, {} bytes), {} differences",
        summary.total,
        summary.valid,
        summary.bytes,
        summary.failures.len()
    );
    assert!(summary.failures.is_empty());
}

struct CorpusSummary {
    total: usize,
    valid: usize,
    bytes: usize,
    failures: Vec<String>,
}

fn compare_corpus() -> CorpusSummary {
    let root = PathBuf::from(std::env::var("IGLINT_ORACLE_ROOT").expect("IGLINT_ORACLE_ROOT"));
    let results = std::env::var("IGLINT_ORACLE_RESULTS").expect("IGLINT_ORACLE_RESULTS");
    let text = std::fs::read_to_string(results).expect("results readable");
    let mut summary = CorpusSummary {
        total: 0,
        valid: 0,
        bytes: 0,
        failures: Vec::new(),
    };
    for (name, record) in parse_records(&text) {
        let data = std::fs::read(root.join(&name)).expect("source readable");
        summary.total += 1;
        summary.bytes += data.len();
        if matches!(record, Record::Ok { .. }) {
            summary.valid += 1;
        }
        if let Some(difference) = compare(&data, &record) {
            summary.failures.push(format!("{name}: {difference}"));
        }
    }
    summary
}
