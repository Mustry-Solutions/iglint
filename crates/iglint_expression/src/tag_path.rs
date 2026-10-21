//! Ignition tag paths: `[provider]Folder/Tag.property`.
//!
//! Follows `TagPathLexer` (JFlex spec `tagpath.lex` in `common.jar`) and
//! `TagPathParser` of Ignition 8.3.3:
//!
//! * `[source]` only before any other token (and at a line start); the source
//!   may contain identifier
//!   characters, spaces and `* ( ) ' _ : / ~ . -`.
//! * Path components are separated by `/` or `\`; empty components are
//!   dropped. `../` (or `..\`) is a component `..`.
//! * Component characters are Java identifier characters (as in the
//!   expression lexer), spaces and `* ( ) ' _ : -`. Tabs, form feeds and line
//!   breaks are skipped and split components.
//! * `.` introduces the property (`Tag.Quality`); a component may carry one
//!   array index, `Tag[0]`, `Tag[1,2]` or `Tag["key"]`.
//! * Anything after the property name is an error.
//!
//! Property names are kept as written. Ignition maps known names
//! case-insensitively (`Quality` becomes `quality`); that mapping belongs to
//! the tag property catalogue, not the parser.

use std::fmt;

use iglint_text::{TextRange, TextSize};

use crate::tables::is_ident_part;

/// A parsed tag path. Parsing never fails outright: on an error the parts
/// before it are kept and [`TagPath::error`] is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagPath {
    /// The text as written.
    pub raw: String,
    /// Range of `raw` in the enclosing source.
    pub range: TextRange,
    /// Provider or relative anchor between the brackets: `default`, `~`
    /// (the provider of the context), `.` (the folder of the tag being
    /// evaluated), `..`, `System`, ... `None` when there are no brackets.
    pub source: Option<TagPathPart>,
    /// Path components. A component's text includes its array index.
    pub components: Vec<TagPathPart>,
    /// The tag property after `.`, as written.
    pub property: Option<TagPathPart>,
    pub error: Option<TagPathError>,
}

/// A piece of a tag path with its range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagPathPart {
    pub text: String,
    pub range: TextRange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagPathError {
    pub range: TextRange,
    pub kind: TagPathErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagPathErrorKind {
    IllegalCharacter(char),
    /// `[source]` somewhere other than the start.
    MisplacedSource,
    /// An array index not directly after a component name.
    InvalidArrayIndex,
    /// `.` not directly after a component name or the source.
    PropertyWithoutTag,
    /// Anything after the property name.
    AfterProperty,
}

impl fmt::Display for TagPathErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagPathErrorKind::IllegalCharacter(c) => {
                write!(f, "illegal character {c:?} in tag path")
            }
            TagPathErrorKind::MisplacedSource => {
                f.write_str("tag source can only appear at the beginning of a tag path")
            }
            TagPathErrorKind::InvalidArrayIndex => {
                f.write_str("invalid source or array specification")
            }
            TagPathErrorKind::PropertyWithoutTag => {
                f.write_str("tag properties can only occur after a tag name")
            }
            TagPathErrorKind::AfterProperty => {
                f.write_str("nothing may follow the tag property name")
            }
        }
    }
}

impl TagPath {
    /// True when Ignition's tag path parser accepts the path.
    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }

    /// The source without brackets, if any.
    pub fn source_name(&self) -> Option<&str> {
        self.source.as_ref().map(|s| s.text.as_str())
    }

    /// True for relative sources `.`, `..` and `~`, and for paths that
    /// start with `..` components.
    pub fn is_relative(&self) -> bool {
        matches!(self.source_name(), Some("." | ".." | "~"))
            || self.components.first().is_some_and(|c| c.text == "..")
    }

    /// Components joined with `/`.
    pub fn path_string(&self) -> String {
        let parts: Vec<&str> = self.components.iter().map(|c| c.text.as_str()).collect();
        parts.join("/")
    }

    pub fn property_name(&self) -> Option<&str> {
        self.property.as_ref().map(|p| p.text.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tok {
    Source,
    Ident,
    Array,
    Sep,
    PropSep,
    DirUp,
}

fn is_component_char(c: char) -> bool {
    is_ident_part(c) || matches!(c, ' ' | '*' | '(' | ')' | '\'' | '_' | ':' | '-')
}

fn is_source_char(c: char) -> bool {
    is_component_char(c) || matches!(c, '/' | '~' | '.')
}

/// Parses a tag path. `offset` is where `path` starts in the enclosing
/// source, for ranges.
pub fn parse_tag_path(path: &str, offset: TextSize) -> TagPath {
    let at = |i: usize| offset + TextSize::from(i);
    let rng = |i: usize, j: usize| TextRange::new(at(i), at(j));
    let mut result = TagPath {
        raw: path.to_owned(),
        range: rng(0, path.len()),
        source: None,
        components: Vec::new(),
        property: None,
        error: None,
    };
    let b = path.as_bytes();
    let mut i = 0;
    let mut prev: Option<Tok> = None;
    let mut prev2: Option<Tok> = None;
    let fail = |result: &mut TagPath, start: usize, end: usize, kind: TagPathErrorKind| {
        result.error = Some(TagPathError {
            range: rng(start, end),
            kind,
        });
    };
    while i < b.len() {
        let start = i;
        let c = path[i..].chars().next().expect("in bounds");
        let tok;
        if matches!(c, '\t' | '\x0C' | '\r' | '\n') {
            i += 1;
            continue;
        } else if path[i..].starts_with("../") || path[i..].starts_with("..\\") {
            i += 3;
            tok = Tok::DirUp;
        } else if c == '/' || c == '\\' {
            i += 1;
            tok = Tok::Sep;
        } else if c == '.' {
            i += 1;
            tok = Tok::PropSep;
        } else if c == '[' {
            // `^\[SourceID\]` only matches at the start of a line. JFlex
            // takes the longest match; on a tie the source rule wins (it
            // comes first in the spec).
            let line_start = start == 0 || matches!(b[start - 1], b'\n' | b'\r');
            let source = if line_start {
                source_end(path, start)
            } else {
                None
            };
            let array = array_end(path, start);
            match (source, array) {
                (Some(s), a) if a.is_none_or(|a| s >= a) => {
                    i = s;
                    tok = Tok::Source;
                }
                (_, Some(a)) => {
                    i = a;
                    tok = Tok::Array;
                }
                _ => {
                    fail(
                        &mut result,
                        i,
                        i + 1,
                        TagPathErrorKind::IllegalCharacter('['),
                    );
                    return result;
                }
            }
        } else if is_component_char(c) {
            while i < b.len() {
                let ch = path[i..].chars().next().expect("in bounds");
                if !is_component_char(ch) {
                    break;
                }
                i += ch.len_utf8();
            }
            tok = Tok::Ident;
        } else {
            fail(
                &mut result,
                i,
                i + c.len_utf8(),
                TagPathErrorKind::IllegalCharacter(c),
            );
            return result;
        }
        let text = &path[start..i];
        if result.property.is_some() {
            fail(&mut result, start, i, TagPathErrorKind::AfterProperty);
            return result;
        }
        match tok {
            Tok::Source => {
                if prev.is_some() {
                    fail(&mut result, start, i, TagPathErrorKind::MisplacedSource);
                    return result;
                }
                result.source = Some(TagPathPart {
                    text: text[1..text.len() - 1].to_owned(),
                    range: rng(start + 1, i - 1),
                });
            }
            Tok::Ident => {
                if prev == Some(Tok::PropSep) {
                    if matches!(prev2, Some(Tok::Ident | Tok::Source | Tok::Array)) {
                        result.property = Some(TagPathPart {
                            text: text.to_owned(),
                            range: rng(start, i),
                        });
                    } else {
                        fail(&mut result, start, i, TagPathErrorKind::PropertyWithoutTag);
                        return result;
                    }
                } else {
                    result.components.push(TagPathPart {
                        text: text.to_owned(),
                        range: rng(start, i),
                    });
                }
            }
            Tok::Array => {
                if prev == Some(Tok::Ident)
                    && let Some(last) = result.components.last_mut()
                {
                    last.text.push_str(text);
                    last.range = last.range.cover(rng(start, i));
                } else {
                    fail(&mut result, start, i, TagPathErrorKind::InvalidArrayIndex);
                    return result;
                }
            }
            Tok::DirUp => result.components.push(TagPathPart {
                text: "..".to_owned(),
                range: rng(start, start + 2),
            }),
            Tok::Sep | Tok::PropSep => {}
        }
        prev2 = prev;
        prev = Some(tok);
    }
    result
}

/// `\[ SourceID \]` at `start`: end offset.
fn source_end(path: &str, start: usize) -> Option<usize> {
    let mut i = start + 1;
    for c in path[start + 1..].chars() {
        if c == ']' {
            return Some(i + 1);
        }
        if !is_source_char(c) {
            return None;
        }
        i += c.len_utf8();
    }
    None
}

/// `\[([0-9]+(,[0-9]+)? | QUOTE .* QUOTE)\]` at `i`: end offset (longest).
fn array_end(path: &str, i: usize) -> Option<usize> {
    let b = path.as_bytes();
    let mut j = i + 1;
    let digits = |mut k: usize| {
        let s = k;
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        (k > s).then_some(k)
    };
    if let Some(k) = digits(j) {
        j = k;
        if b.get(j) == Some(&b',')
            && let Some(k) = digits(j + 1)
        {
            j = k;
        }
        if b.get(j) == Some(&b']') {
            return Some(j + 1);
        }
        return None;
    }
    if matches!(b.get(i + 1), Some(b'"' | b'\'')) {
        // `.*` stops at line breaks; take the longest match ending in a
        // quote followed by `]`.
        let line_end = path[i + 2..]
            .find(['\n', '\r'])
            .map_or(path.len(), |k| i + 2 + k);
        let mut best = None;
        let mut k = i + 2;
        while k + 1 < line_end + 1 && k < line_end {
            if matches!(b[k], b'"' | b'\'') && b.get(k + 1) == Some(&b']') {
                best = Some(k + 2);
            }
            k += 1;
        }
        return best;
    }
    None
}
