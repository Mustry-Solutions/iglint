//! Invalid expressions. Every case is rejected by Ignition 8.3.3; the first
//! error points at the token Ignition reports.

use iglint_expression::{LexErrorKind, ParseErrorKind, TokenKind, dump, parse};

/// Asserts `source` is invalid and the first error starts at the first
/// occurrence of `at` (or at the end for `""`).
#[track_caller]
fn invalid_at(source: &str, at: &str) {
    let p = parse(source);
    assert!(
        !p.is_valid(),
        "{source:?} should be invalid, got {}",
        dump(&p.expr)
    );
    let expected = if at.is_empty() {
        source.len()
    } else {
        source
            .find(at)
            .unwrap_or_else(|| panic!("{at:?} not in {source:?}"))
    };
    assert_eq!(
        p.errors[0].range.start().to_usize(),
        expected,
        "{source:?}: first error {:?}",
        p.errors[0]
    );
}

#[track_caller]
fn invalid_at_offset(source: &str, offset: usize) {
    let p = parse(source);
    assert!(!p.is_valid(), "{source:?}");
    assert_eq!(
        p.errors[0].range.start().to_usize(),
        offset,
        "{source:?}: {:?}",
        p.errors[0]
    );
}

#[test]
fn operators_that_do_not_exist() {
    invalid_at("1 == 2", "= 2");
    invalid_at("1 <> 2", "> 2");
    invalid_at("1 === 2", "== 2");
    invalid_at("1 => 2", "> 2");
    invalid_at("1 =< 2", "< 2");
    invalid_at("1 > = 2", "= 2");
    invalid_at("1 ! = 2", "!");
    invalid_at("1 > > 2", "> 2");
    invalid_at("1 < < 2", "< 2");
    invalid_at("1 <<= 2", "= 2");
    invalid_at("1 <=> 2", "> 2");
    invalid_at("1 &&& 2", "& 2");
    invalid_at("1 & & 2", "& 2");
    invalid_at("1 | | 2", "| 2");
    invalid_at("1 ||| 2", "| 2");
    invalid_at("1 ++ 2", "+ 2");
    invalid_at("+1", "+");
    invalid_at("a and b", "and");
    invalid_at("true and false", "and");
    invalid_at("not true", "true");
    invalid_at("1 ? 2 : 3", "?");
}

#[test]
fn bare_identifiers() {
    invalid_at("abc", "");
    invalid_at("abc + 1", "+");
    invalid_at("f", "");
    invalid_at("!a = b", "=");
    invalid_at("a = !b", "=");
    invalid_at("~a & b", "&");
    invalid_at("(f)(1)", ")(");
    invalid_at("nulls", "");
    invalid_at("truex", "");
    invalid_at("é", "");
    invalid_at("€", "");
    let p = parse("abc");
    assert_eq!(p.errors[0].kind, ParseErrorKind::BareIdentifier);
}

#[test]
fn keywords_are_not_functions() {
    invalid_at("true(1)", "(");
    invalid_at("null(1)", "(");
    invalid_at("None(1)", "(");
    invalid_at("like(1)", "like");
    invalid_at("xor(1)", "xor");
    invalid_at("LiKe", "LiKe");
    invalid_at("likes", "");
    invalid_at("xorx", "");
}

#[test]
fn incomplete_expressions() {
    invalid_at("", "");
    invalid_at("   ", "");
    invalid_at("// only a comment", "");
    invalid_at("1 +", "");
    invalid_at("1 + (2", "");
    invalid_at("(((1)", "");
    invalid_at("1 + 2)", ")");
    invalid_at("()", ")");
    invalid_at("(1, 2)", ",");
    invalid_at("1 like", "");
    invalid_at("like 1", "like");
    invalid_at("1 2", "2");
    invalid_at("true false", "false");
    invalid_at("\"a\"\"b\"", "\"b\"");
    invalid_at("'a''b'", "'b'");
    invalid_at("{a}{b}", "{b}");
    invalid_at("{a}\n{b}", "{b}");
    invalid_at("1!", "!");
    invalid_at("1~", "~");
    invalid_at("&1", "&");
    invalid_at("|1", "|");
    invalid_at("%1", "%");
    invalid_at("*1", "*");
    invalid_at("/1", "/");
    invalid_at("^1", "^");
    invalid_at("=1", "=");
    invalid_at("<1", "<");
    invalid_at("1 + 2 /", "");
    invalid_at_offset("1 + 2 /\n/", 8);
    invalid_at("1 +\n\"abc", "");
    invalid_at("1 + {", "");
    invalid_at("1 + \"abc", "");
    invalid_at("{x", "");
    invalid_at("\"", "");
    invalid_at("'", "");
}

#[test]
fn bad_calls_and_subscripts() {
    invalid_at("foo(,)", ",");
    invalid_at("foo(1,)", ")");
    invalid_at("f(1 2)", "2");
    invalid_at("f(1,,2)", ",2");
    invalid_at("f(1 ,2 ,)", ")");
    invalid_at("f(1)(2)", "(2");
    invalid_at("{x}[]", "]");
    invalid_at("{x}[0,1,2]", ",2");
    invalid_at("{x}[0", "");
    invalid_at("a.b(1)", ".");
    invalid_at("a.b.c(1, 2)", ".");
    invalid_at("{x}.y", ".");
}

#[test]
fn numbers() {
    invalid_at("1abc", "abc");
    invalid_at("1abc(2)", "abc");
    invalid_at("0x", "x");
    invalid_at("0xg", "xg");
    invalid_at("0x1g(1)", "g");
    invalid_at("1e", "e");
    invalid_at("0.5e", "e");
    invalid_at("1e+", "e");
    invalid_at_offset("1e5e5", 3);
    invalid_at("1L", "L");
    invalid_at("1f", "f");
    invalid_at("1.2.3", ".3");
    invalid_at("1..2", ".2");
    invalid_at("1 .5", ".5");
    invalid_at("1._5", "_5");
    invalid_at(".e5", ".");
    invalid_at_offset("1.5.", 3);
    invalid_at("9223372036854775808", "9");
    invalid_at("0xFFFFFFFFFFFFFFFF", "0");
    invalid_at("0x8000000000000000", "0");
    let p = parse("9223372036854775808");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::Lex(LexErrorKind::IntegerOverflow)
    );
}

#[test]
fn illegal_characters() {
    for (src, c) in [
        ("1 # c", '#'),
        ("@", '@'),
        ("1 ; 2", ';'),
        ("`a`", '`'),
        ("}", '}'),
        ("{x}}", '}'),
        ("1 \u{b}+ 2", '\u{b}'),
        ("1\u{a0}+ 2", '\u{a0}'),
        ("a😀(1)", '😀'),
        ("😀a(1)", '😀'),
        ("1 /* c */ + 2", '*'),
    ] {
        let p = parse(src);
        assert!(!p.is_valid(), "{src:?}");
        if c != '*' {
            assert!(
                p.errors
                    .iter()
                    .any(|e| e.kind == ParseErrorKind::Lex(LexErrorKind::IllegalCharacter(c))),
                "{src:?}: {:?}",
                p.errors
            );
        }
    }
    // `/* */` is not a comment: `*` after `/` is a syntax error.
    invalid_at("1 /* c */ + 2", "*");
}

#[test]
fn string_errors() {
    let cases = [
        ("\"a\\'b\"", LexErrorKind::IllegalEscape),
        ("'a\\\"b'", LexErrorKind::IllegalEscape),
        ("\"\\x41\"", LexErrorKind::IllegalEscape),
        ("\"a\\/\"", LexErrorKind::IllegalEscape),
        ("\"\\a\"", LexErrorKind::IllegalEscape),
        ("\"\\u0041\"", LexErrorKind::IllegalEscape),
        ("\"a\nb\"", LexErrorKind::UnterminatedString),
        ("\"a\rb\"", LexErrorKind::UnterminatedString),
        ("1 + 'x\ny'", LexErrorKind::UnterminatedString),
        ("\"abc\n", LexErrorKind::UnterminatedString),
        ("{x}\"abc\n", LexErrorKind::UnterminatedString),
        ("\"a\\", LexErrorKind::IllegalCharacter('\\')),
        ("\"a\\\nb\"", LexErrorKind::IllegalCharacter('\\')),
    ];
    for (src, kind) in cases {
        let p = parse(src);
        assert!(!p.is_valid(), "{src:?}");
        assert_eq!(p.errors[0].kind, ParseErrorKind::Lex(kind), "{src:?}");
    }
}

#[test]
fn reference_errors() {
    let p = parse("{a {b}");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::Lex(LexErrorKind::NestedReference)
    );
    assert_eq!(p.errors[0].range.slice("{a {b}"), "{");
    let p = parse("{{x}}");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::Lex(LexErrorKind::NestedReference)
    );
    let p = parse("{abc\n}");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::Lex(LexErrorKind::UnterminatedReference)
    );
    let p = parse("1 +\n{abc\n");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::Lex(LexErrorKind::UnterminatedReference)
    );
    invalid_at("{a\\}b}", "b}");
}

#[test]
fn error_kinds_and_messages() {
    let p = parse("1 +");
    assert_eq!(
        p.errors[0].kind,
        ParseErrorKind::UnexpectedToken {
            found: TokenKind::Eof,
            expected: "an expression"
        }
    );
    assert_eq!(
        p.errors[0].to_string(),
        "unexpected end of expression, expected an expression at 3..3"
    );
    let p = parse("f(1 2)");
    assert_eq!(
        p.errors[0].to_string(),
        "unexpected integer literal, expected `,` or `)` at 4..5"
    );
    let p = parse("1 2");
    assert_eq!(
        p.errors[0].to_string(),
        "unexpected integer literal, expected an operator or end of expression at 2..3"
    );
}

#[test]
fn recovery_keeps_later_references() {
    let src = "if({a} == 1, {b}, foo({c} +))";
    let p = parse(src);
    assert!(!p.is_valid());
    let paths: Vec<_> = p.expr.references().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, ["a", "b", "c"]);
    let p = parse("{a} {b} {c}");
    assert_eq!(p.errors.len(), 2);
    assert_eq!(p.expr.references().len(), 3);
    let p = parse("1 # 2 + {x}");
    assert_eq!(p.expr.references().len(), 1);
    let p = parse("f(1 2 {x})");
    assert_eq!(p.expr.references().len(), 1);
    assert_eq!(p.expr.calls().len(), 1);
}

#[test]
fn deep_nesting_does_not_overflow() {
    for depth in [60, 300, 990] {
        let src = "(".repeat(depth) + "1" + &")".repeat(depth);
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "-".repeat(depth) + "1";
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "f(".repeat(depth) + &")".repeat(depth);
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "1".to_owned() + &"[0]".repeat(depth);
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "{x}[".repeat(depth) + "0" + &"]".repeat(depth);
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "1".to_owned() + &" + 1".repeat(depth);
        assert!(parse(&src).is_valid(), "{depth}");
        let src = "(".repeat(depth) + "1";
        assert!(!parse(&src).is_valid(), "{depth}");
    }
}

#[test]
fn trees_deeper_than_the_limit_are_rejected() {
    assert_eq!(iglint_expression::MAX_DEPTH, 1000);
    let cases = [
        "(".repeat(5000) + "1" + &")".repeat(5000),
        "-".repeat(50_000) + "1",
        "1".to_owned() + &" + 1".repeat(200_000),
        "1".to_owned() + &"[0]".repeat(5000),
        "f(".repeat(3000) + &")".repeat(3000),
        "(".repeat(200_000),
    ];
    for src in &cases {
        let p = parse(src);
        assert!(
            p.errors.iter().any(|e| e.kind == ParseErrorKind::TooDeep),
            "{}",
            src.get(..10).unwrap()
        );
        assert!(matches!(p.expr.kind, iglint_expression::ExprKind::Error(_)));
    }
}
