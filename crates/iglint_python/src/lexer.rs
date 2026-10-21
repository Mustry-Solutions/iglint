//! The lexer: a hand-written equivalent of the ANTLR lexer that Jython
//! generates from `Python.g`.
//!
//! It reproduces the generated lexer's decisions, including its oddities:
//!
//! * `compile()` appends `"\n\n"` to every source; the lexer reads those two
//!   virtual newlines after the end of the text.
//! * Token prediction looks ahead like ANTLR's DFA, so `1e`, `0x` or `08` are
//!   errors at the first character that cannot continue a number.
//! * Some malformed tokens are dropped silently together with the character
//!   that broke them (an unterminated single-quoted string, a backslash that
//!   is not followed by a newline, `!` without `=`, a `\r` or form feed that
//!   is not followed by `\n`). These are recorded as [`DroppedText`].
//! * A backslash continuation followed by a blank line produces a real
//!   `NEWLINE` token without a position.
//! * Only `\n` ends a line; `\r` is an ordinary character except right
//!   before `\n`.
//!
//! The indentation logic of Jython's `PythonTokenSource` lives in
//! [`crate::indentation`].

use iglint_text::{TextRange, TextSize};

use crate::anchor::Anchor;
use crate::error::{DropReason, DroppedText, ParseError, ParseErrorKind};
use crate::token::{Token, TokenKind};

/// Byte returned past the end of the input. Never occurs in UTF-8 text.
const EOF: u8 = 0xFF;

/// The result of lexing one token.
enum Lexed {
    /// A token on the default channel.
    Visible(Token),
    /// Whitespace, comments, blank lines and continuation lines.
    Hidden,
}

/// Why lexing a token failed.
enum Failure {
    /// The ANTLR lexer reports nothing and skips the text read so far.
    Dropped(DropReason),
    /// The ANTLR lexer raises an error.
    Fatal(ParseError),
}

type LexResult = Result<Lexed, Failure>;

/// Produces the tokens of Jython's lexer that are on the default channel.
pub(crate) struct Lexer<'src> {
    source: &'src str,
    bytes: &'src [u8],
    /// Where lexing starts (after a byte order mark).
    start: usize,
    /// Current position. Positions `len` and `len + 1` are the appended
    /// newlines.
    pos: usize,
    /// `len + 2`.
    end: usize,
    /// ANTLR's `implicitLineJoiningLevel`: bracket depth, may go negative.
    nesting: i32,
    /// Errors in the order they occurred. The parser appends its own errors
    /// here, so that the first entry is the error Jython would report.
    pub(crate) errors: Vec<ParseError>,
    pub(crate) dropped: Vec<DroppedText>,
    /// Set when error recovery consumed half of a surrogate pair.
    split_surrogate: Option<usize>,
    /// Whether to end unclosed brackets at a line that starts a statement
    /// (see [`Lexer::unclosed_bracket_recovery`]).
    recover_unclosed_brackets: bool,
    /// The kind of the last visible token (any keyword may follow a dot).
    last_kind: Option<TokenKind>,
}

impl<'src> Lexer<'src> {
    pub(crate) fn new(source: &'src str) -> Self {
        let start = if source.starts_with('\u{feff}') { 3 } else { 0 };
        Lexer {
            source,
            bytes: source.as_bytes(),
            start,
            pos: start,
            end: source.len() + 2,
            nesting: 0,
            errors: Vec::new(),
            dropped: Vec::new(),
            split_surrogate: None,
            recover_unclosed_brackets: false,
            last_kind: None,
        }
    }

    /// Enables recovery from unclosed brackets for the parser.
    ///
    /// Inside brackets Jython joins lines, so an unclosed bracket swallows
    /// the rest of the file into one logical line. With recovery enabled, a
    /// line inside brackets that starts with a keyword that can only start a
    /// statement (`def`, `class`, `return`, ...), and does not follow a dot,
    /// closes all brackets and gets a [`TokenKind::RecoveryNewline`] before
    /// it. Jython reports a syntax error at or before such a keyword anyway;
    /// the recovery newline is anchored at the keyword and accepted by no
    /// rule, so the first error the parser reports is unchanged.
    pub(crate) fn unclosed_bracket_recovery(mut self) -> Self {
        self.recover_unclosed_brackets = true;
        self
    }

    pub(crate) fn source(&self) -> &'src str {
        self.source
    }

    #[inline]
    fn byte(&self, index: usize) -> u8 {
        if let Some(&byte) = self.bytes.get(index) {
            byte
        } else if index < self.end {
            b'\n'
        } else {
            EOF
        }
    }

    #[inline]
    fn peek(&self) -> u8 {
        self.byte(self.pos)
    }

    #[inline]
    fn peek_at(&self, ahead: usize) -> u8 {
        self.byte(self.pos + ahead)
    }

    /// Consumes one character (one UTF-8 sequence), unless at the end.
    #[inline]
    fn bump(&mut self) {
        let byte = self.peek();
        if byte < 0x80 {
            self.pos += 1;
        } else if byte != EOF {
            self.pos += utf8_len(byte);
        }
    }

    /// ANTLR's `recover()`: consumes the offending character. ANTLR reads
    /// UTF-16, so for a character outside the BMP it consumes only the high
    /// surrogate; the low surrogate then fails to lex.
    fn recover(&mut self) {
        if self.peek() >= 0xF0 && self.peek() != EOF {
            self.split_surrogate = Some(self.pos);
        }
        self.bump();
    }

    /// Whether only whitespace precedes the current position on its line.
    fn first_on_physical_line(&self) -> bool {
        let mut index = self.pos;
        while index > self.start {
            match self.byte(index - 1) {
                b' ' | b'\t' | b'\x0c' => index -= 1,
                b'\n' => return true,
                _ => return false,
            }
        }
        true
    }

    /// Whether a keyword that can only start a statement comes next.
    fn statement_keyword_ahead(&self) -> bool {
        let mut end = self.pos;
        while is_name_continue(self.byte(end)) {
            end += 1;
        }
        let word = &self.bytes[self.pos.min(self.bytes.len())..end.min(self.bytes.len())];
        matches!(
            TokenKind::keyword(word),
            Some(
                TokenKind::Def
                    | TokenKind::Class
                    | TokenKind::Return
                    | TokenKind::Pass
                    | TokenKind::Break
                    | TokenKind::Continue
                    | TokenKind::Global
                    | TokenKind::Del
                    | TokenKind::Assert
                    | TokenKind::Raise
                    | TokenKind::Import
                    | TokenKind::From
                    | TokenKind::Try
                    | TokenKind::Except
                    | TokenKind::Finally
                    | TokenKind::While
                    | TokenKind::With
                    | TokenKind::Elif
                    | TokenKind::Exec
            )
        )
    }

    #[inline]
    fn at_line_start(&self) -> bool {
        self.pos == self.start || self.byte(self.pos - 1) == b'\n'
    }

    /// Clamps a position in the virtual input to the real text.
    #[inline]
    fn clamp(&self, offset: usize) -> TextSize {
        TextSize::from(offset.min(self.source.len()))
    }

    #[inline]
    fn range(&self, start: usize, end: usize) -> TextRange {
        TextRange::new(self.clamp(start), self.clamp(end))
    }

    fn token(&self, kind: TokenKind, start: usize) -> Lexed {
        Lexed::Visible(Token::new(
            kind,
            self.range(start, self.pos),
            Anchor::at(TextSize::from(start)),
        ))
    }

    fn fatal_at(&self, message: String, offset: usize) -> Failure {
        let at = self.clamp(offset);
        let len = if offset < self.source.len() {
            utf8_len(self.bytes[offset])
        } else {
            0
        };
        let range = TextRange::at(at, TextSize::from(len));
        Failure::Fatal(ParseError::syntax(
            message,
            range,
            Anchor::at(TextSize::from(offset)),
        ))
    }

    /// ANTLR's "no viable alternative at character" error.
    fn no_viable_alternative(&self, offset: usize) -> Failure {
        let byte = self.byte(offset);
        let character = match byte {
            EOF => "<EOF>".to_owned(),
            _ => {
                let at = offset.min(self.source.len());
                let c = self.source[at..].chars().next().unwrap_or('\n');
                format!("{c:?}")
            }
        };
        let failure = self.fatal_at(format!("invalid character {character}"), offset);
        if byte >= 0xF0 && byte != EOF {
            // Jython's error message would quote half of a surrogate pair;
            // building the SyntaxError then fails with an internal error.
            if let Failure::Fatal(mut error) = failure {
                error.kind = ParseErrorKind::InternalError;
                return Failure::Fatal(error);
            }
        }
        failure
    }

    /// Returns the next token on the default channel.
    pub(crate) fn next_token(&mut self) -> Token {
        loop {
            if let Lexed::Visible(token) = self.next_any() {
                self.last_kind = Some(token.kind);
                return token;
            }
        }
    }

    /// ANTLR's `nextToken()`: one token, possibly hidden.
    fn next_any(&mut self) -> Lexed {
        if self.nesting > 0
            && self.recover_unclosed_brackets
            && self.last_kind != Some(TokenKind::Dot)
            && self.first_on_physical_line()
            && self.statement_keyword_ahead()
        {
            self.nesting = 0;
            let at = TextSize::from(self.pos);
            self.last_kind = Some(TokenKind::RecoveryNewline);
            return Lexed::Visible(Token::new(
                TokenKind::RecoveryNewline,
                TextRange::empty(self.clamp(self.pos)),
                Anchor::at(at),
            ));
        }
        // Jython computes `startPos` once per call, before error recovery
        // retries, so a dropped token does not change it.
        let line_start = self.at_line_start();
        loop {
            let token_start = self.pos;
            if self.pos >= self.end {
                let at = self.clamp(self.pos);
                return Lexed::Visible(Token::new(
                    TokenKind::EndOfFile,
                    TextRange::empty(at),
                    Anchor::at(TextSize::from(self.pos)),
                ));
            }
            match self.lex(line_start) {
                Ok(lexed) => return lexed,
                Err(Failure::Dropped(reason)) => {
                    let range = self.range(token_start, self.pos);
                    self.dropped.push(DroppedText { range, reason });
                    if let Some(offset) = self.split_surrogate.take() {
                        let range = self.range(offset, self.pos);
                        // As in `no_viable_alternative`: Jython fails with an
                        // internal error when reporting half a surrogate pair.
                        self.errors.push(ParseError::new(
                            ParseErrorKind::InternalError,
                            "invalid character (unpaired low surrogate)",
                            range,
                            Anchor::low_surrogate(TextSize::from(offset)),
                        ));
                        return Lexed::Visible(Token::new(
                            TokenKind::Error,
                            range,
                            Anchor::at(TextSize::from(offset)),
                        ));
                    }
                }
                Err(Failure::Fatal(error)) => {
                    let error_start = error.range.start().to_usize();
                    self.errors.push(error);
                    // Resume at the character that broke the token, or after
                    // it when nothing was read before it.
                    self.pos = token_start.max(error_start.min(self.end));
                    if self.pos == token_start {
                        self.bump();
                    }
                    return Lexed::Visible(Token::new(
                        TokenKind::Error,
                        self.range(token_start, self.pos),
                        Anchor::at(TextSize::from(token_start)),
                    ));
                }
            }
        }
    }

    fn lex(&mut self, line_start: bool) -> LexResult {
        let start = self.pos;
        let c = self.peek();
        let kind = match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => return self.lex_name_or_string(start),
            b'0'..=b'9' => return self.lex_number(start),
            b'.' => {
                if self.peek_at(1).is_ascii_digit() {
                    return self.lex_number(start);
                }
                self.pos += 1;
                TokenKind::Dot
            }
            b'"' | b'\'' => return self.lex_string(start),
            b'\\' => return self.lex_continuation(line_start),
            b'\x0c' => {
                if matches!(self.peek_at(1), b'\n' | b'\r') {
                    return self.lex_newline(line_start);
                }
                return self.lex_whitespace(line_start);
            }
            b'\n' | b'\r' => return self.lex_newline(line_start),
            b' ' | b'\t' => {
                let mut index = self.pos;
                while matches!(self.byte(index), b' ' | b'\t') {
                    index += 1;
                }
                if !line_start {
                    return self.lex_whitespace(line_start);
                }
                if self.byte(index) == b'#' {
                    return self.lex_comment(line_start);
                }
                return self.lex_leading_whitespace();
            }
            b'#' => return self.lex_comment(line_start),
            b'(' => {
                self.pos += 1;
                self.nesting += 1;
                TokenKind::Lpar
            }
            b'[' => {
                self.pos += 1;
                self.nesting += 1;
                TokenKind::Lsqb
            }
            b'{' => {
                self.pos += 1;
                self.nesting += 1;
                TokenKind::Lbrace
            }
            b')' => {
                self.pos += 1;
                self.nesting -= 1;
                TokenKind::Rpar
            }
            b']' => {
                self.pos += 1;
                self.nesting -= 1;
                TokenKind::Rsqb
            }
            b'}' => {
                self.pos += 1;
                self.nesting -= 1;
                TokenKind::Rbrace
            }
            b':' => self.single(TokenKind::Colon),
            b',' => self.single(TokenKind::Comma),
            b';' => self.single(TokenKind::Semi),
            b'`' => self.single(TokenKind::Backquote),
            b'~' => self.single(TokenKind::Tilde),
            b'@' => self.single(TokenKind::At),
            b'+' => self.with_equal(TokenKind::Plus, TokenKind::PlusEqual),
            b'-' => self.with_equal(TokenKind::Minus, TokenKind::MinusEqual),
            b'%' => self.with_equal(TokenKind::Percent, TokenKind::PercentEqual),
            b'|' => self.with_equal(TokenKind::Vbar, TokenKind::VbarEqual),
            b'&' => self.with_equal(TokenKind::Amper, TokenKind::AmperEqual),
            b'^' => self.with_equal(TokenKind::Circumflex, TokenKind::CircumflexEqual),
            b'=' => self.with_equal(TokenKind::Equal, TokenKind::EqEqual),
            b'*' => self.doubled(
                TokenKind::Star,
                TokenKind::StarEqual,
                TokenKind::DoubleStar,
                TokenKind::DoubleStarEqual,
            ),
            b'/' => self.doubled(
                TokenKind::Slash,
                TokenKind::SlashEqual,
                TokenKind::DoubleSlash,
                TokenKind::DoubleSlashEqual,
            ),
            b'>' => self.doubled(
                TokenKind::Greater,
                TokenKind::GreaterEqual,
                TokenKind::RightShift,
                TokenKind::RightShiftEqual,
            ),
            b'<' => {
                if self.peek_at(1) == b'>' {
                    self.pos += 2;
                    TokenKind::AltNotEqual
                } else {
                    self.doubled(
                        TokenKind::Less,
                        TokenKind::LessEqual,
                        TokenKind::LeftShift,
                        TokenKind::LeftShiftEqual,
                    )
                }
            }
            b'!' => {
                // The DFA predicts `!=` from `!` alone; `match("!=")` then
                // fails and the lexer drops `!` and the next character.
                self.pos += 1;
                if self.peek() == b'=' {
                    self.pos += 1;
                    TokenKind::NotEqual
                } else {
                    self.recover();
                    return Err(Failure::Dropped(DropReason::StrayExclamation));
                }
            }
            _ => return Err(self.no_viable_alternative(start)),
        };
        Ok(self.token(kind, start))
    }

    #[inline]
    fn single(&mut self, kind: TokenKind) -> TokenKind {
        self.pos += 1;
        kind
    }

    #[inline]
    fn with_equal(&mut self, plain: TokenKind, equal: TokenKind) -> TokenKind {
        if self.peek_at(1) == b'=' {
            self.pos += 2;
            equal
        } else {
            self.pos += 1;
            plain
        }
    }

    /// Operators like `*`, `*=`, `**`, `**=`.
    fn doubled(
        &mut self,
        plain: TokenKind,
        equal: TokenKind,
        double: TokenKind,
        double_equal: TokenKind,
    ) -> TokenKind {
        let c = self.peek();
        match self.peek_at(1) {
            b'=' => {
                self.pos += 2;
                equal
            }
            next if next == c => {
                if self.peek_at(2) == b'=' {
                    self.pos += 3;
                    double_equal
                } else {
                    self.pos += 2;
                    double
                }
            }
            _ => {
                self.pos += 1;
                plain
            }
        }
    }

    fn lex_name_or_string(&mut self, start: usize) -> LexResult {
        let c = self.peek();
        let next = self.peek_at(1);
        match c {
            b'u' | b'U' | b'b' | b'B' => {
                if is_quote(next) {
                    self.pos += 1;
                    return self.lex_string(start);
                }
                if matches!(next, b'r' | b'R') && is_quote(self.peek_at(2)) {
                    self.pos += 2;
                    return self.lex_string(start);
                }
            }
            b'r' | b'R' if is_quote(next) => {
                self.pos += 1;
                return self.lex_string(start);
            }
            _ => {}
        }
        self.pos += 1;
        while is_name_continue(self.peek()) {
            self.pos += 1;
        }
        let kind = TokenKind::keyword(&self.bytes[start..self.pos]).unwrap_or(TokenKind::Name);
        Ok(self.token(kind, start))
    }

    /// Numbers, following the states of ANTLR's token DFA.
    fn lex_number(&mut self, start: usize) -> LexResult {
        let mut index = start;
        let byte = |index: usize| self.byte(index);

        // Fraction, exponent and imaginary suffix after the integer digits.
        // Returns the kind and end, or the offset of the offending character.
        let tail = |mut index: usize, seen_dot: bool| -> Result<(TokenKind, usize), usize> {
            if !seen_dot && byte(index) == b'.' {
                index += 1;
                while byte(index).is_ascii_digit() {
                    index += 1;
                }
            }
            if matches!(byte(index), b'e' | b'E') {
                index += 1;
                if matches!(byte(index), b'+' | b'-') {
                    index += 1;
                }
                if !byte(index).is_ascii_digit() {
                    return Err(index);
                }
                while byte(index).is_ascii_digit() {
                    index += 1;
                }
            }
            if matches!(byte(index), b'j' | b'J') {
                return Ok((TokenKind::Complex, index + 1));
            }
            Ok((TokenKind::Float, index))
        };

        let result: Result<(TokenKind, usize), usize> = if byte(index) == b'.' {
            // `.5`
            index += 1;
            while byte(index).is_ascii_digit() {
                index += 1;
            }
            tail(index, true)
        } else if byte(index) == b'0' {
            index += 1;
            match byte(index) {
                b'x' | b'X' => {
                    index += 1;
                    if !byte(index).is_ascii_hexdigit() {
                        Err(index)
                    } else {
                        while byte(index).is_ascii_hexdigit() {
                            index += 1;
                        }
                        Ok(int_or_long(byte(index), index))
                    }
                }
                b'o' | b'O' => {
                    index += 1;
                    while matches!(byte(index), b'0'..=b'7') {
                        index += 1;
                    }
                    Ok(int_or_long(byte(index), index))
                }
                b'b' | b'B' => {
                    index += 1;
                    while matches!(byte(index), b'0' | b'1') {
                        index += 1;
                    }
                    Ok(int_or_long(byte(index), index))
                }
                _ => {
                    // Legacy octal; digits 8 and 9 are only allowed when a
                    // fraction, exponent or `j` follows.
                    while matches!(byte(index), b'0'..=b'7') {
                        index += 1;
                    }
                    let mut decimal = false;
                    while byte(index).is_ascii_digit() {
                        decimal = true;
                        index += 1;
                    }
                    match byte(index) {
                        b'.' | b'e' | b'E' => tail(index, false),
                        b'j' | b'J' => Ok((TokenKind::Complex, index + 1)),
                        _ if decimal => Err(index),
                        next => Ok(int_or_long(next, index)),
                    }
                }
            }
        } else {
            while byte(index).is_ascii_digit() {
                index += 1;
            }
            match byte(index) {
                b'.' | b'e' | b'E' => tail(index, false),
                b'j' | b'J' => Ok((TokenKind::Complex, index + 1)),
                next => Ok(int_or_long(next, index)),
            }
        };
        match result {
            Ok((kind, end)) => {
                self.pos = end;
                Ok(self.token(kind, start))
            }
            Err(offending) => Err(self.no_viable_alternative(offending)),
        }
    }

    /// A string literal; the prefix (if any) has been consumed.
    fn lex_string(&mut self, start: usize) -> LexResult {
        let quote = self.peek();
        let triple = if self.peek_at(1) == quote {
            self.peek_at(2) == quote
        } else {
            match self.peek_at(1) {
                b'\n' | EOF => return Err(self.no_viable_alternative(self.pos + 1)),
                _ => false,
            }
        };
        if triple {
            self.pos += 3;
            loop {
                let c0 = self.peek();
                let content = if c0 == quote {
                    let c1 = self.peek_at(1);
                    if c1 == quote {
                        let c2 = self.peek_at(2);
                        c2 != quote && c2 != EOF
                    } else {
                        c1 != EOF
                    }
                } else {
                    c0 != EOF
                };
                if !content {
                    break;
                }
                // TRIQUOTE / TRIAPOS: up to two quotes, then at least one
                // other character or escape.
                if self.peek() == quote {
                    self.pos += 1;
                    if self.peek() == quote {
                        self.pos += 1;
                    }
                }
                loop {
                    match self.peek() {
                        b'\\' => {
                            self.pos += 1;
                            self.bump();
                        }
                        c if c == quote || c == EOF => break,
                        _ => self.bump(),
                    }
                }
            }
            for _ in 0..3 {
                if self.peek() == quote {
                    self.pos += 1;
                } else {
                    self.recover();
                    return Err(Failure::Dropped(DropReason::UnterminatedString));
                }
            }
        } else {
            self.pos += 1;
            loop {
                match self.peek() {
                    b'\\' => {
                        self.pos += 1;
                        self.bump();
                    }
                    b'\n' | EOF => break,
                    c if c == quote => break,
                    _ => self.bump(),
                }
            }
            if self.peek() == quote {
                self.pos += 1;
            } else {
                self.recover();
                return Err(Failure::Dropped(DropReason::UnterminatedString));
            }
        }
        let range = self.range(start, self.pos);
        let multiline = self.pos > self.bytes.len() || self.bytes[start..self.pos].contains(&b'\n');
        let anchor = if multiline {
            // Jython moves a multi-line string token to the line where it
            // ends, at column -1 (-2 internally).
            Anchor::multiline_string(TextSize::from(self.pos))
        } else {
            Anchor::at(TextSize::from(start))
        };
        Ok(Lexed::Visible(Token::new(TokenKind::String, range, anchor)))
    }

    /// `CONTINUED_LINE`: a backslash, an optional `\r`, a newline and the
    /// indentation of the next line.
    fn lex_continuation(&mut self, line_start: bool) -> LexResult {
        self.pos += 1;
        if self.peek() == b'\r' {
            self.pos += 1;
        }
        if self.peek() == b'\n' {
            self.pos += 1;
        } else {
            self.recover();
            return Err(Failure::Dropped(DropReason::StrayBackslash));
        }
        while matches!(self.peek(), b' ' | b'\t') {
            self.pos += 1;
        }
        let mut result = Lexed::Hidden;
        match self.peek() {
            b'#' => {
                self.lex_comment(line_start)?;
            }
            b'\n' | b'\x0c' | b'\r' => {
                let newline_start = self.pos;
                self.lex_newline(line_start)?;
                // The rule emits a fresh NEWLINE token without position, on
                // the default channel even inside brackets.
                result = Lexed::Visible(Token::new(
                    TokenKind::Newline,
                    self.range(newline_start, self.pos),
                    Anchor::synthetic(),
                ));
            }
            _ => {}
        }
        if self.peek() == EOF {
            return Err(Failure::Fatal(ParseError::new(
                ParseErrorKind::SyntaxError,
                "unexpected character after line continuation character",
                self.range(self.pos, self.pos),
                Anchor::unpositioned(),
            )));
        }
        Ok(result)
    }

    /// `NEWLINE`: one or more line ends, each optionally preceded by a form
    /// feed. Hidden at the start of a line and inside brackets.
    fn lex_newline(&mut self, line_start: bool) -> LexResult {
        let start = self.pos;
        let mut count = 0;
        while matches!(self.peek(), b'\n' | b'\x0c' | b'\r') {
            if self.peek() == b'\x0c' {
                self.pos += 1;
            }
            if self.peek() == b'\r' {
                self.pos += 1;
            }
            if self.peek() == b'\n' {
                self.pos += 1;
                count += 1;
            } else {
                self.recover();
                return Err(Failure::Dropped(DropReason::StrayLineBreak));
            }
        }
        debug_assert!(count > 0);
        if line_start || self.nesting > 0 {
            return Ok(Lexed::Hidden);
        }
        Ok(self.token(TokenKind::Newline, start))
    }

    /// `WS`: spaces, tabs and form feeds inside a line. Jython's predicate
    /// rejects it at the start of a line (a form feed followed by code).
    fn lex_whitespace(&mut self, line_start: bool) -> LexResult {
        if line_start {
            return Err(self.fatal_at(
                "form feed at the start of a line must be followed by a newline".to_owned(),
                self.pos,
            ));
        }
        while matches!(self.peek(), b' ' | b'\t' | b'\x0c') {
            self.pos += 1;
        }
        Ok(Lexed::Hidden)
    }

    /// `LEADING_WS`: indentation at the start of a line.
    fn lex_leading_whitespace(&mut self) -> LexResult {
        let start = self.pos;
        if self.nesting > 0 {
            while matches!(self.peek(), b' ' | b'\t') {
                self.pos += 1;
            }
            return Ok(Lexed::Hidden);
        }
        let mut width: u32 = 0;
        loop {
            match self.peek() {
                b' ' => width += 1,
                b'\t' => width = (width / 8 + 1) * 8,
                _ => break,
            }
            self.pos += 1;
        }
        let whitespace_end = self.pos;
        let mut newlines = 0;
        while matches!(self.peek(), b'\n' | b'\r') {
            if self.peek() == b'\r' {
                self.pos += 1;
            }
            if self.peek() == b'\n' {
                self.pos += 1;
                newlines += 1;
            } else {
                self.recover();
                return Err(Failure::Dropped(DropReason::StrayLineBreak));
            }
        }
        // Both tokens get the position after everything read so far.
        let anchor = Anchor::at(TextSize::from(self.pos));
        if self.peek() != EOF || newlines == 0 {
            if newlines != 0 {
                return Ok(Lexed::Hidden);
            }
            let mut token = Token::new(
                TokenKind::LeadingWs,
                self.range(start, whitespace_end),
                anchor,
            );
            token.width = width;
            return Ok(Lexed::Visible(token));
        }
        // Whitespace-only lines at the very end become a visible NEWLINE.
        Ok(Lexed::Visible(Token::new(
            TokenKind::Newline,
            TextRange::empty(self.clamp(self.pos)),
            anchor,
        )))
    }

    /// `COMMENT`. At the start of a line (with optional indentation) the
    /// comment swallows the newlines after it; elsewhere it stops before the
    /// newline.
    fn lex_comment(&mut self, line_start: bool) -> LexResult {
        while matches!(self.peek(), b' ' | b'\t') {
            self.pos += 1;
        }
        // self.peek() == '#'
        self.pos += 1;
        while !matches!(self.peek(), b'\n' | EOF) {
            self.bump();
        }
        if line_start && self.peek() == b'\n' {
            while self.peek() == b'\n' {
                self.pos += 1;
            }
        }
        Ok(Lexed::Hidden)
    }

    /// Column of `offset` in UTF-16 code units, as Jython counts it.
    pub(crate) fn column_of(&self, offset: usize) -> i32 {
        let offset = offset.min(self.bytes.len());
        let line_start = memrchr_newline(&self.bytes[self.start..offset])
            .map_or(self.start, |index| self.start + index + 1);
        let text = &self.source[line_start..offset];
        if text.is_ascii() {
            text.len() as i32
        } else {
            text.chars().map(char::len_utf16).sum::<usize>() as i32
        }
    }
}

#[inline]
fn is_quote(byte: u8) -> bool {
    byte == b'\'' || byte == b'"'
}

#[inline]
fn is_name_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[inline]
fn int_or_long(next: u8, end: usize) -> (TokenKind, usize) {
    if matches!(next, b'l' | b'L') {
        (TokenKind::LongInt, end + 1)
    } else {
        (TokenKind::Int, end)
    }
}

#[inline]
fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

fn memrchr_newline(bytes: &[u8]) -> Option<usize> {
    bytes.iter().rposition(|&byte| byte == b'\n')
}
