//! A recursive-descent parser that follows Jython's `Python.g` decision by
//! decision.
//!
//! Jython's ANTLR parser stops at the first error. This parser reports the
//! same first error (same position, same order relative to lexical errors)
//! and then recovers at the next statement boundary, so that the rest of the
//! file still produces a tree.
//!
//! Where the grammar uses syntactic predicates, the parser either decides
//! deterministically with an equivalent check or speculates (see
//! [`Parser::speculate`]).

mod expression;
mod python3;
mod statement;
mod target;

use iglint_text::{TextRange, TextSize};

use crate::anchor::Anchor;
use crate::ast::Module;
use crate::error::{ParseError, ParseErrorKind};
use crate::indentation::TokenSource;
use crate::lexer::Lexer;
use crate::literal::SourceEncoding;
use crate::token::{Token, TokenKind};
use crate::{DroppedText, ParseOptions, SourceKind};

/// A parse function failed; the error has been recorded (unless the parser
/// is speculating).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Failure;

pub(crate) type PResult<T> = Result<T, Failure>;

/// How deeply expressions may nest (parentheses, unary operators, ...).
/// Jython itself fails with a Java `StackOverflowError` on deep nesting; the
/// limit keeps this parser within the stack of a worker thread.
const MAX_NESTING: u32 = 100;

/// ANTLR's `CommonTokenStream` over the indentation filter: a buffer that is
/// filled lazily, and that fetches the next token as soon as one is
/// consumed (which determines when lexical errors surface).
struct TokenStream<'src> {
    source: TokenSource<'src>,
    tokens: Vec<Token>,
    position: usize,
    fetched_eof: bool,
}

impl<'src> TokenStream<'src> {
    fn new(source: TokenSource<'src>) -> Self {
        TokenStream {
            source,
            tokens: Vec::new(),
            position: 0,
            fetched_eof: false,
        }
    }

    /// Makes sure the token at `index` is fetched (or the end reached).
    fn sync(&mut self, index: usize) {
        while self.tokens.len() <= index && !self.fetched_eof {
            let token = self.source.next_token();
            if token.kind == TokenKind::EndOfFile {
                self.fetched_eof = true;
            }
            self.tokens.push(token);
        }
    }

    /// `LT(k)` for `k >= 1`.
    fn lt(&mut self, k: usize) -> Token {
        let index = self.position + k - 1;
        self.sync(index);
        match self.tokens.get(index) {
            Some(token) => *token,
            // Past the end: repeat the end-of-file token.
            None => *self
                .tokens
                .last()
                .expect("the stream ends with an EOF token"),
        }
    }

    fn consume(&mut self) {
        self.sync(self.position);
        if self.tokens[self.position].kind != TokenKind::EndOfFile {
            self.position += 1;
        }
        self.sync(self.position);
    }

    fn errors(&mut self) -> &mut Vec<ParseError> {
        &mut self.source.lexer_mut().errors
    }
}

/// The position to return to when speculation fails.
#[derive(Clone, Copy)]
struct Checkpoint {
    position: usize,
    previous_end: TextSize,
    deferred: usize,
}

pub(crate) struct Parser<'src> {
    source: &'src str,
    stream: TokenStream<'src>,
    /// End of the last consumed token that has text (not NEWLINE, INDENT,
    /// DEDENT or end of file).
    previous_end: TextSize,
    /// `from __future__ import print_function` seen: `print` is a name.
    print_function: bool,
    /// `from __future__ import unicode_literals` seen.
    unicode_literals: bool,
    encoding: SourceEncoding,
    /// Jython's `suite` scope: whether `continue` is illegal in the
    /// innermost suite (it is directly inside a `finally` clause).
    suites: Vec<bool>,
    /// Nesting depth of speculative parses.
    speculating: u32,
    /// Nesting depth of predicate regions (see [`Parser::region`]).
    regions: u32,
    /// Current expression nesting (see [`MAX_NESTING`]).
    depth: u32,
    /// Errors held back while speculating (dropped if the speculation
    /// fails) or inside a region.
    deferred: Vec<ParseError>,
}

impl<'src> Parser<'src> {
    pub(crate) fn new(source: &'src str, options: &ParseOptions) -> Self {
        let lexer = Lexer::new(source).unclosed_bracket_recovery();
        let mut parser = Parser {
            source,
            stream: TokenStream::new(TokenSource::new(lexer)),
            previous_end: TextSize::ZERO,
            print_function: options.print_function,
            unicode_literals: options.unicode_literals,
            encoding: SourceEncoding::Utf8,
            suites: Vec::new(),
            speculating: 0,
            regions: 0,
            depth: 0,
            deferred: Vec::new(),
        };
        parser.check_source(options);
        parser
    }

    /// Checks that `compile()` performs before parsing.
    fn check_source(&mut self, options: &ParseOptions) {
        if let Some(index) = self.source.find('\0') {
            let at = TextSize::from(index);
            self.stream.errors().push(ParseError::new(
                ParseErrorKind::NullCharacter,
                "source code cannot contain null bytes",
                TextRange::at(at, TextSize::new(1)),
                Anchor::unpositioned(),
            ));
        }
        if let Some((name, range)) = encoding_declaration(self.source) {
            match options.source_kind {
                SourceKind::Unicode => {
                    self.stream.errors().push(ParseError::new(
                        ParseErrorKind::SyntaxError,
                        "encoding declaration in Unicode string",
                        range,
                        Anchor::unpositioned(),
                    ));
                }
                SourceKind::File => match SourceEncoding::from_name(name) {
                    Some(encoding) => self.encoding = encoding,
                    None => {
                        self.stream.errors().push(ParseError::syntax(
                            format!("unknown encoding: {name}"),
                            range,
                            Anchor::at(TextSize::ZERO),
                        ));
                    }
                },
            }
        }
    }

    /// Parses a module (`file_input`).
    pub(crate) fn parse_module(mut self) -> (Module, Vec<ParseError>, Vec<DroppedText>) {
        let body = self.file_input();
        let range = TextRange::new(TextSize::ZERO, TextSize::of(self.source));
        let module = Module { body, range };
        let (errors, dropped) = self.finish();
        (module, errors, dropped)
    }

    /// Parses an expression (`eval_input`).
    pub(crate) fn parse_expression(
        mut self,
    ) -> (Option<crate::ast::Expr>, Vec<ParseError>, Vec<DroppedText>) {
        let expr = self.eval_input();
        let (errors, dropped) = self.finish();
        (expr, errors, dropped)
    }

    fn finish(self) -> (Vec<ParseError>, Vec<DroppedText>) {
        let lexer = self.stream.source.into_lexer();
        (lexer.errors, lexer.dropped)
    }

    // -- Tokens -------------------------------------------------------------

    #[inline]
    fn lt1(&mut self) -> Token {
        self.stream.lt(1)
    }

    #[inline]
    fn kind(&mut self) -> TokenKind {
        self.stream.lt(1).kind
    }

    #[inline]
    fn peek_kind(&mut self, k: usize) -> TokenKind {
        self.stream.lt(k).kind
    }

    #[inline]
    fn at(&mut self, kind: TokenKind) -> bool {
        self.kind() == kind
    }

    /// Consumes the current token and returns it.
    fn bump(&mut self) -> Token {
        let token = self.lt1();
        if !matches!(
            token.kind,
            TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::EndOfFile
        ) {
            self.previous_end = token.range.end();
        }
        self.stream.consume();
        token
    }

    /// Consumes the current token if it has the given kind.
    fn eat(&mut self, kind: TokenKind) -> Option<Token> {
        if self.at(kind) {
            Some(self.bump())
        } else {
            None
        }
    }

    /// Consumes a token of the given kind or fails at the current token.
    fn expect(&mut self, kind: TokenKind) -> PResult<Token> {
        if self.at(kind) {
            Ok(self.bump())
        } else {
            Err(self.expected(kind.describe()))
        }
    }

    /// The source text of a token.
    fn text(&self, token: &Token) -> &'src str {
        token.range.slice(self.source)
    }

    /// The range from `start` to the end of the last consumed token.
    #[inline]
    fn range_from(&self, start: TextSize) -> TextRange {
        TextRange::new(start.min(self.previous_end), self.previous_end)
    }

    // -- Errors -------------------------------------------------------------

    /// Records a syntax error at the current token. Errors at a lexical
    /// error token are not recorded again.
    fn unexpected(&mut self, message: impl FnOnce(&Token) -> String) -> Failure {
        let token = self.lt1();
        if token.kind == TokenKind::Error || self.speculating > 0 {
            return Failure;
        }
        let mut message = message(&token);
        if let Some(hint) = self.python3_hint(&token) {
            message.push_str(" (");
            message.push_str(hint);
            message.push(')');
        }
        let range = if token.range.is_empty() && token.kind == TokenKind::EndOfFile {
            TextRange::empty(token.range.start())
        } else {
            token.range
        };
        self.record(ParseError::syntax(message, range, token.anchor.as_token()));
        Failure
    }

    /// An explanation when a syntax error at `token` is Python 3 syntax.
    fn python3_hint(&self, token: &Token) -> Option<&'static str> {
        let position = self.stream.position.min(self.stream.tokens.len());
        let before = &self.stream.tokens[..position];
        let line_start = before
            .iter()
            .rposition(|token| {
                matches!(
                    token.kind,
                    TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent | TokenKind::Semi
                )
            })
            .map_or(0, |index| index + 1);
        python3::hint(&python3::Context {
            source: self.source,
            line: &before[line_start..],
            token,
            print_function: self.print_function,
        })
    }

    /// "expected X" at the current token.
    fn expected(&mut self, what: &str) -> Failure {
        let source = self.source;
        self.unexpected(|token| {
            format!(
                "invalid syntax: expected {what}, found {}",
                describe_token(source, token)
            )
        })
    }

    /// "invalid syntax" at the current token.
    fn invalid_syntax(&mut self) -> Failure {
        let source = self.source;
        self.unexpected(|token| {
            format!(
                "invalid syntax: unexpected {}",
                describe_token(source, token)
            )
        })
    }

    /// Records an error that Jython raises from a grammar action (for
    /// example "can't assign to literal"). Parsing continues.
    fn report(&mut self, error: ParseError) {
        if self.speculating > 0 {
            self.deferred.push(error);
        } else {
            self.record(error);
        }
    }

    /// Records an error found by the parser, after the errors of everything
    /// Jython would have read before it.
    fn record(&mut self, error: ParseError) {
        if self.regions > 0 {
            self.deferred.push(error);
        } else {
            self.stream.errors().push(error);
        }
    }

    /// Parses a construct that Jython guards with a syntactic predicate.
    ///
    /// Jython first scans such a construct (and the token after it) without
    /// running its grammar actions, and then parses it again for real. Its
    /// grammar actions, and so the errors they raise, therefore come after
    /// every lexical error in the scanned tokens. This parser parses once and
    /// holds back its own errors until the outermost region ends; `parse`
    /// must consume or look at the token that ends Jython's scan.
    fn region<T>(&mut self, parse: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        self.regions += 1;
        let result = parse(self);
        self.regions -= 1;
        self.flush_deferred();
        result
    }

    /// Moves held-back errors to the error list once no region or
    /// speculation is open.
    fn flush_deferred(&mut self) {
        if self.regions == 0 && self.speculating == 0 && !self.deferred.is_empty() {
            let deferred = std::mem::take(&mut self.deferred);
            self.stream.errors().extend(deferred);
        }
    }

    /// Runs `parse` speculatively, like an ANTLR syntactic predicate. On
    /// failure the token position is restored and nothing is reported; on
    /// success the result is kept along with the errors its grammar actions
    /// raised.
    fn speculate<T>(&mut self, parse: impl FnOnce(&mut Self) -> PResult<T>) -> Option<T> {
        let checkpoint = Checkpoint {
            position: self.stream.position,
            previous_end: self.previous_end,
            deferred: self.deferred.len(),
        };
        self.speculating += 1;
        let result = parse(self);
        self.speculating -= 1;
        match result {
            Ok(value) => {
                self.flush_deferred();
                Some(value)
            }
            Err(Failure) => {
                self.stream.position = checkpoint.position;
                self.previous_end = checkpoint.previous_end;
                self.deferred.truncate(checkpoint.deferred);
                None
            }
        }
    }

    /// Runs a parse function one nesting level deeper, failing when the
    /// expression is nested too deeply.
    fn nested<T>(&mut self, parse: impl FnOnce(&mut Self) -> PResult<T>) -> PResult<T> {
        if self.depth >= MAX_NESTING {
            let token = self.lt1();
            if self.speculating == 0 {
                self.record(ParseError::new(
                    ParseErrorKind::InternalError,
                    "expression nested too deeply",
                    token.range,
                    token.anchor,
                ));
            }
            return Err(Failure);
        }
        self.depth += 1;
        let result = parse(self);
        self.depth -= 1;
        result
    }

    // -- Recovery -----------------------------------------------------------

    /// Skips to the end of the current logical line (consuming its NEWLINE),
    /// or to the end of the enclosing block.
    fn recover_to_line_end(&mut self) {
        loop {
            match self.kind() {
                TokenKind::Newline => {
                    self.bump();
                    return;
                }
                TokenKind::EndOfFile | TokenKind::Dedent | TokenKind::Indent => return,
                _ => {
                    self.bump();
                }
            }
        }
    }
}

/// A short description of a token for error messages.
fn describe_token(source: &str, token: &Token) -> String {
    match token.kind {
        TokenKind::Name => format!("name '{}'", token.range.slice(source)),
        TokenKind::Int
        | TokenKind::LongInt
        | TokenKind::Float
        | TokenKind::Complex
        | TokenKind::String => {
            let text = token.range.slice(source);
            if text.len() > 20 {
                token.kind.describe().to_owned()
            } else {
                format!("{} {text}", token.kind.describe())
            }
        }
        kind => kind.describe().to_owned(),
    }
}

/// Finds a PEP 263 encoding declaration in the first two lines, the way
/// Jython does (`#.*coding[:=]\s*([-\w.]+)` anywhere in the line).
pub(crate) fn encoding_declaration(source: &str) -> Option<(&str, TextRange)> {
    let source_start = if source.starts_with('\u{feff}') { 3 } else { 0 };
    let mut line_start = source_start;
    for _ in 0..2 {
        if line_start > source.len() {
            break;
        }
        let rest = &source[line_start..];
        let line_len = rest.find(['\n', '\r']).unwrap_or(rest.len());
        let line = &rest[..line_len];
        if let Some(found) = match_coding(line) {
            let start = line_start + found.0;
            let name = &line[found.0..found.1];
            return Some((name, TextRange::from(start..line_start + found.1)));
        }
        if line_len == rest.len() {
            break;
        }
        let terminator = if rest[line_len..].starts_with("\r\n") {
            2
        } else {
            1
        };
        line_start += line_len + terminator;
    }
    None
}

/// Returns the byte range of the encoding name in `line`.
fn match_coding(line: &str) -> Option<(usize, usize)> {
    let hash = line.find('#')?;
    let bytes = line.as_bytes();
    let is_name = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.');
    // `.*` is greedy, so the last occurrence that matches wins.
    let mut search_end = line.len();
    while let Some(index) = line[hash + 1..search_end].rfind("coding") {
        let index = hash + 1 + index;
        let mut cursor = index + "coding".len();
        if matches!(bytes.get(cursor), Some(b':' | b'=')) {
            cursor += 1;
            while matches!(bytes.get(cursor), Some(b' ' | b'\t' | b'\x0b' | b'\x0c')) {
                cursor += 1;
            }
            let name_start = cursor;
            while bytes.get(cursor).is_some_and(|&byte| is_name(byte)) {
                cursor += 1;
            }
            if cursor > name_start {
                return Some((name_start, cursor));
            }
        }
        search_end = index;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_encoding_declarations() {
        assert_eq!(
            encoding_declaration("# -*- coding: latin-1 -*-\nx = 1\n").map(|found| found.0),
            Some("latin-1")
        );
        assert_eq!(
            encoding_declaration("#!/usr/bin/env python\n# vim: set fileencoding=utf-8 :\n")
                .map(|found| found.0),
            Some("utf-8")
        );
        assert_eq!(
            encoding_declaration("x = 1\ny = 2\n# coding: utf-8\n"),
            None
        );
        assert_eq!(encoding_declaration("x = '# coding'\n"), None);
    }
}
