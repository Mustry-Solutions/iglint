//! Ignition 8.3 expression language: lexer, parser, syntax tree, visitor,
//! printers and structured `{...}` references.
//!
//! ```
//! use iglint_expression::{parse, dump, ReferenceContext, Reference};
//!
//! let parsed = parse("if({view.params.mode} = 'edit', \"Save\", \"Edit\")");
//! assert!(parsed.is_valid());
//! assert_eq!(dump(&parsed.expr), r#"(call if (= {view.params.mode} "edit") "Save" "Edit")"#);
//! let r = parsed.expr.references()[0].resolve(ReferenceContext::PerspectiveBinding);
//! assert!(matches!(r, Reference::Property(_)));
//! ```
//!
//! Ranges are byte offsets into the expression text. For expressions stored
//! in JSON strings, map them to the file with the string's offset map
//! (`iglint_json::JsonString::raw_range`).
//!
//! # Grammar
//!
//! The grammar is that of Ignition's own parser, not of its documentation.
//! Sources, all from the `inductiveautomation/ignition:8.3.3` Docker image:
//!
//! * `HMIEL.lex` (JFlex) and `HMIEL.cup` (CUP), shipped inside
//!   `lib/core/common/common.jar` next to the generated `ELLexer` and
//!   `ELParser` classes;
//! * the generated parser itself, run on several hundred probe expressions
//!   through Jython inside the image, to confirm every rule below;
//! * Perspective's `ExpressionBindingParseContext`, `PropertyRef` and
//!   `LegacyPropertyRef` (module 3.3.3) for references, see [`mod@reference`].
//!
//! Lexical rules:
//!
//! * Whitespace is space, tab, form feed, CR and LF. Other characters such
//!   as vertical tab or no-break space are errors.
//! * Comments are `//` to the end of the line. There are no block comments.
//! * Keywords are case-insensitive: `true`, `false`, `null`, `none`, `xor`,
//!   `like`. They cannot be function names (`like(1)` is an error).
//! * Integers: `0`, `[1-9][0-9]*` and `0x`/`0X` hex, as Java `long`; values
//!   out of range are errors. `007` is the double `7.0` (JFlex longest match
//!   picks the double rule). Doubles: `1.`, `.5`, `1.5`, `1e3`, `1.5E-3`.
//!   A number directly followed by letters (`1abc`, `0x`, `1e`) is two
//!   tokens and therefore a syntax error.
//! * Strings use `"..."` or `'...'`. The only escapes are `\t`, `\n`, `\r`
//!   (case-insensitive, so `\T` is a tab), `\\` and the literal's own quote;
//!   `\'` inside double quotes is an error, as are `\u`, `\/` and raw line
//!   breaks.
//! * `{...}` is a reference: any characters except `{`, `}` and line breaks,
//!   kept verbatim (not trimmed).
//! * A string or reference still open at the very end of the input is
//!   discarded without an error (`1 + 2 "abc` is `3`); see
//!   [`Parsed::dropped_tail`].
//! * Identifiers are Java identifier characters as of the JDK the lexer was
//!   generated with (see `tables.rs`); `$` is allowed, `.` is not, so there
//!   are no dotted function names.
//!
//! Syntax (lowest to highest precedence, all binary operators left
//! associative):
//!
//! | level | operators |
//! |---|---|
//! | 1 | `\|\|` |
//! | 2 | `&&` |
//! | 3 | `xor` (bitwise) |
//! | 4 | `\|` |
//! | 5 | `&` |
//! | 6 | `=` `!=` `like` |
//! | 7 | `<` `<=` `>` `>=` |
//! | 8 | `<<` `>>` |
//! | 9 | `+` `-` |
//! | 10 | `*` `/` |
//! | 11 | `%` `^` (same level: `2 ^ 3 % 2` is `(2 ^ 3) % 2`, `1 * 2 % 3` is `1 * (2 % 3)`) |
//! | 12 | prefix `-` `!` `~` (so `-2 ^ 2` is `(-2) ^ 2`, `!a = b` is `(!a) = b`) |
//! | 13 | subscript `x[i]` and `x[row, column]` |
//!
//! There is no `==`, `<>`, `and`, `or`, `not`, ternary or unary `+`.
//! Primaries are literals, references, parenthesised expressions and calls
//! `name(arg, ...)`; a bare identifier is an error. Comparisons chain
//! (`1 < 2 < 3` parses as `(1 < 2) < 3`).
//!
//! # Errors
//!
//! Ignition stops at the first error. [`parse`] reports the same first error
//! (the LALR parser and this parser detect errors at the same token; a
//! lexical error inside a token comes before a syntax error at that token,
//! because Ignition's lexer runs lazily) and then recovers, so later
//! references and calls stay visible to rules.
//!
//! One deliberate deviation: trees deeper than [`MAX_DEPTH`] levels are
//! rejected with [`ParseErrorKind::TooDeep`], so that recursive consumers
//! cannot overflow the stack. Deeply nested input is parsed on a helper
//! thread with a stack sized for it.
//!
//! # Verification
//!
//! The lexer, parser, property-reference grammar and tag-path parser were
//! compared with Ignition's own classes (run through Jython inside the
//! Docker image) on about 90,000 random expressions, 20,000 random
//! property paths and 60,000 random tag paths, with no differences in
//! acceptance or tree shape, and with the first error on the same token in
//! all of the 51,000 rejected expressions. The harness is in
//! `oracle/expression/`; `tests/oracle.rs` replays a recorded fixed-seed
//! set without Docker.

mod ast;
mod lexer;
mod parser;
mod printer;
pub mod reference;
mod tables;
pub mod tag_path;
mod visitor;

pub use ast::{Binary, BinaryOp, BoundRef, Call, Expr, ExprKind, Subscript, Unary, UnaryOp};
pub use lexer::{
    DroppedTail, DroppedTailKind, LexError, LexErrorKind, Lexed, Token, TokenKind, TokenValue, lex,
};
pub use parser::{MAX_DEPTH, ParseError, ParseErrorKind, Parsed, parse};
pub use printer::{dump, quote_string, to_source};
pub use reference::{
    PropertyRef, PropertySegment, PropertyTarget, Reference, ReferenceContext, SegmentKind,
    classify, is_property_ref, parse_property_ref,
};
pub use tag_path::{TagPath, TagPathError, TagPathErrorKind, TagPathPart, parse_tag_path};
pub use visitor::{Visitor, walk_call, walk_expr};

/// True when `c` can start an identifier in Ignition's expression lexer.
pub fn is_identifier_start(c: char) -> bool {
    tables::is_ident_start(c)
}

/// True when `c` can continue an identifier in Ignition's expression lexer.
pub fn is_identifier_part(c: char) -> bool {
    tables::is_ident_part(c)
}
