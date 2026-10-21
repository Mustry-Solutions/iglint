//! Parse errors and warnings.

use std::fmt;

use iglint_text::{Ranged, TextRange};

use crate::anchor::{Anchor, JythonLocation, Locator};

/// The exception Jython raises for an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParseErrorKind {
    /// `SyntaxError`.
    SyntaxError,
    /// `IndentationError` (a dedent to a column that matches no enclosing
    /// block).
    IndentationError,
    /// `TypeError`: the source contains a NUL character, which `compile()`
    /// rejects before parsing.
    NullCharacter,
    /// Jython's parser fails with an internal Java exception (for example a
    /// `NumberFormatException` on the literal `0o`). The code cannot be
    /// compiled either way.
    InternalError,
}

/// An error that makes Jython reject the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Where the problem is in the source.
    pub range: TextRange,
    pub message: String,
    pub kind: ParseErrorKind,
    /// The position Jython reports for this error.
    pub(crate) anchor: Anchor,
}

impl ParseError {
    pub(crate) fn new(
        kind: ParseErrorKind,
        message: impl Into<String>,
        range: TextRange,
        anchor: Anchor,
    ) -> Self {
        ParseError {
            range,
            message: message.into(),
            kind,
            anchor,
        }
    }

    pub(crate) fn syntax(message: impl Into<String>, range: TextRange, anchor: Anchor) -> Self {
        ParseError::new(ParseErrorKind::SyntaxError, message, range, anchor)
    }

    /// The line and column Jython reports for this error (`SyntaxError.lineno`
    /// and `SyntaxError.offset`).
    pub fn jython_location(&self, locator: &Locator<'_>) -> JythonLocation {
        match self.kind {
            ParseErrorKind::InternalError | ParseErrorKind::NullCharacter => {
                JythonLocation { line: 0, column: 0 }
            }
            _ => locator.anchor_location(self.anchor),
        }
    }

    /// The token Jython's parser or lexer was at when it failed. For
    /// [`ParseErrorKind::InternalError`] this is where the failure started,
    /// although Jython itself reports no position then.
    pub fn anchor(&self) -> Anchor {
        self.anchor
    }
}

impl Ranged for ParseError {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// Source text that Jython's lexer silently discards.
///
/// Jython's ANTLR lexer recovers from some malformed tokens by dropping the
/// characters it has read so far plus the next one, without reporting an
/// error. The rest of the file is then parsed as if that text were not there,
/// which can hide real mistakes (an unterminated string swallows the end of
/// its line, for example). The parser mirrors this exactly and reports every
/// such place here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedText {
    /// The discarded text.
    pub range: TextRange,
    /// Why the lexer gave up on it.
    pub reason: DropReason,
}

/// Why Jython's lexer dropped some text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropReason {
    /// A string literal without its closing quote.
    UnterminatedString,
    /// A backslash that is not followed by a newline.
    StrayBackslash,
    /// A carriage return or form feed that is not followed by a newline.
    StrayLineBreak,
    /// A `!` that is not followed by `=`.
    StrayExclamation,
}

impl DropReason {
    pub fn message(self) -> &'static str {
        match self {
            DropReason::UnterminatedString => {
                "unterminated string literal; Jython silently drops it and the next character"
            }
            DropReason::StrayBackslash => {
                "backslash not followed by a newline; Jython silently drops it and the next character"
            }
            DropReason::StrayLineBreak => {
                "carriage return or form feed not followed by a newline; Jython silently drops it"
            }
            DropReason::StrayExclamation => {
                "'!' not followed by '='; Jython silently drops it and the next character"
            }
        }
    }
}

impl Ranged for DroppedText {
    fn range(&self) -> TextRange {
        self.range
    }
}
