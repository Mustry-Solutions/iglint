//! The public API beyond `parse_module`.

use iglint_python::ast::{Expr, StringValue};
use iglint_python::dump::{dump, dump_expr};
use iglint_python::{
    ParseErrorKind, ParseOptions, SourceKind, TokenKind, parse_expression,
    parse_module_with_options, tokenize,
};

#[test]
fn parse_expression_returns_the_expression() {
    let parsed = parse_expression("a + b * 2");
    assert!(parsed.errors.is_empty());
    let expr = parsed.expr.expect("expression");
    assert_eq!(
        dump_expr(&expr),
        "BinOp(left=Name(id='a', ctx=Load()), op=Add(), right=BinOp(left=Name(id='b', ctx=Load()), op=Mult(), right=Num(n=2)))"
    );
    assert!(!parse_expression("a +").errors.is_empty());
    assert!(!parse_expression("x = 1").errors.is_empty());
}

#[test]
fn options_enable_future_features() {
    let options = ParseOptions {
        print_function: true,
        unicode_literals: true,
        ..ParseOptions::default()
    };
    let parsed = parse_module_with_options("print('x', end='')\n", &options);
    assert!(parsed.errors.is_empty());
    let Some(iglint_python::Stmt::Expr(statement)) = parsed.module.body.first() else {
        panic!("expected an expression statement");
    };
    let Expr::Call(call) = &*statement.value else {
        panic!("expected a call");
    };
    let Expr::Str(string) = &call.args[0] else {
        panic!("expected a string");
    };
    assert_eq!(string.s, StringValue::Unicode("x".into()));
}

#[test]
fn encoding_declarations_depend_on_the_source_kind() {
    let source = "# -*- coding: latin-1 -*-\nx = '\u{e9}'\n";
    // Ignition compiles unicode text, where a declaration is an error.
    let parsed = iglint_python::parse_module(source);
    assert_eq!(
        parsed.errors[0].message,
        "encoding declaration in Unicode string"
    );
    // A file module uses it to encode plain string literals.
    let options = ParseOptions {
        source_kind: SourceKind::File,
        ..ParseOptions::default()
    };
    let parsed = parse_module_with_options(source, &options);
    assert!(parsed.errors.is_empty());
    assert_eq!(
        dump(&parsed.module),
        "Module(body=[Assign(targets=[Name(id='x', ctx=Store())], value=Str(s='\\xe9'))])"
    );
    let options = ParseOptions {
        source_kind: SourceKind::File,
        ..ParseOptions::default()
    };
    let parsed = parse_module_with_options("# coding: no-such-charset\n", &options);
    assert_eq!(parsed.errors[0].kind, ParseErrorKind::SyntaxError);
}

#[test]
fn null_bytes_are_rejected() {
    let parsed = iglint_python::parse_module("x = 1\0\n");
    assert_eq!(parsed.errors[0].kind, ParseErrorKind::NullCharacter);
}

#[test]
fn tokenize_produces_indentation_tokens() {
    let (tokens, errors) = tokenize("if x:\n    y\n");
    assert!(errors.is_empty());
    let kinds: Vec<TokenKind> = tokens.iter().map(|token| token.kind).collect();
    assert_eq!(
        kinds,
        [
            TokenKind::If,
            TokenKind::Name,
            TokenKind::Colon,
            TokenKind::Newline,
            TokenKind::Indent,
            TokenKind::Name,
            TokenKind::Newline,
            TokenKind::Dedent,
            TokenKind::EndOfFile,
        ]
    );
}
