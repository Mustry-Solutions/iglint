//! Valid expressions and their trees. Every expectation here was checked
//! against Ignition 8.3.3's own parser.

use iglint_expression::{ExprKind, dump, parse};

#[track_caller]
fn check(source: &str, expected: &str) {
    let p = parse(source);
    assert!(
        p.is_valid(),
        "{source:?} should be valid, got {:?}",
        p.errors
    );
    assert_eq!(dump(&p.expr), expected, "{source:?}");
}

#[test]
fn literals() {
    let cases = [
        ("0", "0"),
        ("42", "42"),
        ("9223372036854775807", "9223372036854775807"),
        ("0x1F", "31"),
        ("0X1f", "31"),
        ("0x7FFFFFFFFFFFFFFF", "9223372036854775807"),
        ("1.5", "1.5"),
        (".5", "0.5"),
        ("5.", "5.0"),
        ("1e5", "100000.0"),
        ("1E+05", "100000.0"),
        ("1.5e3", "1500.0"),
        ("1.5E-3", "0.0015"),
        ("1.e5", "100000.0"),
        ("007", "7.0"),
        ("00", "0.0"),
        ("0e5", "0.0"),
        ("true", "true"),
        ("TRUE", "true"),
        ("True", "true"),
        ("TrUe", "true"),
        ("false", "false"),
        ("fAlSe", "false"),
        ("null", "null"),
        ("NULL", "null"),
        ("None", "null"),
        ("none", "null"),
        ("NONE", "null"),
        ("\"abc\"", "\"abc\""),
        ("'abc'", "\"abc\""),
        ("\"\"", "\"\""),
        ("''", "\"\""),
        ("\"é€😀\"", "\"é€😀\""),
        ("'中文'", "\"中文\""),
        ("\"a//b\"", "\"a//b\""),
        ("\"{x}\"", "\"{x}\""),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn string_escapes() {
    let cases = [
        (r#""a\tb""#, "a\tb"),
        (r#""a\nb""#, "a\nb"),
        (r#""a\rb""#, "a\rb"),
        (r#""a\Tb""#, "a\tb"),
        (r#""a\Nb""#, "a\nb"),
        (r#""a\Rb""#, "a\rb"),
        (r#""a\\b""#, "a\\b"),
        (r#""q\"q""#, "q\"q"),
        (r#"'q\'q'"#, "q'q"),
        ("'say \"hi\"'", "say \"hi\""),
        ("\"it's\"", "it's"),
        ("\"tab\there\"", "tab\there"),
        (r#""\\\\""#, "\\\\"),
    ];
    for (src, decoded) in cases {
        let p = parse(src);
        assert!(p.is_valid(), "{src:?}: {:?}", p.errors);
        assert_eq!(p.expr.kind, ExprKind::String(decoded.to_owned()), "{src:?}");
    }
}

#[test]
fn references_are_verbatim() {
    let cases = [
        ("{this.props.text}", "this.props.text"),
        ("{[default]Folder/Tag}", "[default]Folder/Tag"),
        ("{ a }", " a "),
        ("{\ta}", "\ta"),
        ("{}", ""),
        ("{a//b}", "a//b"),
        ("{a\"b}", "a\"b"),
        ("{this.custom.€}", "this.custom.€"),
        ("{[default]中/x}", "[default]中/x"),
        ("{x[0]}", "x[0]"),
    ];
    for (src, path) in cases {
        let p = parse(src);
        assert!(p.is_valid(), "{src:?}");
        let refs = p.expr.references();
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].path, path);
        assert_eq!(refs[0].path_range.slice(src), path);
    }
}

#[test]
fn arithmetic_precedence() {
    let cases = [
        ("1 + 2 * 3", "(+ 1 (* 2 3))"),
        ("1 * 2 + 3", "(+ (* 1 2) 3)"),
        ("1 - 2 - 3", "(- (- 1 2) 3)"),
        ("8 / 2 / 2", "(/ (/ 8 2) 2)"),
        ("2 ^ 3 ^ 2", "(^ (^ 2 3) 2)"),
        ("2 % 3 ^ 2", "(^ (% 2 3) 2)"),
        ("2 ^ 3 % 2", "(% (^ 2 3) 2)"),
        ("1 % 2 * 3", "(* (% 1 2) 3)"),
        ("1 * 2 % 3", "(* 1 (% 2 3))"),
        ("1 ^ 2 * 3", "(* (^ 1 2) 3)"),
        ("1 / 2 ^ 3", "(/ 1 (^ 2 3))"),
        ("1 + 2 % 3", "(+ 1 (% 2 3))"),
        ("(1 + 2) * 3", "(* (paren (+ 1 2)) 3)"),
        ("((1))", "(paren (paren 1))"),
        ("1+2", "(+ 1 2)"),
        ("1-1", "(- 1 1)"),
        ("1 -1", "(- 1 1)"),
        ("\"a\" + 1", "(+ \"a\" 1)"),
        ("1 / 0", "(/ 1 0)"),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn unary_operators() {
    let cases = [
        ("-1", "(neg 1)"),
        ("- 1", "(neg 1)"),
        ("--2", "(neg (neg 2))"),
        ("1 - - 2", "(- 1 (neg 2))"),
        ("1 --2", "(- 1 (neg 2))"),
        ("-2 ^ 2", "(^ (neg 2) 2)"),
        ("2 ^ -3", "(^ 2 (neg 3))"),
        ("2 * -3", "(* 2 (neg 3))"),
        ("-2 * 3", "(* (neg 2) 3)"),
        ("!true", "(not true)"),
        ("!!true", "(not (not true))"),
        ("!1 + 2", "(+ (not 1) 2)"),
        ("~1 + 2", "(+ (bitnot 1) 2)"),
        ("~~1", "(bitnot (bitnot 1))"),
        ("~-!1", "(bitnot (neg (not 1)))"),
        ("-{x}", "(neg {x})"),
        ("!{a} && {b}", "(&& (not {a}) {b})"),
        ("!{a} = {b}", "(= (not {a}) {b})"),
        ("-{x}[0]", "(neg (index {x} 0))"),
        ("-1[0]", "(neg (index 1 0))"),
        ("!a(1)[0]", "(not (index (call a 1) 0))"),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn comparison_and_logic_precedence() {
    let cases = [
        ("1 = 2", "(= 1 2)"),
        ("1 != 2", "(!= 1 2)"),
        ("1 < 2 < 3", "(< (< 1 2) 3)"),
        ("1 = 2 = 3", "(= (= 1 2) 3)"),
        ("1 < 2 = 3", "(= (< 1 2) 3)"),
        ("1 = 2 < 3", "(= 1 (< 2 3))"),
        ("1 >= 2", "(>= 1 2)"),
        ("1 <= 2", "(<= 1 2)"),
        ("1 > 2", "(> 1 2)"),
        ("\"a\" like \"b\"", "(like \"a\" \"b\")"),
        ("\"a\" LIKE \"b\"", "(like \"a\" \"b\")"),
        ("\"a\" LiKe \"b\"", "(like \"a\" \"b\")"),
        ("1 like 2 = 3", "(= (like 1 2) 3)"),
        ("1 = 2 like 3", "(like (= 1 2) 3)"),
        ("true && false", "(&& true false)"),
        ("true || false", "(|| true false)"),
        ("1 || 2 && 3", "(|| 1 (&& 2 3))"),
        ("1 && 2 || 3", "(|| (&& 1 2) 3)"),
        ("1 && 2 = 3", "(&& 1 (= 2 3))"),
        (
            "{a} = 1 || {b} > 2 && {c}",
            "(|| (= {a} 1) (&& (> {b} 2) {c}))",
        ),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn bitwise_precedence() {
    let cases = [
        ("1 xor 2", "(xor 1 2)"),
        ("1 XOR 2", "(xor 1 2)"),
        ("1 Xor 2", "(xor 1 2)"),
        ("1 << 2 + 3", "(<< 1 (+ 2 3))"),
        ("1 >> 2 << 3", "(<< (>> 1 2) 3)"),
        ("1 >> 2", "(>> 1 2)"),
        ("1 & 2 = 2", "(& 1 (= 2 2))"),
        ("1 | 2 & 3", "(| 1 (& 2 3))"),
        ("1 xor 2 | 3", "(xor 1 (| 2 3))"),
        ("1 && 2 xor 3", "(&& 1 (xor 2 3))"),
        ("1 || 2 xor 3", "(|| 1 (xor 2 3))"),
        ("1 < 2 << 3", "(< 1 (<< 2 3))"),
        ("~1 & 2", "(& (bitnot 1) 2)"),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn calls() {
    let cases = [
        ("foo()", "(call foo)"),
        ("f (1)", "(call f 1)"),
        ("if(1,2,3)", "(call if 1 2 3)"),
        ("IF(1, 2, 3)", "(call IF 1 2 3)"),
        ("$abc(1)", "(call $abc 1)"),
        ("_a(1)", "(call _a 1)"),
        ("a1(1)", "(call a1 1)"),
        ("été(1)", "(call été 1)"),
        ("ŝ(1)", "(call ŝ 1)"),
        ("a€(1)", "(call a€ 1)"),
        ("nullx(1)", "(call nullx 1)"),
        ("trueish()", "(call trueish)"),
        ("f(g(h(1)), 2)", "(call f (call g (call h 1)) 2)"),
        ("f(1 + 2, -3)", "(call f (+ 1 2) (neg 3))"),
        ("foo(1)[2]", "(index (call foo 1) 2)"),
        (
            "if({view.params.mode} = 'edit', \"Save\", \"Edit\")",
            "(call if (= {view.params.mode} \"edit\") \"Save\" \"Edit\")",
        ),
        (
            "dateFormat(now(1000), \"HH:mm:ss\")",
            "(call dateFormat (call now 1000) \"HH:mm:ss\")",
        ),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
    let p = parse("TOSTR(1)");
    let calls = p.expr.calls();
    assert_eq!(calls[0].name, "TOSTR");
    assert_eq!(calls[0].name_lower(), "tostr");
    assert!(calls[0].is("toStr"));
}

#[test]
fn subscripts() {
    let cases = [
        ("{x}[0]", "(index {x} 0)"),
        ("{x} [0]", "(index {x} 0)"),
        ("{x}[0, 1]", "(index {x} 0 1)"),
        ("{x}[0][1]", "(index (index {x} 0) 1)"),
        ("{x}[0][1,2]", "(index (index {x} 0) 1 2)"),
        ("\"abc\"[1]", "(index \"abc\" 1)"),
        ("1[0]", "(index 1 0)"),
        ("(1 + 2)[0]", "(index (paren (+ 1 2)) 0)"),
        ("1 + 2[0]", "(+ 1 (index 2 0))"),
        ("{ds}[{row}, \"col\"]", "(index {ds} {row} \"col\")"),
        ("{x}[1 + 2]", "(index {x} (+ 1 2))"),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
}

#[test]
fn whitespace_and_comments() {
    let cases = [
        ("1 +\n2", "(+ 1 2)"),
        ("\n1", "1"),
        ("1 + 2\n", "(+ 1 2)"),
        ("1 \r+ 2", "(+ 1 2)"),
        ("1 \r\n+ 2", "(+ 1 2)"),
        ("1 \u{c}+ 2", "(+ 1 2)"),
        ("1 +\t2", "(+ 1 2)"),
        ("1 // comment", "1"),
        ("1 // €", "1"),
        ("1 // 😀", "1"),
        ("1 + 2 //", "(+ 1 2)"),
        ("1 //x\n+ 2", "(+ 1 2)"),
        ("1 //x\r+ 2", "(+ 1 2)"),
        ("1 //x\r\n+ 2", "(+ 1 2)"),
        ("{a}//comment", "{a}"),
        ("// leading\n1", "1"),
        ("  \t 1  ", "1"),
    ];
    for (src, expected) in cases {
        check(src, expected);
    }
    let p = parse("1 // a\n// b\n+ 2");
    assert_eq!(p.comments.len(), 2);
    assert_eq!(p.comments[0].slice("1 // a\n// b\n+ 2"), "// a");
}

#[test]
fn dropped_tail_is_not_an_error() {
    let cases = [
        ("1 + 2 \"", "(+ 1 2)"),
        ("1 'abc", "1"),
        ("f(1) {x", "(call f 1)"),
        ("{x}\"abc", "{x}"),
        ("\"abc\" \"", "\"abc\""),
        ("{x}{", "{x}"),
        ("\"abc\"[0]\"def", "(index \"abc\" 0)"),
    ];
    for (src, expected) in cases {
        check(src, expected);
        assert!(parse(src).dropped_tail.is_some(), "{src:?}");
    }
    assert!(parse("1").dropped_tail.is_none());
    let p = parse("1 'abc");
    let tail = p.dropped_tail.unwrap();
    assert_eq!(tail.range.slice("1 'abc"), "'abc");
    assert_eq!(tail.kind, iglint_expression::DroppedTailKind::String);
    let p = parse("1 {abc");
    assert_eq!(
        p.dropped_tail.unwrap().kind,
        iglint_expression::DroppedTailKind::Reference
    );
}

#[test]
fn ranges() {
    let src = "if({a} = 1, f(\"x\"), -2)";
    let p = parse(src);
    let ExprKind::Call(call) = &p.expr.kind else {
        panic!()
    };
    assert_eq!(p.expr.range.slice(src), src);
    assert_eq!(call.name_range.slice(src), "if");
    assert_eq!(call.args[0].range.slice(src), "{a} = 1");
    assert_eq!(call.args[1].range.slice(src), "f(\"x\")");
    assert_eq!(call.args[2].range.slice(src), "-2");
    let ExprKind::Binary(b) = &call.args[0].kind else {
        panic!()
    };
    assert_eq!(b.op_range.slice(src), "=");
    assert_eq!(b.left.range.slice(src), "{a}");
    let src = " ( {x} ) [ 0 , 1 ] ";
    let p = parse(src);
    assert_eq!(p.expr.range.slice(src), "( {x} ) [ 0 , 1 ]");
}

#[test]
fn unicode_ranges_are_byte_offsets() {
    let src = "\"é\" + {€}";
    let p = parse(src);
    let refs = p.expr.references();
    assert_eq!(refs[0].path_range.slice(src), "€");
    assert_eq!(refs[0].path_range.start().to_u32(), 8);
}
