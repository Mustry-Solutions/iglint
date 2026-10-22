//! Hand-written strict JSON parser over `&str`.

use std::borrow::Cow;
use std::fmt;

use iglint_text::{TextRange, TextSize};

use crate::string::{self, StringError};
use crate::value::{Array, JsonString, Member, Number, Object, Value, ValueKind};

/// Parser settings. The defaults accept exactly RFC 8259 plus what Ignition
/// can write: a leading UTF-8 byte order mark and the Gson literals `NaN`,
/// `Infinity` and `-Infinity`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseOptions {
    /// Accept `NaN`, `Infinity` and `-Infinity` as numbers. Gson writes them
    /// when special floating point serialization is enabled.
    pub allow_special_floats: bool,
    /// Maximum nesting depth of arrays and objects. Deeper input is an error
    /// rather than a stack overflow.
    pub max_depth: usize,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            allow_special_floats: true,
            max_depth: 512,
        }
    }
}

impl ParseOptions {
    /// RFC 8259 only (the byte order mark is still skipped).
    pub fn strict() -> Self {
        ParseOptions {
            allow_special_floats: false,
            ..ParseOptions::default()
        }
    }
}

/// A syntax error with the byte range it applies to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub range: TextRange,
    pub kind: ParseErrorKind,
}

/// The reason for a [`ParseError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// The input ended where `expected` was required.
    UnexpectedEof { expected: Expected },
    /// A character that cannot start or continue the current construct.
    UnexpectedCharacter { expected: Expected },
    /// A malformed number such as `01`, `1.`, `-` or `1e`.
    InvalidNumber,
    /// A `\` escape other than `\" \\ \/ \b \f \n \r \t \uXXXX`.
    InvalidEscape,
    /// `\u` without four hex digits.
    InvalidUnicodeEscape,
    /// A raw control character (U+0000 to U+001F) inside a string.
    ControlCharacterInString,
    /// A string without a closing quote.
    UnterminatedString,
    /// A comma directly before `]` or `}`.
    TrailingComma,
    /// Anything but whitespace after the root value.
    TrailingContent,
    /// Arrays and objects nested deeper than [`ParseOptions::max_depth`].
    NestingTooDeep,
    /// Input of 4 GiB or more.
    InputTooLarge,
}

/// What the parser expected at an error position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    Value,
    Key,
    Colon,
    CommaOrCloseBracket,
    CommaOrCloseBrace,
    StringEnd,
}

impl fmt::Display for Expected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Expected::Value => "a value",
            Expected::Key => "a string key",
            Expected::Colon => "`:`",
            Expected::CommaOrCloseBracket => "`,` or `]`",
            Expected::CommaOrCloseBrace => "`,` or `}`",
            Expected::StringEnd => "`\"`",
        })
    }
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::UnexpectedEof { expected } => {
                write!(f, "unexpected end of input, expected {expected}")
            }
            ParseErrorKind::UnexpectedCharacter { expected } => {
                write!(f, "unexpected character, expected {expected}")
            }
            ParseErrorKind::InvalidNumber => f.write_str("invalid number"),
            ParseErrorKind::InvalidEscape => f.write_str("invalid escape sequence"),
            ParseErrorKind::InvalidUnicodeEscape => {
                f.write_str("invalid unicode escape, expected four hex digits")
            }
            ParseErrorKind::ControlCharacterInString => {
                f.write_str("control character in string must be escaped")
            }
            ParseErrorKind::UnterminatedString => f.write_str("unterminated string"),
            ParseErrorKind::TrailingComma => f.write_str("trailing comma"),
            ParseErrorKind::TrailingContent => f.write_str("unexpected content after the value"),
            ParseErrorKind::NestingTooDeep => f.write_str("nesting too deep"),
            ParseErrorKind::InputTooLarge => f.write_str("input larger than 4 GiB"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.range)
    }
}

impl std::error::Error for ParseError {}

/// Parses a complete JSON document with the default [`ParseOptions`].
pub fn parse(text: &str) -> Result<Value<'_>, ParseError> {
    parse_with(text, ParseOptions::default())
}

/// Parses a complete JSON document.
pub fn parse_with(text: &str, options: ParseOptions) -> Result<Value<'_>, ParseError> {
    if text.len() >= u32::MAX as usize {
        return Err(ParseError {
            range: TextRange::default(),
            kind: ParseErrorKind::InputTooLarge,
        });
    }
    let mut p = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
        options,
        depth: 0,
        elements: Vec::new(),
        members: Vec::new(),
    };
    if text.starts_with('\u{FEFF}') {
        p.pos = 3;
    }
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        let end = p.char_end(p.pos);
        return Err(p.error_at(p.pos, end, ParseErrorKind::TrailingContent));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    options: ParseOptions,
    depth: usize,
    /// Elements and members of the arrays and objects being parsed, nested
    /// ones on top. Each container takes its own off the top into a vector
    /// of exactly the right size: no growth while parsing, no spare
    /// capacity kept in the tree.
    elements: Vec<Value<'a>>,
    members: Vec<Member<'a>>,
}

/// Bytes that end a fast string scan: `"`, `\` and control characters.
static STRING_STOP: [bool; 256] = {
    let mut table = [false; 256];
    let mut i = 0;
    while i < 0x20 {
        table[i] = true;
        i += 1;
    }
    table[b'"' as usize] = true;
    table[b'\\' as usize] = true;
    table
};

#[inline]
fn size(offset: usize) -> TextSize {
    TextSize::new(offset as u32)
}

#[inline]
fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(size(start), size(end))
}

impl<'a> Parser<'a> {
    fn error_at(&self, start: usize, end: usize, kind: ParseErrorKind) -> ParseError {
        ParseError {
            range: range(start, end),
            kind,
        }
    }

    /// End of the character starting at `at` (for error ranges).
    fn char_end(&self, at: usize) -> usize {
        let mut end = at + 1;
        while end < self.bytes.len() && !self.text.is_char_boundary(end) {
            end += 1;
        }
        end.min(self.bytes.len())
    }

    fn unexpected(&self, expected: Expected) -> ParseError {
        if self.pos >= self.bytes.len() {
            self.error_at(
                self.pos,
                self.pos,
                ParseErrorKind::UnexpectedEof { expected },
            )
        } else {
            self.error_at(
                self.pos,
                self.char_end(self.pos),
                ParseErrorKind::UnexpectedCharacter { expected },
            )
        }
    }

    #[inline]
    fn skip_ws(&mut self) {
        let bytes = self.bytes;
        let mut pos = self.pos;
        while pos < bytes.len() {
            match bytes[pos] {
                b' ' | b'\n' | b'\r' | b'\t' => pos += 1,
                _ => break,
            }
        }
        self.pos = pos;
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn value(&mut self) -> Result<Value<'a>, ParseError> {
        let start = self.pos;
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => {
                let s = self.string()?;
                Ok(Value {
                    range: s.range(),
                    kind: ValueKind::String(s),
                })
            }
            Some(b't') => self.literal("true", ValueKind::Bool(true)),
            Some(b'f') => self.literal("false", ValueKind::Bool(false)),
            Some(b'n') => self.literal("null", ValueKind::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'N') if self.options.allow_special_floats => self.special("NaN", start),
            Some(b'I') if self.options.allow_special_floats => self.special("Infinity", start),
            _ => Err(self.unexpected(Expected::Value)),
        }
    }

    fn literal(
        &mut self,
        word: &'static str,
        kind: ValueKind<'a>,
    ) -> Result<Value<'a>, ParseError> {
        let start = self.pos;
        if self.bytes[start..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(Value {
                range: range(start, self.pos),
                kind,
            })
        } else {
            Err(self.unexpected(Expected::Value))
        }
    }

    fn special(&mut self, word: &'static str, start: usize) -> Result<Value<'a>, ParseError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(Value {
                range: range(start, self.pos),
                kind: ValueKind::Number(Number::new(&self.text[start..self.pos])),
            })
        } else {
            Err(self.unexpected(Expected::Value))
        }
    }

    fn number(&mut self) -> Result<Value<'a>, ParseError> {
        let bytes = self.bytes;
        let start = self.pos;
        let mut pos = start;
        if bytes[pos] == b'-' {
            pos += 1;
            if self.options.allow_special_floats && bytes[pos..].starts_with(b"Infinity") {
                self.pos = pos;
                return self.special("Infinity", start);
            }
        }
        let invalid = |end: usize| ParseError {
            range: range(start, end.max(start + 1).min(bytes.len().max(start))),
            kind: ParseErrorKind::InvalidNumber,
        };
        match bytes.get(pos) {
            Some(b'0') => pos += 1,
            Some(b'1'..=b'9') => {
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            _ => return Err(invalid(pos)),
        }
        if bytes.get(pos) == Some(&b'.') {
            pos += 1;
            let digits = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos == digits {
                return Err(invalid(pos));
            }
        }
        if matches!(bytes.get(pos), Some(b'e' | b'E')) {
            pos += 1;
            if matches!(bytes.get(pos), Some(b'+' | b'-')) {
                pos += 1;
            }
            let digits = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                pos += 1;
            }
            if pos == digits {
                return Err(invalid(pos));
            }
        }
        // `01` and `1a` are not numbers followed by something else: report
        // the whole run as one invalid number.
        if pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'.') {
            let mut end = pos;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'.') {
                end += 1;
            }
            return Err(invalid(end));
        }
        self.pos = pos;
        Ok(Value {
            range: range(start, pos),
            kind: ValueKind::Number(Number::new(&self.text[start..pos])),
        })
    }

    fn string(&mut self) -> Result<JsonString<'a>, ParseError> {
        let bytes = self.bytes;
        let start = self.pos;
        let content_start = start + 1;
        let mut pos = content_start;
        let mut has_escape = false;
        loop {
            while pos < bytes.len() && !STRING_STOP[bytes[pos] as usize] {
                pos += 1;
            }
            match bytes.get(pos) {
                None => {
                    return Err(self.error_at(
                        start,
                        bytes.len(),
                        ParseErrorKind::UnterminatedString,
                    ));
                }
                Some(b'"') => break,
                Some(b'\\') => {
                    has_escape = true;
                    // Skip the escaped byte so `\"` does not end the string.
                    // Validation happens in `decode`.
                    pos += 2;
                    if pos > bytes.len() {
                        return Err(self.error_at(
                            start,
                            bytes.len(),
                            ParseErrorKind::UnterminatedString,
                        ));
                    }
                }
                Some(_) => {
                    return Err(self.error_at(
                        pos,
                        pos + 1,
                        ParseErrorKind::ControlCharacterInString,
                    ));
                }
            }
        }
        // `pos` may have stepped into the middle of a multi-byte character
        // after a backslash; the closing quote is always a boundary.
        let raw = &self.text[content_start..pos];
        self.pos = pos + 1;
        let value = if has_escape {
            match string::decode(raw) {
                Ok(s) => Cow::Owned(s),
                Err(StringError::InvalidEscape { at, len }) => {
                    let s = content_start + at;
                    return Err(self.error_at(s, s + len, ParseErrorKind::InvalidEscape));
                }
                Err(StringError::InvalidUnicodeEscape { at, len }) => {
                    let s = content_start + at;
                    return Err(self.error_at(s, s + len, ParseErrorKind::InvalidUnicodeEscape));
                }
            }
        } else {
            Cow::Borrowed(raw)
        };
        Ok(JsonString::new(range(start, self.pos), raw, value))
    }

    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > self.options.max_depth {
            return Err(self.error_at(self.pos, self.pos + 1, ParseErrorKind::NestingTooDeep));
        }
        Ok(())
    }

    fn array(&mut self) -> Result<Value<'a>, ParseError> {
        let start = self.pos;
        self.enter()?;
        self.pos += 1;
        let first = self.elements.len();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
        } else {
            loop {
                let value = self.value()?;
                self.elements.push(value);
                self.skip_ws();
                match self.peek() {
                    Some(b',') => {
                        let comma = self.pos;
                        self.pos += 1;
                        self.skip_ws();
                        if self.peek() == Some(b']') {
                            return Err(self.error_at(
                                comma,
                                comma + 1,
                                ParseErrorKind::TrailingComma,
                            ));
                        }
                    }
                    Some(b']') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return Err(self.unexpected(Expected::CommaOrCloseBracket)),
                }
            }
        }
        self.depth -= 1;
        let elements = self.elements.split_off(first);
        Ok(Value {
            range: range(start, self.pos),
            kind: ValueKind::Array(Array { elements }),
        })
    }

    fn object(&mut self) -> Result<Value<'a>, ParseError> {
        let start = self.pos;
        self.enter()?;
        self.pos += 1;
        let first = self.members.len();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
        } else {
            loop {
                if self.peek() != Some(b'"') {
                    return Err(self.unexpected(Expected::Key));
                }
                let key = self.string()?;
                self.skip_ws();
                if self.peek() != Some(b':') {
                    return Err(self.unexpected(Expected::Colon));
                }
                self.pos += 1;
                self.skip_ws();
                let value = self.value()?;
                self.members.push(Member { key, value });
                self.skip_ws();
                match self.peek() {
                    Some(b',') => {
                        let comma = self.pos;
                        self.pos += 1;
                        self.skip_ws();
                        if self.peek() == Some(b'}') {
                            return Err(self.error_at(
                                comma,
                                comma + 1,
                                ParseErrorKind::TrailingComma,
                            ));
                        }
                    }
                    Some(b'}') => {
                        self.pos += 1;
                        break;
                    }
                    _ => return Err(self.unexpected(Expected::CommaOrCloseBrace)),
                }
            }
        }
        self.depth -= 1;
        let members = self.members.split_off(first);
        Ok(Value {
            range: range(start, self.pos),
            kind: ValueKind::Object(Object { members }),
        })
    }
}
