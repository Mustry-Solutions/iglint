//! The parser must never panic, loop forever or produce out-of-bounds ranges,
//! whatever the input.

use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python::{Expr, Locator, Stmt};
use iglint_text::{Ranged, TextSize};

/// A small deterministic pseudo-random generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// Checks that every node range lies within the source.
struct RangeCheck {
    len: TextSize,
}

impl<'a> Visitor<'a> for RangeCheck {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        assert!(stmt.range().end() <= self.len, "{stmt:?}");
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        assert!(expr.range().end() <= self.len, "{expr:?}");
        walk_expr(self, expr);
    }
}

fn exercise(source: &str) {
    let parsed = iglint_python::parse_module(source);
    let len = TextSize::of(source);
    RangeCheck { len }.visit_module(&parsed.module);
    let locator = Locator::new(source);
    for error in &parsed.errors {
        assert!(error.range.end() <= len, "{error:?}");
        let _ = error.jython_location(&locator);
    }
    for dropped in &parsed.dropped {
        assert!(dropped.range.end() <= len);
    }
    let _ = iglint_python::dump::dump(&parsed.module);
    let _ = iglint_python::dump::dump_with_attributes(&parsed.module, source);
    let _ = iglint_python::parse_expression(source);
}

const FRAGMENTS: &[&str] = &[
    "def ", "class ", "if ", "elif ", "else", ":", "\n", "\n    ", "\n\t", "(", ")", "[", "]", "{",
    "}", "'", "\"", "'''", "\"\"\"", "\\", "\\\n", "#", "x", "1", "0x", ".", "e", "j", "L", ",",
    "=", "+=", "**", "lambda", "yield", "print", ">>", "for ", " in ", "try", "except", "finally",
    "with ", " as ", "@", "`", "!", "\r", "\r\n", "\u{c}", "\u{0}", "é", "😀", "None", "u'", "r'",
    "from ", "import ", "*", "not ", " is ", "\u{feff}", ";", "global ", "exec ", "del ", "...",
];

#[test]
fn random_fragments_do_not_panic() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let count = 1 + rng.below(30);
        let source: String = (0..count)
            .map(|_| FRAGMENTS[rng.below(FRAGMENTS.len())])
            .collect();
        exercise(&source);
    }
}

#[test]
fn random_bytes_do_not_panic() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for _ in 0..20_000 {
        let len = rng.below(64);
        let bytes: Vec<u8> = (0..len).map(|_| (rng.next() & 0xFF) as u8).collect();
        let source = String::from_utf8_lossy(&bytes);
        exercise(&source);
    }
}

#[test]
fn every_prefix_of_a_program_parses_without_panicking() {
    let program = include_str!("oracle/statements.cases");
    let mut end = 0;
    while end <= program.len() {
        if program.is_char_boundary(end) {
            exercise(&program[..end]);
        }
        end += 7;
    }
}

#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    let source = format!("x = {}1{}\n", "(".repeat(200), ")".repeat(200));
    exercise(&source);
    let source = format!("x = {}1\n", "-".repeat(2000));
    exercise(&source);
    let mut source = String::new();
    for depth in 0..120 {
        source.push_str(&" ".repeat(depth));
        source.push_str("if x:\n");
    }
    source.push_str(&" ".repeat(120));
    source.push_str("pass\n");
    exercise(&source);
}
