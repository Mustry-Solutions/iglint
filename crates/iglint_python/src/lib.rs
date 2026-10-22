//! Jython 2.7 lexer, parser and syntax tree.
//!
//! The parser accepts exactly what Inductive Automation's Jython 2.7.3 (the
//! interpreter of Ignition 8.3) accepts and builds the same tree as its
//! `_ast` module, including Jython's idiosyncrasies. It is tested
//! differentially against the real Jython parser (see `oracle/` in the
//! repository). Unlike Jython it never stops at the first error: it reports
//! the error Jython would report first and keeps going, so that the rest of a
//! file can still be analysed.
//!
//! ```
//! let parsed = iglint_python::parse_module("print >>f, 'x'\n");
//! assert!(parsed.errors.is_empty());
//! assert_eq!(
//!     iglint_python::dump::dump(&parsed.module),
//!     "Module(body=[Print(dest=Name(id='f', ctx=Load()), values=[Str(s='x')], nl=True)])"
//! );
//! ```
//!
//! # Jython behaviour worth knowing
//!
//! * The source is parsed as `compile()` sees it: with two newlines
//!   appended, lines separated by `\n` only, columns counted in UTF-16 code
//!   units ([`Locator`] converts offsets).
//! * Some malformed tokens (an unterminated single-quoted string, a stray
//!   backslash, `!` without `=`) are silently dropped by Jython's lexer
//!   together with the next character; the code is accepted with that text
//!   missing. [`Parsed::dropped`] lists every such place.
//! * Keywords are allowed as attribute names (`x.print`, `x.from`) and as
//!   later parts of dotted import names (`import a.if`).
//! * Several assignment targets that CPython rejects are accepted (`-a = 1`,
//!   `a < b = 1`, `for f() in x:`, `del a + b`, `except E, f():`), with the
//!   target context applied to their operands.
//! * Node positions follow Jython's tokens, not CPython's: a chained
//!   arithmetic operation is anchored at its operator, a `with` statement at
//!   its first context expression, a negative literal at its digits, a
//!   multi-line string at its last line with column -1.
//! * Float reprs use the digits of Java's `Double.toString`
//!   (`4.9e-324`, `6.1987046510000005e+18`); dictionary comprehensions dump
//!   as the bare word `DictComp`.
//!
//! # Deliberate differences
//!
//! * Expressions nested more than 100 levels deep are rejected; Jython's
//!   limit depends on the JVM stack.
//! * After the first error, recovery may differ from what Jython would do
//!   (Jython stops there).

pub mod anchor;
pub mod ast;
mod charsets;
pub mod dump;
pub mod error;
mod indentation;
mod java_double;
mod lexer;
mod literal;
mod parser;
pub mod query;
pub mod repr;
pub mod token;
mod unicode_names;
pub mod visitor;

pub use anchor::{Anchor, AnchorKind, JythonLocation, Locator};
pub use ast::{Expr, Module, Stmt};
pub use error::{DropReason, DroppedText, ParseError, ParseErrorKind};
pub use literal::SourceEncoding;
pub use token::{Token, TokenKind};

/// How the source text reached the compiler.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceKind {
    /// A unicode string handed to `compile()`, which is how Ignition
    /// compiles every script. A PEP 263 encoding declaration is a syntax
    /// error, and plain string literals are UTF-8 encoded.
    #[default]
    Unicode,
    /// A module file read from disk. A PEP 263 encoding declaration selects
    /// the charset that plain string literals are encoded with.
    File,
}

/// Parser settings.
#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    /// Parse as if `from __future__ import print_function` were in effect
    /// from the start (`print` is a name).
    pub print_function: bool,
    /// Parse as if `from __future__ import unicode_literals` were in effect.
    pub unicode_literals: bool,
    pub source_kind: SourceKind,
}

/// The result of parsing a module.
#[derive(Clone, Debug)]
pub struct Parsed {
    /// The syntax tree. Statements that could not be parsed are left out.
    pub module: Module,
    /// Errors in the order Jython would encounter them: the first one is the
    /// error Jython reports. The rest are found after error recovery.
    pub errors: Vec<ParseError>,
    /// Text that Jython's lexer silently discards (see [`DroppedText`]).
    pub dropped: Vec<DroppedText>,
}

impl Parsed {
    /// Whether Jython accepts the source.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// The result of parsing an expression.
#[derive(Clone, Debug)]
pub struct ParsedExpression {
    /// The expression, unless it could not be parsed at all.
    pub expr: Option<Expr>,
    pub errors: Vec<ParseError>,
    pub dropped: Vec<DroppedText>,
}

/// Parses a module, like Jython's `compile(source, filename, 'exec')`.
pub fn parse_module(source: &str) -> Parsed {
    parse_module_with_options(source, &ParseOptions::default())
}

/// Parses a module with the given options.
pub fn parse_module_with_options(source: &str, options: &ParseOptions) -> Parsed {
    let (module, errors, dropped) = parser::Parser::new(source, options).parse_module();
    Parsed {
        module,
        errors,
        dropped,
    }
}

/// Parses an expression, like Jython's `compile(source, filename, 'eval')`.
pub fn parse_expression(source: &str) -> ParsedExpression {
    let (expr, errors, dropped) =
        parser::Parser::new(source, &ParseOptions::default()).parse_expression();
    ParsedExpression {
        expr,
        errors,
        dropped,
    }
}

/// The PEP 263 encoding declaration in the first two lines of `source`, if
/// any: the encoding name and its range. Jython finds it with the pattern
/// `#.*coding[:=]\s*([-\w.]+)`. In a script compiled from a unicode string,
/// as Ignition does, such a declaration is a syntax error.
pub fn encoding_declaration(source: &str) -> Option<(&str, iglint_text::TextRange)> {
    parser::encoding_declaration(source)
}

/// The tokens the parser sees for `source`, up to and including the end of
/// file, together with the lexical errors in the order they occur.
pub fn tokenize(source: &str) -> (Vec<Token>, Vec<ParseError>) {
    let mut tokens = Vec::new();
    let mut source_tokens = indentation::TokenSource::new(lexer::Lexer::new(source));
    loop {
        let token = source_tokens.next_token();
        tokens.push(token);
        if token.kind == TokenKind::EndOfFile {
            break;
        }
    }
    let lexer = source_tokens.into_lexer();
    (tokens, lexer.errors)
}

/// Like [`tokenize`], but stops where Jython stops: at the first lexical
/// error. Returns the tokens produced before it and the error.
pub fn tokenize_until_error(source: &str) -> (Vec<Token>, Option<ParseError>) {
    let mut tokens = Vec::new();
    let mut source_tokens = indentation::TokenSource::new(lexer::Lexer::new(source));
    loop {
        let token = source_tokens.next_token();
        if let Some(error) = source_tokens.lexer().errors.first() {
            return (tokens, Some(error.clone()));
        }
        tokens.push(token);
        if token.kind == TokenKind::EndOfFile {
            return (tokens, None);
        }
    }
}
