//! Differential test of compile-time warnings against IA's Jython.
//!
//! `tests/oracle/warnings.cases` (written for the warnings) and
//! `compile.cases` (the compile-error cases) hold small sources;
//! `warnings.expected` and `compile.warnings.expected` are the
//! `SyntaxWarning`s Jython's full `compile()` issues for each (recorded
//! with `oracle/run-oracle.sh --warnings`). For every case that parses and
//! compiles, [`compile_warnings`] must report exactly Jython's warnings:
//! the same messages on the same lines.

use iglint_python_semantic::{SemanticModel, SemanticOptions, compile_warnings};

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

/// `name -> (warnings as (line, message), compiled)`.
type Expected = Vec<(String, Vec<(usize, String)>, bool)>;

fn expected(text: &str) -> Expected {
    let mut out: Expected = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("### ") {
            out.push((name.to_owned(), Vec::new(), false));
        } else if let Some(rest) = line.strip_prefix("warning ") {
            let (lineno, message) = rest.split_once(' ').unwrap();
            let last = out.last_mut().unwrap();
            last.1.push((lineno.parse().unwrap(), message.to_owned()));
        } else {
            out.last_mut().unwrap().2 = line == "ok";
        }
    }
    out
}

fn check(cases_file: &str, expected_file: &str) -> usize {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/oracle");
    let sources = cases(&std::fs::read_to_string(format!("{dir}/{cases_file}")).unwrap());
    let expected = expected(&std::fs::read_to_string(format!("{dir}/{expected_file}")).unwrap());
    assert_eq!(sources.len(), expected.len());
    let mut failures = Vec::new();
    let mut checked = 0;
    for ((name, source), (expected_name, jython, compiled)) in sources.iter().zip(&expected) {
        assert_eq!(name, expected_name);
        let parsed = iglint_python::parse_module(source);
        if !parsed.errors.is_empty() || !compiled {
            continue;
        }
        checked += 1;
        let model = SemanticModel::build(&parsed.module, &SemanticOptions::default());
        let mut ours: Vec<(usize, String)> = compile_warnings(&model)
            .into_iter()
            .map(|w| {
                let line = source[..w.range.start().to_usize()].matches('\n').count() + 1;
                (line, w.message)
            })
            .collect();
        ours.sort();
        let mut theirs = jython.clone();
        theirs.sort();
        if ours != theirs {
            failures.push(format!("{name}: Jython {theirs:?}, we {ours:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    checked
}

#[test]
fn compile_warnings_match_jython() {
    assert!(check("warnings.cases", "warnings.expected") > 50);
    assert!(check("compile.cases", "compile.warnings.expected") > 200);
}
