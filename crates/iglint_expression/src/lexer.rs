//! Lexer reproducing Ignition's `ELLexer` (JFlex spec `HMIEL.lex`).

use std::fmt;

use iglint_text::{TextRange, TextSize};

use crate::tables::{is_ident_part, is_ident_start};

/// Token kinds. Keywords are case-insensitive (`%ignorecase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    LParen,
    RParen,
    Comma,
    LBracket,
    RBracket,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Bang,
    Tilde,
    AmpAmp,
    PipePipe,
    Amp,
    Pipe,
    /// The keyword `xor`.
    Xor,
    Shl,
    Shr,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    /// The keyword `like`.
    Like,
    /// `true` or `false`.
    Bool,
    /// `null`, `none` (any case).
    Null,
    Ident,
    /// Decimal or hexadecimal integer (Java `long`).
    Integer,
    /// Floating point literal (Java `double`).
    Float,
    /// String literal in double or single quotes.
    String,
    /// `{...}`: a property or tag reference.
    Reference,
    Eof,
}

impl TokenKind {
    /// Human-readable name for messages.
    pub fn describe(self) -> &'static str {
        match self {
            TokenKind::LParen => "`(`",
            TokenKind::RParen => "`)`",
            TokenKind::Comma => "`,`",
            TokenKind::LBracket => "`[`",
            TokenKind::RBracket => "`]`",
            TokenKind::Plus => "`+`",
            TokenKind::Minus => "`-`",
            TokenKind::Star => "`*`",
            TokenKind::Slash => "`/`",
            TokenKind::Percent => "`%`",
            TokenKind::Caret => "`^`",
            TokenKind::Bang => "`!`",
            TokenKind::Tilde => "`~`",
            TokenKind::AmpAmp => "`&&`",
            TokenKind::PipePipe => "`||`",
            TokenKind::Amp => "`&`",
            TokenKind::Pipe => "`|`",
            TokenKind::Xor => "`xor`",
            TokenKind::Shl => "`<<`",
            TokenKind::Shr => "`>>`",
            TokenKind::Eq => "`=`",
            TokenKind::NotEq => "`!=`",
            TokenKind::Lt => "`<`",
            TokenKind::LtEq => "`<=`",
            TokenKind::Gt => "`>`",
            TokenKind::GtEq => "`>=`",
            TokenKind::Like => "`like`",
            TokenKind::Bool => "boolean literal",
            TokenKind::Null => "`null`",
            TokenKind::Ident => "identifier",
            TokenKind::Integer => "integer literal",
            TokenKind::Float => "number literal",
            TokenKind::String => "string literal",
            TokenKind::Reference => "reference",
            TokenKind::Eof => "end of expression",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// The value carried by literal tokens.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenValue {
    None,
    Bool(bool),
    Integer(i64),
    Float(f64),
    /// Decoded string literal, or the raw path of a reference.
    Text(String),
}

/// A token with its byte range in the expression source.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: TextRange,
    pub value: TokenValue,
}

/// A lexical error. Ignition's lexer throws on the first one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LexErrorKind {
    /// A character that cannot start a token (`.`, `#`, `?`, `;`, `@`, a stray
    /// `}`, vertical tab, non-breaking space, any supplementary character, ...).
    IllegalCharacter(char),
    /// A backslash escape other than `\t \n \r \\` and the quote of the
    /// literal (`\"` or `\'`).
    IllegalEscape,
    /// A line break inside a string literal.
    UnterminatedString,
    /// A line break inside `{...}`.
    UnterminatedReference,
    /// `{` inside `{...}`.
    NestedReference,
    /// An integer literal outside the Java `long` range.
    IntegerOverflow,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexErrorKind::IllegalCharacter(c) => write!(f, "illegal character {c:?}"),
            LexErrorKind::IllegalEscape => f.write_str("illegal escape sequence"),
            LexErrorKind::UnterminatedString => f.write_str("unterminated string at end of line"),
            LexErrorKind::UnterminatedReference => {
                f.write_str("unterminated property or tag reference at end of line")
            }
            LexErrorKind::NestedReference => f.write_str("nested `{` inside a reference"),
            LexErrorKind::IntegerOverflow => f.write_str("integer literal out of range"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub range: TextRange,
    pub kind: LexErrorKind,
    /// Start of the token being scanned when the error occurred. Ignition's
    /// lexer runs lazily, so the error surfaces when the parser asks for
    /// that token, before any syntax error at the same token.
    pub token_start: TextSize,
}

/// A string literal or reference left open at the very end of the input.
///
/// Ignition's lexer silently discards it: `1 + 2 "abc` evaluates to `3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedTail {
    pub range: TextRange,
    pub kind: DroppedTailKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DroppedTailKind {
    String,
    Reference,
}

/// Output of [`lex`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lexed {
    /// Tokens, ending with [`TokenKind::Eof`].
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
    /// `//` comments, from the slashes to the end of the line.
    pub comments: Vec<TextRange>,
    pub dropped_tail: Option<DroppedTail>,
}

#[inline]
fn size(offset: usize) -> TextSize {
    TextSize::from(offset)
}

#[inline]
fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(size(start), size(end))
}

/// Tokenizes an expression.
pub fn lex(source: &str) -> Lexed {
    let mut lx = Lexer {
        src: source,
        bytes: source.as_bytes(),
        pos: 0,
        token_start: 0,
        out: Lexed::default(),
    };
    lx.run();
    let end = size(source.len());
    lx.out.tokens.push(Token {
        kind: TokenKind::Eof,
        range: TextRange::empty(end),
        value: TokenValue::None,
    });
    lx.out
}

struct Lexer<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    token_start: usize,
    out: Lexed,
}

impl Lexer<'_> {
    fn push(&mut self, kind: TokenKind, start: usize, value: TokenValue) {
        self.out.tokens.push(Token {
            kind,
            range: range(start, self.pos),
            value,
        });
    }

    fn error(&mut self, start: usize, end: usize, kind: LexErrorKind) {
        self.out.errors.push(LexError {
            range: range(start, end),
            kind,
            token_start: size(self.token_start),
        });
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.pos + offset).copied()
    }

    fn run(&mut self) {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            self.token_start = start;
            let b = self.bytes[start];
            match b {
                b' ' | b'\t' | b'\x0C' | b'\r' | b'\n' => self.pos += 1,
                b'(' => self.single(TokenKind::LParen),
                b')' => self.single(TokenKind::RParen),
                b',' => self.single(TokenKind::Comma),
                b'[' => self.single(TokenKind::LBracket),
                b']' => self.single(TokenKind::RBracket),
                b'+' => self.single(TokenKind::Plus),
                b'-' => self.single(TokenKind::Minus),
                b'*' => self.single(TokenKind::Star),
                b'%' => self.single(TokenKind::Percent),
                b'^' => self.single(TokenKind::Caret),
                b'~' => self.single(TokenKind::Tilde),
                b'=' => self.single(TokenKind::Eq),
                b'/' => {
                    if self.peek_at(1) == Some(b'/') {
                        let mut end = start + 2;
                        while end < self.bytes.len() && !matches!(self.bytes[end], b'\n' | b'\r') {
                            end += 1;
                        }
                        self.pos = end;
                        self.out.comments.push(range(start, end));
                    } else {
                        self.single(TokenKind::Slash);
                    }
                }
                b'!' => self.one_or_two(b'=', TokenKind::NotEq, TokenKind::Bang),
                b'&' => self.one_or_two(b'&', TokenKind::AmpAmp, TokenKind::Amp),
                b'|' => self.one_or_two(b'|', TokenKind::PipePipe, TokenKind::Pipe),
                b'<' => match self.peek_at(1) {
                    Some(b'<') => self.double(TokenKind::Shl),
                    Some(b'=') => self.double(TokenKind::LtEq),
                    _ => self.single(TokenKind::Lt),
                },
                b'>' => match self.peek_at(1) {
                    Some(b'>') => self.double(TokenKind::Shr),
                    Some(b'=') => self.double(TokenKind::GtEq),
                    _ => self.single(TokenKind::Gt),
                },
                b'"' | b'\'' => self.string(b),
                b'{' => self.reference(),
                b'0'..=b'9' => self.number(),
                b'.' if self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) => self.number(),
                _ => {
                    let c = self.src[start..].chars().next().expect("in bounds");
                    if is_ident_start(c) {
                        self.ident();
                    } else {
                        self.pos += c.len_utf8();
                        self.error(start, self.pos, LexErrorKind::IllegalCharacter(c));
                    }
                }
            }
        }
    }

    fn single(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 1;
        self.push(kind, start, TokenValue::None);
    }

    fn double(&mut self, kind: TokenKind) {
        let start = self.pos;
        self.pos += 2;
        self.push(kind, start, TokenValue::None);
    }

    fn one_or_two(&mut self, second: u8, two: TokenKind, one: TokenKind) {
        if self.peek_at(1) == Some(second) {
            self.double(two);
        } else {
            self.single(one);
        }
    }

    fn ident(&mut self) {
        let start = self.pos;
        let mut end = start;
        for (i, c) in self.src[start..].char_indices() {
            let ok = if i == 0 {
                is_ident_start(c)
            } else {
                is_ident_part(c)
            };
            if !ok {
                break;
            }
            end = start + i + c.len_utf8();
        }
        self.pos = end;
        let text = &self.src[start..end];
        let (kind, value) = if text.eq_ignore_ascii_case("true") {
            (TokenKind::Bool, TokenValue::Bool(true))
        } else if text.eq_ignore_ascii_case("false") {
            (TokenKind::Bool, TokenValue::Bool(false))
        } else if text.eq_ignore_ascii_case("null") || text.eq_ignore_ascii_case("none") {
            (TokenKind::Null, TokenValue::None)
        } else if text.eq_ignore_ascii_case("xor") {
            (TokenKind::Xor, TokenValue::None)
        } else if text.eq_ignore_ascii_case("like") {
            (TokenKind::Like, TokenValue::None)
        } else {
            (TokenKind::Ident, TokenValue::Text(text.to_owned()))
        };
        self.push(kind, start, value);
    }

    /// Numbers by JFlex longest match over the rules, in spec order on ties:
    /// `DecLongLiteral = 0 | [1-9][0-9]*`, `DoubleLiteral = ([0-9]+ \. [0-9]*
    /// | \. [0-9]+ | [0-9]+) ([eE] [+-]? [0-9]+)?`, `HexLiteral =
    /// 0x[0-9A-Fa-f]+` (case-insensitive).
    fn number(&mut self) {
        let b = self.bytes;
        let start = self.pos;
        let digits_from = |mut i: usize| {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            i
        };
        // DecLongLiteral
        let dec_end = match b[start] {
            b'0' => start + 1,
            b'1'..=b'9' => digits_from(start + 1),
            _ => start,
        };
        // DoubleLiteral
        let int_end = digits_from(start);
        let mut mantissa_end = int_end;
        if int_end > start {
            if b.get(int_end) == Some(&b'.') {
                mantissa_end = digits_from(int_end + 1);
            }
        } else {
            // `\.[0-9]+` (the caller checked the digit)
            mantissa_end = digits_from(start + 1);
        }
        let mut double_end = mantissa_end;
        if matches!(b.get(mantissa_end), Some(b'e' | b'E')) {
            let mut i = mantissa_end + 1;
            if matches!(b.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            let exp_end = digits_from(i);
            if exp_end > i {
                double_end = exp_end;
            }
        }
        // HexLiteral
        let mut hex_end = start;
        if b[start] == b'0' && matches!(b.get(start + 1), Some(b'x' | b'X')) {
            let mut i = start + 2;
            while i < b.len() && b[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i > start + 2 {
                hex_end = i;
            }
        }
        if hex_end > dec_end.max(double_end) {
            self.pos = hex_end;
            let value = i64::from_str_radix(&self.src[start + 2..hex_end], 16);
            self.integer(start, value.ok());
        } else if dec_end >= double_end {
            self.pos = dec_end;
            let value = self.src[start..dec_end].parse::<i64>();
            self.integer(start, value.ok());
        } else {
            self.pos = double_end;
            let text = &self.src[start..double_end];
            // Java accepts `5.` and `.5`; Rust's parser does too.
            let value = text.parse::<f64>().unwrap_or(f64::NAN);
            self.push(TokenKind::Float, start, TokenValue::Float(value));
        }
    }

    fn integer(&mut self, start: usize, value: Option<i64>) {
        match value {
            Some(v) => self.push(TokenKind::Integer, start, TokenValue::Integer(v)),
            None => {
                self.error(start, self.pos, LexErrorKind::IntegerOverflow);
                self.push(TokenKind::Integer, start, TokenValue::Integer(0));
            }
        }
    }

    /// `"..."` or `'...'`. Escapes: `\t \n \r \\` and the literal's own quote,
    /// matched case-insensitively (`\T` is a tab).
    fn string(&mut self, quote: u8) {
        let b = self.bytes;
        let start = self.pos;
        let mut i = start + 1;
        let mut value = String::new();
        let mut run = i;
        loop {
            let Some(&c) = b.get(i) else {
                // End of input inside the literal: the lexer returns EOF and
                // the literal is discarded.
                self.pos = b.len();
                self.out.dropped_tail = Some(DroppedTail {
                    range: range(start, b.len()),
                    kind: DroppedTailKind::String,
                });
                return;
            };
            if c == quote {
                value.push_str(&self.src[run..i]);
                self.pos = i + 1;
                self.push(TokenKind::String, start, TokenValue::Text(value));
                return;
            }
            match c {
                b'\n' | b'\r' => {
                    value.push_str(&self.src[run..i]);
                    // Reported at the line break, where Ignition detects it.
                    self.error(i, i + 1, LexErrorKind::UnterminatedString);
                    self.pos = i;
                    self.push(TokenKind::String, start, TokenValue::Text(value));
                    return;
                }
                b'\\' => {
                    value.push_str(&self.src[run..i]);
                    let next = self.src[i + 1..].chars().next();
                    let decoded = match next {
                        Some('t' | 'T') => Some('\t'),
                        Some('n' | 'N') => Some('\n'),
                        Some('r' | 'R') => Some('\r'),
                        Some('\\') => Some('\\'),
                        Some(q) if q as u32 == quote as u32 => Some(q),
                        _ => None,
                    };
                    match (decoded, next) {
                        (Some(d), _) => {
                            value.push(d);
                            i += 2;
                        }
                        (None, Some(n)) if n != '\n' && n != '\r' => {
                            self.error(i, i + 1 + n.len_utf8(), LexErrorKind::IllegalEscape);
                            i += 1 + n.len_utf8();
                        }
                        _ => {
                            // A backslash before a line break or at the end
                            // of input matches no string rule.
                            self.error(i, i + 1, LexErrorKind::IllegalCharacter('\\'));
                            i += 1;
                        }
                    }
                    run = i;
                }
                _ => i += 1,
            }
        }
    }

    /// `{...}`: anything but `{`, `}` and line breaks, taken verbatim.
    fn reference(&mut self) {
        let b = self.bytes;
        let start = self.pos;
        let mut i = start + 1;
        loop {
            match b.get(i) {
                None => {
                    self.pos = b.len();
                    self.out.dropped_tail = Some(DroppedTail {
                        range: range(start, b.len()),
                        kind: DroppedTailKind::Reference,
                    });
                    return;
                }
                Some(b'}') => {
                    self.pos = i + 1;
                    let path = self.src[start + 1..i].to_owned();
                    self.push(TokenKind::Reference, start, TokenValue::Text(path));
                    return;
                }
                Some(b'\n' | b'\r') => {
                    self.error(i, i + 1, LexErrorKind::UnterminatedReference);
                    self.pos = i;
                    let path = self.src[start + 1..i].to_owned();
                    self.push(TokenKind::Reference, start, TokenValue::Text(path));
                    return;
                }
                Some(b'{') => {
                    self.error(i, i + 1, LexErrorKind::NestedReference);
                    i += 1;
                }
                Some(_) => i += 1,
            }
        }
    }
}
