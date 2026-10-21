//! Indentation: Jython's `PythonTokenSource`, which turns line starts into
//! `INDENT` and `DEDENT` tokens.
//!
//! Like the original it works on the lexer's visible tokens through a
//! one-token window and fetches the next token as soon as it consumes one,
//! which determines when lexical errors surface relative to syntax errors.

use std::collections::VecDeque;

use iglint_text::TextRange;

use crate::anchor::AnchorKind;
use crate::error::{ParseError, ParseErrorKind};
use crate::lexer::Lexer;
use crate::token::{Token, TokenKind};

/// Jython's indentation stack holds 100 levels, including the outermost.
const MAX_INDENTS: usize = 100;

pub(crate) struct TokenSource<'src> {
    lexer: Lexer<'src>,
    /// The lexer stream's `LT(1)`, once fetched.
    current: Option<Token>,
    /// The lexer stream's `LT(-1)`: the last consumed visible token.
    previous: Option<Token>,
    queue: VecDeque<Token>,
    /// Stack of indentation columns; never empty.
    indents: Vec<i32>,
}

impl<'src> TokenSource<'src> {
    pub(crate) fn new(lexer: Lexer<'src>) -> Self {
        TokenSource {
            lexer,
            current: None,
            previous: None,
            queue: VecDeque::new(),
            indents: vec![0],
        }
    }

    pub(crate) fn lexer(&self) -> &Lexer<'src> {
        &self.lexer
    }

    pub(crate) fn lexer_mut(&mut self) -> &mut Lexer<'src> {
        &mut self.lexer
    }

    pub(crate) fn into_lexer(self) -> Lexer<'src> {
        self.lexer
    }

    fn lt1(&mut self) -> Token {
        match self.current {
            Some(token) => token,
            None => {
                let token = self.lexer.next_token();
                self.current = Some(token);
                token
            }
        }
    }

    /// Consumes `LT(1)` and, like ANTLR's `CommonTokenStream`, immediately
    /// fetches the next token.
    fn consume(&mut self) {
        self.previous = self.current.take();
        self.current = Some(self.lexer.next_token());
    }

    /// The next token for the parser.
    pub(crate) fn next_token(&mut self) -> Token {
        loop {
            if let Some(&front) = self.queue.front() {
                if front.kind != TokenKind::EndOfFile {
                    self.queue.pop_front();
                }
                return front;
            }
            self.insert_imaginary_tokens();
        }
    }

    fn insert_imaginary_tokens(&mut self) {
        let token = self.lt1();
        match token.kind {
            TokenKind::EndOfFile => {
                let mut eof = token;
                let previous = self.previous;
                if let Some(previous) = previous {
                    move_eof_to(&mut eof, &previous);
                    self.current = Some(eof);
                }
                match previous.map(|token| token.kind) {
                    None => self.generate_newline(&eof),
                    Some(TokenKind::LeadingWs) => {
                        self.dedent(-1, &eof, false);
                        self.generate_newline(&eof);
                    }
                    Some(TokenKind::Newline) => {}
                    Some(_) => {
                        self.generate_newline(&eof);
                        self.dedent(-1, &eof, false);
                    }
                }
                self.queue.push_back(eof);
            }
            TokenKind::Newline | TokenKind::RecoveryNewline => {
                self.queue.push_back(token);
                let newline = token;
                // After a recovery newline Jython has already failed, so an
                // inconsistent dedent is not reported again.
                let lenient = token.kind == TokenKind::RecoveryNewline;
                self.consume();

                let mut first = self.lt1();
                let column = if first.kind == TokenKind::EndOfFile {
                    move_eof_to(&mut first, &newline);
                    self.current = Some(first);
                    -1
                } else if first.kind == TokenKind::LeadingWs {
                    self.consume();
                    if self.lt1().kind == TokenKind::EndOfFile {
                        return;
                    }
                    first.width as i32
                } else {
                    self.consume();
                    self.token_column(&first)
                };

                let last = self.indents.last().copied().unwrap_or(0);
                if column > last && self.indents.len() >= MAX_INDENTS {
                    // Jython's fixed-size indentation stack overflows.
                    self.lexer.errors.push(ParseError::new(
                        ParseErrorKind::InternalError,
                        "too many levels of indentation",
                        first.range,
                        first.anchor,
                    ));
                } else if column > last {
                    self.indents.push(column);
                    let range = if first.kind == TokenKind::LeadingWs {
                        first.range
                    } else {
                        TextRange::empty(first.range.start())
                    };
                    self.queue
                        .push_back(Token::new(TokenKind::Indent, range, first.anchor));
                } else if column < last {
                    self.dedent(column, &first, lenient);
                }
                if first.kind != TokenKind::LeadingWs {
                    self.queue.push_back(first);
                }
            }
            _ => {
                self.queue.push_back(token);
                self.consume();
            }
        }
    }

    /// Jython's `getCharPositionInLine()` of a token.
    fn token_column(&self, token: &Token) -> i32 {
        match token.anchor.kind() {
            AnchorKind::Offset(offset) => {
                let offset = offset.to_usize();
                if offset > self.lexer.source().len() {
                    0
                } else {
                    self.lexer.column_of(offset)
                }
            }
            AnchorKind::MultilineString(_) | AnchorKind::MultilineStringToken(_) => -2,
            AnchorKind::Synthetic => -1,
            AnchorKind::Unpositioned => 0,
            AnchorKind::LowSurrogate(offset) => self.lexer.column_of(offset.to_usize()) + 1,
        }
    }

    fn generate_newline(&mut self, at: &Token) {
        self.queue.push_back(Token::new(
            TokenKind::Newline,
            TextRange::empty(at.range.start()),
            at.anchor,
        ));
    }

    /// Pops the indentation stack down to `column`, emitting one `DEDENT`
    /// per level. `column` -1 or -2 dedents to the outermost level.
    fn dedent(&mut self, column: i32, at: &Token, lenient: bool) {
        let target = match self.indents.iter().rposition(|&level| level == column) {
            Some(index) if index + 1 < self.indents.len() => index,
            // Equal to the top level: nothing to pop. (Callers only dedent to
            // smaller columns, but a stack with duplicate levels cannot
            // occur either way.)
            Some(index) => index,
            None if column < 0 => 0,
            None if lenient => self
                .indents
                .iter()
                .rposition(|&level| level < column)
                .unwrap_or(0),
            None => {
                let error = ParseError::new(
                    ParseErrorKind::IndentationError,
                    "unindent does not match any outer indentation level",
                    TextRange::empty(at.range.end()),
                    at.anchor,
                );
                self.lexer.errors.push(error);
                // Recover by dedenting to the closest enclosing level.
                self.indents
                    .iter()
                    .rposition(|&level| level < column)
                    .unwrap_or(0)
            }
        };
        let position = if at.kind == TokenKind::LeadingWs {
            at.range.end()
        } else {
            at.range.start()
        };
        while self.indents.len() > target + 1 {
            self.indents.pop();
            self.queue.push_back(Token::new(
                TokenKind::Dedent,
                TextRange::empty(position),
                at.anchor,
            ));
        }
    }
}

/// Jython's `handleEOF`: the end-of-file token takes the position of the
/// token before it.
fn move_eof_to(eof: &mut Token, previous: &Token) {
    eof.anchor = previous.anchor;
    eof.range = TextRange::empty(previous.range.end());
}
