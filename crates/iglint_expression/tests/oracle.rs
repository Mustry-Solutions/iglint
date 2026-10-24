//! Differential tests against Ignition's own parsers.
//!
//! `tests/oracle/*.txt` hold inputs generated from fixed seeds together with
//! what Ignition 8.3.3 answered for them (`oracle/expression/run.sh record`).
//! Each line is `<hex-encoded input>\t<result>`; see
//! `oracle/expression/ignition_oracle.py` for the result formats.
//! `oracle/expression/run.sh random` produces a fresh set and runs
//! `oracle_random_run` on it.

use std::path::{Path, PathBuf};

use iglint_expression::{
    BinaryOp, Expr, ExprKind, LexErrorKind, ParseErrorKind, UnaryOp, is_property_ref, parse,
    parse_property_ref, parse_tag_path,
};
use iglint_text::TextSize;

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> String {
    let bytes: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).unwrap(), 16).expect("hex input"))
        .collect();
    String::from_utf8(bytes).expect("UTF-8 input")
}

/// The tree in the oracle's notation (Ignition class and operator names).
fn dump(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Integer(v) => format!("Long({v})"),
        ExprKind::Float(v) => format!("Double({:016x})", v.to_bits()),
        ExprKind::String(s) => format!("Str({})", hex(s)),
        ExprKind::Bool(b) => format!("Bool({b})"),
        ExprKind::Null => "Null".into(),
        ExprKind::Reference(r) => format!("Bound({})", hex(&r.path)),
        ExprKind::Paren(inner) => dump(inner),
        ExprKind::Call(c) => format!(
            "Call({};{})",
            hex(&c.name),
            c.args.iter().map(dump).collect::<Vec<_>>().join(",")
        ),
        ExprKind::Unary(u) => {
            let (cls, op) = match u.op {
                UnaryOp::Neg => ("UMinusExpression", "Unary Minus"),
                UnaryOp::Not => ("NotExpression", "Logical Not"),
                UnaryOp::BitNot => ("BitwiseNot", "Bitwise NOT"),
            };
            format!("{cls}[{op}]({})", dump(&u.operand))
        }
        ExprKind::Binary(b) => {
            let (cls, op) = match b.op {
                BinaryOp::Add => ("ArithmeticExpression", "PLUS"),
                BinaryOp::Sub => ("ArithmeticExpression", "MINUS"),
                BinaryOp::Mul => ("ArithmeticExpression", "MULTIPLY"),
                BinaryOp::Div => ("ArithmeticExpression", "DIVIDE"),
                BinaryOp::Mod => ("ArithmeticExpression", "MOD"),
                BinaryOp::Pow => ("ArithmeticExpression", "POWER"),
                BinaryOp::Eq => ("EqualityExpression", "Equal"),
                BinaryOp::NotEq => ("EqualityExpression", "Not Equal"),
                BinaryOp::Like => ("LikeExpression", "Like"),
                BinaryOp::Lt => ("ComparisonExpression", "Less Than"),
                BinaryOp::LtEq => ("ComparisonExpression", "Less Than or Equal"),
                BinaryOp::Gt => ("ComparisonExpression", "Greater Than"),
                BinaryOp::GtEq => ("ComparisonExpression", "Greater Than or Equal"),
                BinaryOp::And => ("LogicalExpression", "And"),
                BinaryOp::Or => ("LogicalExpression", "Or"),
                BinaryOp::BitAnd => ("BitwiseExpression", "BAnd"),
                BinaryOp::BitOr => ("BitwiseExpression", "BOr"),
                BinaryOp::BitXor => ("BitwiseExpression", "XOR"),
                BinaryOp::Shl => ("BitwiseExpression", "LShift"),
                BinaryOp::Shr => ("BitwiseExpression", "RShift"),
            };
            format!("{cls}[{op}]({},{})", dump(&b.left), dump(&b.right))
        }
        ExprKind::Subscript(s) => {
            let mut parts = vec![dump(&s.target), dump(&s.index)];
            if let Some(c) = &s.column {
                parts.push(dump(c));
            }
            format!("SubscriptExpression[Subscript]({})", parts.join(","))
        }
        ExprKind::Error(_) => "ERROR".into(),
    }
}

/// The first error as Ignition reports it: `line:column` (1-based, UTF-16
/// columns, JFlex line breaks), `0:0` at the end of input, `?` when the
/// exception has no position (integer overflow). Ignition reports string and
/// reference tokens at their closing delimiter.
fn error_position(src: &str, kind: &ParseErrorKind, start: usize, end: usize) -> String {
    let lexical = matches!(kind, ParseErrorKind::Lex(_));
    if *kind == ParseErrorKind::Lex(LexErrorKind::IntegerOverflow) {
        return "?".into();
    }
    if start == src.len() && !lexical {
        return "0:0".into();
    }
    let mut at = start;
    if !lexical && matches!(src.as_bytes().get(at), Some(b'"' | b'\'' | b'{')) {
        at = end - 1;
    }
    let b = &src.as_bytes()[..at];
    let (mut line, mut line_start, mut i) = (1, 0, 0);
    while i < b.len() {
        if matches!(b[i], b'\n' | b'\r' | 0x0B | 0x0C) {
            if b[i] == b'\r' && b.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            line += 1;
            line_start = i + 1;
        }
        i += 1;
    }
    let column = src.get(line_start..at).unwrap().encode_utf16().count() + 1;
    format!("{line}:{column}")
}

fn expression_result(src: &str) -> String {
    let p = parse(src);
    match p.errors.first() {
        None => format!("OK\t{}", dump(&p.expr)),
        Some(e) => format!(
            "ERR\t{}",
            error_position(
                src,
                &e.kind,
                e.range.start().to_usize(),
                e.range.end().to_usize()
            )
        ),
    }
}

fn property_result(src: &str) -> String {
    let modern = parse_property_ref(src, TextSize::ZERO).is_some_and(|p| !p.legacy);
    format!("{modern}\t{}", is_property_ref(src))
}

fn tag_result(src: &str) -> String {
    let t = parse_tag_path(src, TextSize::ZERO);
    if !t.is_valid() {
        return "ERR".into();
    }
    let comps: Vec<String> = t.components.iter().map(|c| hex(&c.text)).collect();
    format!(
        "OK\t{}\t{}\t{}",
        t.source_name().map(hex).unwrap_or_default(),
        comps.join(","),
        if t.property.is_some() { "P" } else { "-" }
    )
}

/// Compares every line of `file`; returns (cases, mismatches) and prints the
/// first few mismatches.
fn compare(file: &Path, ours: fn(&str) -> String) -> (usize, usize) {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let mut cases = 0;
    let mut bad = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (input, expected) = line.split_once('\t').expect("<hex>\\t<result>");
        let src = unhex(input);
        let got = ours(&src);
        cases += 1;
        if got != expected {
            bad += 1;
            if bad <= 10 {
                eprintln!(
                    "{}: {src:?}\n  ignition: {expected}\n  iglint:   {got}",
                    file.display()
                );
            }
        }
    }
    (cases, bad)
}

fn recorded(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/oracle")
        .join(name)
}

#[test]
fn recorded_expressions_match_ignition() {
    let (cases, bad) = compare(&recorded("expressions.txt"), expression_result);
    assert!(cases >= 2000);
    assert_eq!(bad, 0, "{bad} of {cases} expressions differ from Ignition");
}

#[test]
fn recorded_property_refs_match_perspective() {
    let (cases, bad) = compare(&recorded("property_refs.txt"), property_result);
    assert!(cases >= 2000);
    assert_eq!(
        bad, 0,
        "{bad} of {cases} property paths differ from Perspective"
    );
}

#[test]
fn recorded_tag_paths_match_ignition() {
    let (cases, bad) = compare(&recorded("tag_paths.txt"), tag_result);
    assert!(cases >= 2000);
    assert_eq!(bad, 0, "{bad} of {cases} tag paths differ from Ignition");
}

/// Run by `oracle/expression/run.sh random` on freshly generated
/// expectations in `$IGLINT_EXPRESSION_ORACLE_DIR`.
#[test]
#[ignore = "needs IGLINT_EXPRESSION_ORACLE_DIR from oracle/expression/run.sh random"]
fn oracle_random_run() {
    let dir = PathBuf::from(
        std::env::var_os("IGLINT_EXPRESSION_ORACLE_DIR").expect("IGLINT_EXPRESSION_ORACLE_DIR"),
    );
    let mut failed = false;
    for (name, ours) in [
        ("expressions.txt", expression_result as fn(&str) -> String),
        ("property_refs.txt", property_result),
        ("tag_paths.txt", tag_result),
    ] {
        let (cases, bad) = compare(&dir.join(name), ours);
        eprintln!("{name}: {cases} cases, {bad} mismatches");
        failed |= bad > 0;
    }
    assert!(!failed, "iglint differs from Ignition");
}
