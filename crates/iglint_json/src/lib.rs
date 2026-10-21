//! Lossless, span-preserving JSON for iglint.
//!
//! * [`parse`] reads a document into a [`Value`] tree. Every value, key and
//!   member knows its byte range in the source. Object members keep their
//!   order, and duplicate keys are kept (see [`Object::duplicate_keys`]).
//! * Strings keep their raw text and their decoded value (borrowed when the
//!   raw text has no escapes). [`JsonString::raw_offset`] and
//!   [`JsonString::raw_range`] map offsets in the decoded value back to the
//!   file, which is how diagnostics inside scripts and expressions embedded
//!   in `view.json` point at the exact bytes on disk.
//! * [`JsonEditor`] builds minimal [`iglint_text::TextEdit`]s (remove,
//!   insert, replace, rename) that keep the rest of the file byte-for-byte.
//!
//! The parser is strict RFC 8259 with two tolerances for files written by
//! Ignition: a leading byte order mark is skipped and Gson's `NaN`,
//! `Infinity` and `-Infinity` are accepted as numbers (disable with
//! [`ParseOptions::strict`]). It never panics; errors carry a byte range.
//!
//! ## Ignition's formatting
//!
//! Ignition 8.3 writes resources with Gson pretty printing: two-space
//! indentation, `": "` between key and value, one member or element per line,
//! `{}` and `[]` for empty containers, `\n` line endings and usually no final
//! newline. View keys are written in sorted order. Strings use Gson's
//! HTML-safe escaping: `=`, `'`, `<`, `>` and `&` are written as six-character
//! unicode escapes (backslash, `u`, four lowercase hex digits); non-ASCII text
//! is written as is. [`EscapeStyle::Ignition`]
//! reproduces this escaping for new text.

mod edit;
mod parser;
mod path;
mod string;
mod value;

pub use edit::{EditError, InsertPosition, JsonEditor, reindent};
pub use parser::{Expected, ParseError, ParseErrorKind, ParseOptions, parse, parse_with};
pub use path::{JsonPath, PathSegment};
pub use string::{
    EscapeStyle, OffsetMap, detect_escape_style, escape_into, to_json_string, write_json_string,
};
pub use value::{Array, JsonString, Member, Number, Object, Value, ValueKind, compare_utf16};
