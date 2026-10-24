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
            exercise(program.get(..end).unwrap());
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

fn messages(source: &str) -> Vec<String> {
    let parsed = iglint_python::parse_module(source);
    parsed.errors.iter().map(|e| e.message.clone()).collect()
}

#[test]
fn trees_may_be_as_deep_as_max_depth() {
    use iglint_python::{MAX_DEPTH, body_depth};
    // Assign > UnaryOp * n > Name.
    let n = MAX_DEPTH - 2;
    let parsed = iglint_python::parse_module(&format!("x = {}a\n", "-".repeat(n)));
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(body_depth(&parsed.module.body), MAX_DEPTH);
    let parsed = iglint_python::parse_module(&format!("x = {}a\ny = 1\n", "-".repeat(n + 1)));
    let errors: Vec<&str> = parsed.errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(errors, ["expression nested too deeply"]);
    // The statement is left out, the rest is kept.
    assert_eq!(parsed.module.body.len(), 1);
    let elifs = |n: usize| "if a: pass\n".to_owned() + &"elif a: pass\n".repeat(n) + "z = 1\n";
    // If * (n + 1) > Pass.
    let parsed = iglint_python::parse_module(&elifs(MAX_DEPTH - 2));
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(body_depth(&parsed.module.body), MAX_DEPTH);
    assert_eq!(
        messages(&elifs(MAX_DEPTH)),
        ["statements nested too deeply"]
    );
}

#[test]
fn chains_beyond_the_limit_are_errors_not_crashes() {
    for source in [
        "if a: pass\n".to_owned() + &"elif a: pass\n".repeat(30_000),
        format!("x = a{}\n", "[0]".repeat(50_000)),
        format!("x = a{}\n", ".b".repeat(100_000)),
        format!("x = a{}\n", "()".repeat(50_000)),
        format!("x = {}1\n", "-".repeat(100_000)),
        format!("x = {}1\n", "not ".repeat(100_000)),
        format!("x = {}\n", vec!["1"; 200_000].join("+")),
        format!("x = ({})\n", vec!["1"; 200_000].join("+\n")),
        // A syntax error after a long chain: the partial tree is dropped.
        format!("x = {} +\n", vec!["1"; 200_000].join("+")),
    ] {
        let errors = messages(&source);
        assert_eq!(errors.len(), 1, "{errors:?}");
    }
}

#[test]
fn expressions_nest_as_deep_as_jython_allows() {
    // Jython fails beyond about 1,950 levels on every construct.
    let parens = |n: usize| format!("x = {}1{}\n", "(".repeat(n), ")".repeat(n));
    assert!(messages(&parens(1_999)).is_empty());
    assert_eq!(messages(&parens(2_000)), ["expression nested too deeply"]);
    assert!(messages(&format!("x = {}1\n", "lambda: ".repeat(1_990))).is_empty());
    assert!(messages(&format!("x = {}\n", vec!["a"; 1_990].join("**"))).is_empty());
    assert!(
        messages(&format!(
            "x = {}1{}\n",
            "f(".repeat(1_990),
            ")".repeat(1_990)
        ))
        .is_empty()
    );
}
