//! `float.__repr__` as printed by Jython (digits of Java's
//! `Double.toString`), checked against values recorded with the oracle.
//!
//! `tests/oracle/float_repr.txt` holds lines `<literal> <jython repr>`; a
//! larger file can be checked with `IGLINT_FLOAT_SAMPLES=<file> cargo test
//! -p iglint_python --test float_repr -- --ignored`.

use iglint_python::ast::Number;
use iglint_python::repr::number_repr;

fn check(samples: &str) -> (usize, Vec<String>) {
    let mut failures = Vec::new();
    let mut count = 0;
    for line in samples.lines().filter(|line| !line.trim().is_empty()) {
        let Some((literal, expected)) = line.split_once(' ') else {
            continue;
        };
        let value: f64 = literal.parse().expect("literal parses");
        count += 1;
        let actual = number_repr(&Number::Float(value));
        if actual != expected {
            failures.push(format!("{literal}: expected {expected}, got {actual}"));
        }
    }
    (count, failures)
}

#[test]
fn recorded_float_reprs() {
    let (count, failures) = check(include_str!("oracle/float_repr.txt"));
    assert!(count > 1000);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "needs IGLINT_FLOAT_SAMPLES"]
fn sampled_float_reprs() {
    let path = std::env::var("IGLINT_FLOAT_SAMPLES").expect("IGLINT_FLOAT_SAMPLES is set");
    let samples = std::fs::read_to_string(path).expect("samples readable");
    let (count, failures) = check(&samples);
    println!("{count} values, {} mismatches", failures.len());
    for failure in failures.iter().take(20) {
        println!("{failure}");
    }
    assert!(failures.is_empty());
}
