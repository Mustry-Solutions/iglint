//! After a syntax error the parser keeps going, so that the rest of the file
//! can be analysed; the first error is the one Jython reports.

use iglint_python::{Locator, Stmt, parse_module};

/// The names assigned by the top-level statements.
fn assigned_names(source: &str) -> Vec<String> {
    let parsed = parse_module(source);
    let mut names = Vec::new();
    for stmt in &parsed.module.body {
        collect(stmt, &mut names);
    }
    names
}

fn collect(stmt: &Stmt, names: &mut Vec<String>) {
    match stmt {
        Stmt::Assign(assign) => {
            for target in &assign.targets {
                if let Some(name) = target.as_name() {
                    names.push(name.id.to_string());
                }
            }
        }
        Stmt::If(node) => node.body.iter().for_each(|stmt| collect(stmt, names)),
        Stmt::FunctionDef(node) => {
            names.push(format!("def {}", node.name.id));
            node.body.iter().for_each(|stmt| collect(stmt, names));
        }
        _ => {}
    }
}

fn first_error_line(source: &str) -> i32 {
    let parsed = parse_module(source);
    let locator = Locator::new(source);
    parsed.errors[0].jython_location(&locator).line
}

#[test]
fn statements_after_a_broken_line_are_kept() {
    let source = "a = 1\nb = 1 +\nc = 3\n";
    assert_eq!(first_error_line(source), 2);
    assert_eq!(assigned_names(source), ["a", "c"]);
}

#[test]
fn block_of_a_broken_header_is_kept() {
    let source = "if x\n    a = 1\nb = 2\n";
    assert_eq!(first_error_line(source), 1);
    assert_eq!(assigned_names(source), ["a", "b"]);
}

#[test]
fn unclosed_bracket_ends_at_a_statement_keyword() {
    // Jython joins all lines inside the brackets and fails at `def`; the
    // parser reports that and recovers from the brackets there.
    let source = "x = foo(1,\ndef g():\n    a = 1\nb = 2\n";
    assert_eq!(first_error_line(source), 2);
    assert_eq!(assigned_names(source), ["def g", "a", "b"]);
}

#[test]
fn lexical_errors_do_not_stop_the_parser() {
    let source = "a = $\nb = 2\n";
    assert_eq!(first_error_line(source), 1);
    assert_eq!(assigned_names(source), ["b"]);
}

#[test]
fn unexpected_indent_keeps_the_indented_statements() {
    let source = "a = 1\n    b = 2\nc = 3\n";
    assert_eq!(first_error_line(source), 2);
    assert_eq!(assigned_names(source), ["a", "b", "c"]);
}

#[test]
fn errors_inside_a_block_keep_the_rest_of_the_block() {
    let source = "def f():\n    a = (\n    b = 2\n\ndef g():\n    c = 3\n";
    let names = assigned_names(source);
    assert!(names.contains(&"def g".to_owned()), "{names:?}");
    assert!(names.contains(&"c".to_owned()), "{names:?}");
}

#[test]
fn semantic_errors_keep_the_statement() {
    // Jython rejects the assignment, but the tree still has it.
    let source = "None = 1\nf() = 2\nx = 3\n";
    let parsed = parse_module(source);
    assert_eq!(parsed.errors.len(), 2);
    assert_eq!(parsed.module.body.len(), 3);
}

#[test]
fn dropped_text_is_reported() {
    // Jython silently drops the unterminated string and the newline.
    let source = "x = 'abc\ny = 1\n";
    let parsed = parse_module(source);
    assert!(parsed.errors.is_empty());
    assert_eq!(parsed.dropped.len(), 1);
    assert_eq!(parsed.dropped[0].range.slice(source), "'abc\n");
    // `x = y = 1`
    assert_eq!(assigned_names(source), ["x", "y"]);
}
