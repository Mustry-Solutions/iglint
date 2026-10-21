//! Tokens as seen by the parser.

use std::fmt;

use iglint_text::{Ranged, TextRange};

use crate::anchor::Anchor;

/// The kind of a token. Names follow Jython's grammar (`Python.g`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    // Keywords. `print` and `exec` are keywords in Python 2.7; `print` acts as
    // a name after `from __future__ import print_function`. `None`, `True`,
    // `False`, `nonlocal`, `async` and `await` are names.
    And,
    As,
    Assert,
    Break,
    Class,
    Continue,
    Def,
    Del,
    Elif,
    Else,
    Except,
    Exec,
    Finally,
    For,
    From,
    Global,
    If,
    Import,
    In,
    Is,
    Lambda,
    Not,
    Or,
    Pass,
    Print,
    Raise,
    Return,
    Try,
    While,
    With,
    Yield,

    Name,
    /// An integer literal without `L` suffix (decimal, `0x`, `0o`, `0b` or
    /// legacy octal).
    Int,
    /// An integer literal with `L`/`l` suffix.
    LongInt,
    Float,
    /// An imaginary literal (`1j`, `1.5J`).
    Complex,
    /// One string literal, including its prefix and quotes.
    String,

    Lpar,
    Rpar,
    Lsqb,
    Rsqb,
    Lbrace,
    Rbrace,
    Colon,
    Comma,
    Semi,
    Plus,
    Minus,
    Star,
    Slash,
    Vbar,
    Amper,
    Less,
    Greater,
    Equal,
    Percent,
    Backquote,
    Circumflex,
    Tilde,
    EqEqual,
    NotEqual,
    /// `<>`
    AltNotEqual,
    LessEqual,
    LeftShift,
    GreaterEqual,
    RightShift,
    PlusEqual,
    MinusEqual,
    DoubleStar,
    StarEqual,
    DoubleSlash,
    SlashEqual,
    VbarEqual,
    PercentEqual,
    AmperEqual,
    CircumflexEqual,
    LeftShiftEqual,
    RightShiftEqual,
    DoubleStarEqual,
    DoubleSlashEqual,
    Dot,
    At,

    /// End of a logical line.
    Newline,
    /// End of a logical line that Jython does not have: the parser's lexer
    /// inserts it inside unclosed brackets before a line that starts with a
    /// statement keyword (see [`crate::parse_module`]). No grammar rule
    /// accepts it, so it is a syntax error wherever it appears, anchored at
    /// that keyword, where Jython reports its error.
    RecoveryNewline,
    Indent,
    Dedent,
    /// Leading whitespace that Jython's lexer passes to the parser: on the
    /// first line of a file, or before a form feed.
    LeadingWs,
    EndOfFile,
    /// A lexical error; the error itself is reported separately.
    Error,
}

impl TokenKind {
    /// The keyword spelled `text`, if any.
    pub fn keyword(text: &[u8]) -> Option<TokenKind> {
        use TokenKind::*;
        Some(match text {
            b"and" => And,
            b"as" => As,
            b"assert" => Assert,
            b"break" => Break,
            b"class" => Class,
            b"continue" => Continue,
            b"def" => Def,
            b"del" => Del,
            b"elif" => Elif,
            b"else" => Else,
            b"except" => Except,
            b"exec" => Exec,
            b"finally" => Finally,
            b"for" => For,
            b"from" => From,
            b"global" => Global,
            b"if" => If,
            b"import" => Import,
            b"in" => In,
            b"is" => Is,
            b"lambda" => Lambda,
            b"not" => Not,
            b"or" => Or,
            b"pass" => Pass,
            b"print" => Print,
            b"raise" => Raise,
            b"return" => Return,
            b"try" => Try,
            b"while" => While,
            b"with" => With,
            b"yield" => Yield,
            _ => return None,
        })
    }

    #[inline]
    pub fn is_keyword(self) -> bool {
        (self as u8) <= (TokenKind::Yield as u8)
    }

    /// Jython's grammar name for the token kind, as used in its messages.
    pub fn jython_name(self) -> &'static str {
        use TokenKind::*;
        match self {
            And => "AND",
            As => "AS",
            Assert => "ASSERT",
            Break => "BREAK",
            Class => "CLASS",
            Continue => "CONTINUE",
            Def => "DEF",
            Del => "DELETE",
            Elif => "ELIF",
            Else => "ORELSE",
            Except => "EXCEPT",
            Exec => "EXEC",
            Finally => "FINALLY",
            For => "FOR",
            From => "FROM",
            Global => "GLOBAL",
            If => "IF",
            Import => "IMPORT",
            In => "IN",
            Is => "IS",
            Lambda => "LAMBDA",
            Not => "NOT",
            Or => "OR",
            Pass => "PASS",
            Print => "PRINT",
            Raise => "RAISE",
            Return => "RETURN",
            Try => "TRY",
            While => "WHILE",
            With => "WITH",
            Yield => "YIELD",
            Name => "NAME",
            Int => "INT",
            LongInt => "LONGINT",
            Float => "FLOAT",
            Complex => "COMPLEX",
            String => "STRING",
            Lpar => "LPAREN",
            Rpar => "RPAREN",
            Lsqb => "LBRACK",
            Rsqb => "RBRACK",
            Lbrace => "LCURLY",
            Rbrace => "RCURLY",
            Colon => "COLON",
            Comma => "COMMA",
            Semi => "SEMI",
            Plus => "PLUS",
            Minus => "MINUS",
            Star => "STAR",
            Slash => "SLASH",
            Vbar => "VBAR",
            Amper => "AMPER",
            Less => "LESS",
            Greater => "GREATER",
            Equal => "ASSIGN",
            Percent => "PERCENT",
            Backquote => "BACKQUOTE",
            Circumflex => "CIRCUMFLEX",
            Tilde => "TILDE",
            EqEqual => "EQUAL",
            NotEqual => "NOTEQUAL",
            AltNotEqual => "ALT_NOTEQUAL",
            LessEqual => "LESSEQUAL",
            LeftShift => "LEFTSHIFT",
            GreaterEqual => "GREATEREQUAL",
            RightShift => "RIGHTSHIFT",
            PlusEqual => "PLUSEQUAL",
            MinusEqual => "MINUSEQUAL",
            DoubleStar => "DOUBLESTAR",
            StarEqual => "STAREQUAL",
            DoubleSlash => "DOUBLESLASH",
            SlashEqual => "SLASHEQUAL",
            VbarEqual => "VBAREQUAL",
            PercentEqual => "PERCENTEQUAL",
            AmperEqual => "AMPEREQUAL",
            CircumflexEqual => "CIRCUMFLEXEQUAL",
            LeftShiftEqual => "LEFTSHIFTEQUAL",
            RightShiftEqual => "RIGHTSHIFTEQUAL",
            DoubleStarEqual => "DOUBLESTAREQUAL",
            DoubleSlashEqual => "DOUBLESLASHEQUAL",
            Dot => "DOT",
            At => "AT",
            Newline => "NEWLINE",
            RecoveryNewline => "NEWLINE",
            Indent => "INDENT",
            Dedent => "DEDENT",
            LeadingWs => "LEADING_WS",
            EndOfFile => "EOF",
            Error => "ERROR",
        }
    }

    /// A human readable description for error messages.
    pub fn describe(self) -> &'static str {
        use TokenKind::*;
        match self {
            Name => "name",
            Int | LongInt | Float | Complex => "number",
            String => "string",
            Newline => "end of line",
            RecoveryNewline => "statement keyword inside unclosed brackets",
            Indent => "indent",
            Dedent => "dedent",
            LeadingWs => "leading whitespace",
            EndOfFile => "end of file",
            Error => "invalid token",
            Lpar => "'('",
            Rpar => "')'",
            Lsqb => "'['",
            Rsqb => "']'",
            Lbrace => "'{'",
            Rbrace => "'}'",
            Colon => "':'",
            Comma => "','",
            Semi => "';'",
            Plus => "'+'",
            Minus => "'-'",
            Star => "'*'",
            Slash => "'/'",
            Vbar => "'|'",
            Amper => "'&'",
            Less => "'<'",
            Greater => "'>'",
            Equal => "'='",
            Percent => "'%'",
            Backquote => "'`'",
            Circumflex => "'^'",
            Tilde => "'~'",
            EqEqual => "'=='",
            NotEqual => "'!='",
            AltNotEqual => "'<>'",
            LessEqual => "'<='",
            LeftShift => "'<<'",
            GreaterEqual => "'>='",
            RightShift => "'>>'",
            PlusEqual => "'+='",
            MinusEqual => "'-='",
            DoubleStar => "'**'",
            StarEqual => "'*='",
            DoubleSlash => "'//'",
            SlashEqual => "'/='",
            VbarEqual => "'|='",
            PercentEqual => "'%='",
            AmperEqual => "'&='",
            CircumflexEqual => "'^='",
            LeftShiftEqual => "'<<='",
            RightShiftEqual => "'>>='",
            DoubleStarEqual => "'**='",
            DoubleSlashEqual => "'//='",
            Dot => "'.'",
            At => "'@'",
            And => "'and'",
            As => "'as'",
            Assert => "'assert'",
            Break => "'break'",
            Class => "'class'",
            Continue => "'continue'",
            Def => "'def'",
            Del => "'del'",
            Elif => "'elif'",
            Else => "'else'",
            Except => "'except'",
            Exec => "'exec'",
            Finally => "'finally'",
            For => "'for'",
            From => "'from'",
            Global => "'global'",
            If => "'if'",
            Import => "'import'",
            In => "'in'",
            Is => "'is'",
            Lambda => "'lambda'",
            Not => "'not'",
            Or => "'or'",
            Pass => "'pass'",
            Print => "'print'",
            Raise => "'raise'",
            Return => "'return'",
            Try => "'try'",
            While => "'while'",
            With => "'with'",
            Yield => "'yield'",
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.describe())
    }
}

/// A token handed to the parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// The source range of the token. Tokens that Jython synthesises
    /// (indentation, the final newline) have an empty range.
    pub range: TextRange,
    /// The position Jython attributes to the token.
    pub anchor: Anchor,
    /// For [`TokenKind::LeadingWs`]: the indentation width (tabs advance to the
    /// next multiple of eight). Zero otherwise.
    pub width: u32,
}

impl Token {
    #[inline]
    pub fn new(kind: TokenKind, range: TextRange, anchor: Anchor) -> Self {
        Token {
            kind,
            range,
            anchor,
            width: 0,
        }
    }
}

impl Ranged for Token {
    fn range(&self) -> TextRange {
        self.range
    }
}
