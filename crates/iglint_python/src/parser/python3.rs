//! Explanations for syntax errors caused by Python 3 syntax, which is the
//! most common reason Jython 2.7 rejects code that looks fine.
//!
//! The hints only look at tokens that have already been read and at the
//! source text, so they never change which error is reported first.

use crate::token::{Token, TokenKind};

/// The tokens of the current logical line up to the error, the error token
/// and the source.
pub(super) struct Context<'a> {
    pub(super) source: &'a str,
    /// Tokens of the logical line before the error token.
    pub(super) line: &'a [Token],
    pub(super) token: &'a Token,
    pub(super) print_function: bool,
}

impl Context<'_> {
    fn text(&self, token: &Token) -> &str {
        token.range.slice(self.source)
    }

    fn previous(&self) -> Option<&Token> {
        self.line.last()
    }

    fn previous_text(&self) -> Option<&str> {
        self.previous().map(|token| self.text(token))
    }

    fn adjacent_to_previous(&self) -> bool {
        self.previous()
            .is_some_and(|previous| previous.range.end() == self.token.range.start())
    }

    /// The byte right after the error token.
    fn next_byte(&self) -> Option<u8> {
        self.source
            .as_bytes()
            .get(self.token.range.end().to_usize())
            .copied()
    }

    fn first_kind(&self) -> Option<TokenKind> {
        self.line.first().map(|token| token.kind)
    }

    /// Bracket depth at the error token, counted from the line start.
    fn depth(&self) -> i32 {
        self.line.iter().fold(0, |depth, token| match token.kind {
            TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => depth + 1,
            TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => depth - 1,
            _ => depth,
        })
    }

    /// The innermost unclosed bracket before the error token.
    fn open_bracket(&self) -> Option<TokenKind> {
        let mut depth = 0;
        for token in self.line.iter().rev() {
            match token.kind {
                TokenKind::Rpar | TokenKind::Rsqb | TokenKind::Rbrace => depth += 1,
                TokenKind::Lpar | TokenKind::Lsqb | TokenKind::Lbrace => {
                    if depth == 0 {
                        return Some(token.kind);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        None
    }
}

/// An explanation if the error looks like Python 3 syntax.
pub(super) fn hint(context: &Context<'_>) -> Option<&'static str> {
    let token = context.token;
    let previous = context.previous_text();
    match token.kind {
        TokenKind::String if context.adjacent_to_previous() => {
            let prefix = previous?.to_ascii_lowercase();
            if prefix.contains('f') && prefix.len() <= 2 {
                return Some("f-strings are Python 3 syntax; use '%' formatting or str.format()");
            }
            if prefix == "rb" {
                return Some("the string prefix 'rb' is Python 3 syntax; write 'br'");
            }
            None
        }
        TokenKind::Name if previous == Some("nonlocal") => Some(
            "'nonlocal' is Python 3 syntax; Python 2 has no way to rebind an enclosing variable",
        ),
        TokenKind::Def | TokenKind::With | TokenKind::For if previous == Some("async") => {
            Some("'async' is Python 3 syntax")
        }
        TokenKind::Name | TokenKind::Lpar if previous == Some("await") => {
            Some("'await' is Python 3 syntax")
        }
        TokenKind::From if context.previous().map(|token| token.kind) == Some(TokenKind::Yield) => {
            Some("'yield from' is Python 3 syntax")
        }
        TokenKind::From if context.first_kind() == Some(TokenKind::Raise) => {
            Some("'raise ... from ...' is Python 3 syntax")
        }
        TokenKind::Comma | TokenKind::Rpar | TokenKind::Colon
            if context.previous().map(|token| token.kind) == Some(TokenKind::Star) =>
        {
            Some("keyword-only arguments (a bare '*') are Python 3 syntax")
        }
        TokenKind::Colon if context.next_byte() == Some(b'=') => {
            Some("assignment expressions (':=') are Python 3 syntax")
        }
        TokenKind::Colon if context.first_kind() == Some(TokenKind::Def) && context.depth() > 0 => {
            Some("parameter annotations are Python 3 syntax")
        }
        TokenKind::Minus
            if context.first_kind() == Some(TokenKind::Def)
                && context.next_byte() == Some(b'>') =>
        {
            Some("return annotations ('->') are Python 3 syntax")
        }
        TokenKind::Star
            if context.depth() == 0
                && matches!(
                    context.previous().map(|token| token.kind),
                    None | Some(TokenKind::Comma) | Some(TokenKind::Equal)
                ) =>
        {
            Some("starred assignment targets are Python 3 syntax")
        }
        TokenKind::Star | TokenKind::DoubleStar
            if matches!(
                context.open_bracket(),
                Some(TokenKind::Lsqb | TokenKind::Lbrace)
            ) =>
        {
            Some("unpacking inside list, set and dict displays is Python 3 syntax")
        }
        TokenKind::At if !context.line.is_empty() => {
            Some("the '@' (matrix multiplication) operator is Python 3 syntax")
        }
        TokenKind::Dot
            if context.next_byte() == Some(b'.')
                && context.open_bracket() != Some(TokenKind::Lsqb) =>
        {
            Some("'...' outside a subscript is Python 3 syntax")
        }
        TokenKind::Equal
            if context.first_kind() == Some(TokenKind::Print) && !context.print_function =>
        {
            Some(
                "print is a statement in Python 2; keyword arguments such as end= or sep= need \
                 'from __future__ import print_function'",
            )
        }
        TokenKind::Equal if context.first_kind() == Some(TokenKind::Class) => {
            Some("class keyword arguments (metaclass=...) are Python 3 syntax")
        }
        TokenKind::Name
            if context.adjacent_to_previous()
                && context.text(token).starts_with('_')
                && matches!(
                    context.previous().map(|token| token.kind),
                    Some(TokenKind::Int | TokenKind::Float)
                ) =>
        {
            Some("underscores in number literals are Python 3 syntax")
        }
        _ => None,
    }
}
