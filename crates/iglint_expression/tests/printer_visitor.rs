use iglint_expression::{
    BinaryOp, BoundRef, Call, Expr, ExprKind, TokenKind, TokenValue, Visitor, dump, lex, parse,
    quote_string, to_source, walk_call,
};

#[track_caller]
fn round_trip(src: &str) -> String {
    let p = parse(src);
    assert!(p.is_valid(), "{src:?}");
    let printed = to_source(&p.expr);
    let again = parse(&printed);
    assert!(again.is_valid(), "{src:?} printed as {printed:?}");
    assert_eq!(
        dump(&again.expr),
        dump(&p.expr),
        "{src:?} printed as {printed:?}"
    );
    printed
}

#[test]
fn printer_round_trips() {
    let cases = [
        ("1+2*3", "1 + 2 * 3"),
        ("(1+2)*3", "(1 + 2) * 3"),
        ("1-(2-3)", "1 - (2 - 3)"),
        ("-2^2", "-2 ^ 2"),
        ("-(2^2)", "-(2 ^ 2)"),
        ("!{a}&&{b}", "!{a} && {b}"),
        ("if({a}='x',\"y\",'z')", "if({a} = \"x\", \"y\", \"z\")"),
        ("{x}[0,1]", "{x}[0, 1]"),
        ("0x1F", "31"),
        ("007", "7.0"),
        ("1e300", "1e300"),
        ("1e999", "1e999"),
        ("TRUE", "true"),
        ("None", "null"),
        ("'a\\'b\\\\c\\td'", "\"a'b\\\\c\\td\""),
        ("1 xor 2 LIKE 3", "1 xor 2 like 3"),
        ("--1", "--1"),
        ("f()", "f()"),
        ("// c\n1", "1"),
    ];
    for (src, expected) in cases {
        assert_eq!(round_trip(src), expected, "{src:?}");
    }
}

#[test]
fn printer_adds_needed_parentheses_for_built_trees() {
    let p = parse("1 - 2");
    let mut expr = p.expr.clone();
    // Build `1 - (1 - 2)` by hand without a Paren node.
    expr = Expr {
        range: expr.range,
        kind: ExprKind::Binary(Box::new(iglint_expression::Binary {
            op: BinaryOp::Sub,
            op_range: expr.range,
            left: parse("1").expr,
            right: expr,
        })),
    };
    assert_eq!(to_source(&expr), "1 - (1 - 2)");
    let neg = Expr {
        range: expr.range,
        kind: ExprKind::Integer(-5),
    };
    assert_eq!(to_source(&neg), "(-5)");
    for (v, s) in [
        (f64::NAN, "(0.0 / 0.0)"),
        (f64::INFINITY, "1e999"),
        (f64::NEG_INFINITY, "(-1e999)"),
        (-1.5, "(-1.5)"),
    ] {
        let e = Expr {
            range: expr.range,
            kind: ExprKind::Float(v),
        };
        assert_eq!(to_source(&e), s);
    }
}

#[test]
fn quote_string_escapes() {
    assert_eq!(quote_string("a\"b"), "\"a\\\"b\"");
    assert_eq!(quote_string("x\ny\rz\tw\\"), "\"x\\ny\\rz\\tw\\\\\"");
    assert_eq!(quote_string("é'"), "\"é'\"");
    for s in ["", "plain", "\"\"", "\\", "\n", "it's"] {
        let p = parse(&quote_string(s));
        assert_eq!(p.expr.kind, ExprKind::String(s.to_owned()));
    }
}

#[test]
fn dump_forms() {
    let cases = [
        ("{a}", "{a}"),
        ("-1", "(neg 1)"),
        ("~1", "(bitnot 1)"),
        ("!1", "(not 1)"),
        ("(1)", "(paren 1)"),
        ("1.0", "1.0"),
        ("f(1)", "(call f 1)"),
        ("{x}[1]", "(index {x} 1)"),
        ("\"a\\nb\"", "\"a\\nb\""),
    ];
    for (src, d) in cases {
        assert_eq!(dump(&parse(src).expr), d);
    }
    assert_eq!(dump(&parse("{a} {b}").expr), "(error {a} {b})");
}

#[derive(Default)]
struct Collect {
    refs: Vec<String>,
    calls: Vec<String>,
    literals: usize,
    binaries: Vec<BinaryOp>,
}

impl Visitor for Collect {
    fn visit_reference(&mut self, _expr: &Expr, reference: &BoundRef) {
        self.refs.push(reference.path.clone());
    }
    fn visit_call(&mut self, expr: &Expr, call: &Call) {
        self.calls.push(call.name.clone());
        walk_call(self, expr, call);
    }
    fn visit_literal(&mut self, _expr: &Expr) {
        self.literals += 1;
    }
    fn visit_binary(&mut self, _expr: &Expr, binary: &iglint_expression::Binary) {
        self.binaries.push(binary.op);
        self.visit_expr(&binary.left);
        self.visit_expr(&binary.right);
    }
}

#[test]
fn visitor_sees_everything() {
    let p = parse("if({a} > 1, coalesce({b}, -{c}[0]), toStr((1 + 2) * {d}))");
    let mut v = Collect::default();
    v.visit_expr(&p.expr);
    assert_eq!(v.refs, ["a", "b", "c", "d"]);
    assert_eq!(v.calls, ["if", "coalesce", "toStr"]);
    assert_eq!(v.literals, 4);
    assert_eq!(v.binaries, [BinaryOp::Gt, BinaryOp::Mul, BinaryOp::Add]);
    // Error nodes are visited too.
    let p = parse("{a} {b} f({c} 1)");
    let mut v = Collect::default();
    v.visit_expr(&p.expr);
    assert_eq!(v.refs, ["a", "b", "c"]);
}

#[test]
fn walk_children_and_helpers() {
    let p = parse("((f({x}, 2)))");
    assert!(matches!(p.expr.unparenthesized().kind, ExprKind::Call(_)));
    assert_eq!(p.expr.children().len(), 1);
    let mut count = 0;
    p.expr.walk(&mut |_| count += 1);
    assert_eq!(count, 5);
    assert!(parse("1").expr.is_literal());
    assert!(!parse("{x}").expr.is_literal());
    assert!(BinaryOp::Like.is_comparison());
    assert!(BinaryOp::Or.is_logical());
    assert!(BinaryOp::BitXor.is_bitwise());
    assert!(BinaryOp::Pow.is_arithmetic());
    assert_eq!(BinaryOp::Shl.to_string(), "<<");
}

#[test]
fn lexer_tokens() {
    let lexed = lex("a(1, 0x10, 2.5, 'x', {p}) // c");
    let kinds: Vec<TokenKind> = lexed.tokens.iter().map(|t| t.kind).collect();
    use TokenKind::*;
    assert_eq!(
        kinds,
        [
            Ident, LParen, Integer, Comma, Integer, Comma, Float, Comma, String, Comma, Reference,
            RParen, Eof
        ]
    );
    assert_eq!(lexed.tokens[4].value, TokenValue::Integer(16));
    assert_eq!(lexed.tokens[6].value, TokenValue::Float(2.5));
    assert_eq!(lexed.tokens[8].value, TokenValue::Text("x".to_owned()));
    assert_eq!(lexed.tokens[10].value, TokenValue::Text("p".to_owned()));
    assert_eq!(lexed.comments.len(), 1);
    let ops = lex("|| && | & xor = != < <= > >= << >> + - * / % ^ ! ~ like");
    let kinds: Vec<TokenKind> = ops.tokens.iter().map(|t| t.kind).collect();
    assert_eq!(
        kinds,
        [
            PipePipe, AmpAmp, Pipe, Amp, Xor, Eq, NotEq, Lt, LtEq, Gt, GtEq, Shl, Shr, Plus, Minus,
            Star, Slash, Percent, Caret, Bang, Tilde, Like, Eof
        ]
    );
    assert_eq!(TokenKind::Eof.to_string(), "end of expression");
}

#[test]
fn identifier_classes() {
    use iglint_expression::{is_identifier_part, is_identifier_start};
    for c in [
        'a', 'Z', '_', '$', 'é', 'ŝ', '€', '¢', 'ª', 'µ', 'º', 'ÿ', 'Ω', 'ж', '中',
    ] {
        assert!(is_identifier_start(c), "{c:?}");
    }
    for c in [
        '0', '9', '\u{0}', '\u{8}', '\u{e}', '\u{1b}', '\u{7f}', '\u{9f}', '\u{ad}',
    ] {
        assert!(!is_identifier_start(c) || c.is_alphabetic(), "{c:?}");
        assert!(is_identifier_part(c), "{c:?}");
    }
    for c in [' ', '.', '-', '#', '\u{a0}', '\u{37f}', '😀', '\u{2028}'] {
        assert!(!is_identifier_start(c), "{c:?}");
        assert!(!is_identifier_part(c), "{c:?}");
    }
}
