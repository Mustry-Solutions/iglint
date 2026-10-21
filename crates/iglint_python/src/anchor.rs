//! Jython-compatible source positions.
//!
//! Every node of a Jython AST carries `lineno` and `col_offset`, which Jython
//! takes from one specific token of the node. Which token that is does not
//! always coincide with the start of the node's source range (a chained
//! binary operation uses its operator, a `with` statement its first context
//! expression, a negative literal its digits, ...). An [`Anchor`] records that
//! token so that [`Locator`] can reproduce Jython's numbers exactly.

use iglint_text::TextSize;

/// The token Jython uses for a node's `lineno` / `col_offset`.
///
/// Packed into 32 bits: the top three bits select the kind, the rest is a
/// byte offset into the source (so sources are limited to 512 MiB).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Anchor(u32);

const KIND_SHIFT: u32 = 29;
const OFFSET_MASK: u32 = (1 << KIND_SHIFT) - 1;
const KIND_OFFSET: u32 = 0;
const KIND_STRING_END: u32 = 1;
const KIND_SYNTHETIC: u32 = 2;
const KIND_UNPOSITIONED: u32 = 3;
const KIND_LOW_SURROGATE: u32 = 4;
const KIND_STRING_TOKEN: u32 = 5;

/// What an [`Anchor`] points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AnchorKind {
    /// The start of a token at this byte offset.
    Offset(TextSize),
    /// A string token that spans several lines. Jython reports the line on
    /// which the token ends (the line of this byte offset) and column -1.
    MultilineString(TextSize),
    /// A token that Jython synthesises without any position (line 0,
    /// column -1), such as the newline produced by a backslash continuation
    /// that is followed by a blank line.
    Synthetic,
    /// No position at all (line 0, column 0); used by some of Jython's
    /// errors.
    Unpositioned,
    /// The second UTF-16 code unit of the character (outside the BMP) at
    /// this byte offset.
    LowSurrogate(TextSize),
    /// A multi-line string token itself (rather than a node anchored at
    /// it), as in a syntax error at that token: the line of this byte offset
    /// and column -2.
    MultilineStringToken(TextSize),
}

impl Anchor {
    /// Anchors at the token that starts at `offset`.
    #[inline]
    pub fn at(offset: TextSize) -> Self {
        Anchor(offset.to_u32().min(OFFSET_MASK))
    }

    /// Anchors at a multi-line string token that ends at `end`.
    #[inline]
    pub fn multiline_string(end: TextSize) -> Self {
        Anchor((KIND_STRING_END << KIND_SHIFT) | end.to_u32().min(OFFSET_MASK))
    }

    /// An anchor without a position.
    #[inline]
    pub const fn synthetic() -> Self {
        Anchor(KIND_SYNTHETIC << KIND_SHIFT)
    }

    /// An anchor that Jython reports as line 0, column 0.
    #[inline]
    pub const fn unpositioned() -> Self {
        Anchor(KIND_UNPOSITIONED << KIND_SHIFT)
    }

    /// Anchors at the low surrogate of the character at `offset`.
    #[inline]
    pub fn low_surrogate(offset: TextSize) -> Self {
        Anchor((KIND_LOW_SURROGATE << KIND_SHIFT) | offset.to_u32().min(OFFSET_MASK))
    }

    /// The anchor for reporting an error at the token anchored here. Only
    /// multi-line string tokens differ: Jython reports column -2 for them.
    #[inline]
    pub fn as_token(self) -> Self {
        match self.kind() {
            AnchorKind::MultilineString(end) => {
                Anchor((KIND_STRING_TOKEN << KIND_SHIFT) | end.to_u32().min(OFFSET_MASK))
            }
            _ => self,
        }
    }

    #[inline]
    pub fn kind(self) -> AnchorKind {
        let offset = TextSize::new(self.0 & OFFSET_MASK);
        match self.0 >> KIND_SHIFT {
            KIND_OFFSET => AnchorKind::Offset(offset),
            KIND_STRING_END => AnchorKind::MultilineString(offset),
            KIND_SYNTHETIC => AnchorKind::Synthetic,
            KIND_UNPOSITIONED => AnchorKind::Unpositioned,
            KIND_LOW_SURROGATE => AnchorKind::LowSurrogate(offset),
            _ => AnchorKind::MultilineStringToken(offset),
        }
    }

    /// The byte offset this anchor refers to, if any.
    #[inline]
    pub fn offset(self) -> Option<TextSize> {
        match self.kind() {
            AnchorKind::Offset(offset)
            | AnchorKind::MultilineString(offset)
            | AnchorKind::LowSurrogate(offset)
            | AnchorKind::MultilineStringToken(offset) => Some(offset),
            AnchorKind::Synthetic | AnchorKind::Unpositioned => None,
        }
    }
}

impl std::fmt::Debug for Anchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind() {
            AnchorKind::Offset(offset) => write!(f, "@{offset}"),
            AnchorKind::MultilineString(offset) => write!(f, "@{offset}(multiline)"),
            AnchorKind::Synthetic => write!(f, "@synthetic"),
            AnchorKind::Unpositioned => write!(f, "@unpositioned"),
            AnchorKind::LowSurrogate(offset) => write!(f, "@{offset}+1"),
            AnchorKind::MultilineStringToken(offset) => write!(f, "@{offset}(multiline token)"),
        }
    }
}

/// A Jython `lineno` / `col_offset` pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JythonLocation {
    /// One-based line. Lines are separated by `\n` only; a lone `\r` does not
    /// start a new line. Zero for synthetic positions.
    pub line: i32,
    /// Zero-based column in UTF-16 code units, or -1 (multi-line strings and
    /// synthetic positions).
    pub column: i32,
}

/// Converts byte offsets and anchors into Jython's line/column numbering.
#[derive(Clone, Debug)]
pub struct Locator<'src> {
    source: &'src str,
    /// Byte offset of the start of every line (`\n`-separated).
    line_starts: Vec<u32>,
    /// Length of a leading byte order mark, which Jython never sees.
    bom: u32,
}

impl<'src> Locator<'src> {
    pub fn new(source: &'src str) -> Self {
        let bom = if source.starts_with('\u{feff}') { 3 } else { 0 };
        let mut line_starts = Vec::with_capacity(source.len() / 32 + 1);
        line_starts.push(bom);
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index as u32 + 1);
            }
        }
        Locator {
            source,
            line_starts,
            bom,
        }
    }

    /// Jython's location of the character at `offset`. Offsets past the end
    /// of the source refer to the two newlines that Jython's `compile()`
    /// appends to every source.
    pub fn location(&self, offset: TextSize) -> JythonLocation {
        let len = self.source.len() as u32;
        let offset = offset.to_u32().max(self.bom);
        if offset > len {
            // Inside the appended "\n\n": the first appended newline sits at
            // `len`, the second one starts an empty line.
            let last_line = self.line_starts.len() as i32;
            let extra = (offset - len) as i32;
            return JythonLocation {
                line: last_line + extra,
                column: 0,
            };
        }
        let line = match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        };
        let start = self.line_starts[line] as usize;
        let text = &self.source[start..offset as usize];
        let column = if text.is_ascii() {
            text.len()
        } else {
            text.chars().map(char::len_utf16).sum()
        };
        JythonLocation {
            line: line as i32 + 1,
            column: column as i32,
        }
    }

    /// Jython's `lineno` / `col_offset` for a node anchored at `anchor`.
    pub fn anchor_location(&self, anchor: Anchor) -> JythonLocation {
        match anchor.kind() {
            AnchorKind::Offset(offset) => self.location(offset),
            AnchorKind::MultilineString(end) => JythonLocation {
                line: self.location(end).line,
                column: -1,
            },
            AnchorKind::Synthetic => JythonLocation {
                line: 0,
                column: -1,
            },
            AnchorKind::Unpositioned => JythonLocation { line: 0, column: 0 },
            AnchorKind::LowSurrogate(offset) => {
                let location = self.location(offset);
                JythonLocation {
                    line: location.line,
                    column: location.column + 1,
                }
            }
            AnchorKind::MultilineStringToken(end) => JythonLocation {
                line: self.location(end).line,
                column: -2,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_count_utf16_units_and_only_newline_breaks_lines() {
        let source = "a\r\nb = '\u{e9}\u{1F600}' + c\rd\n";
        let locator = Locator::new(source);
        let c = source.find('c').unwrap();
        assert_eq!(
            locator.location(TextSize::from(c)),
            JythonLocation {
                line: 2,
                column: 12
            }
        );
        let d = source.find('d').unwrap();
        assert_eq!(
            locator.location(TextSize::from(d)),
            JythonLocation {
                line: 2,
                column: 14
            }
        );
    }

    #[test]
    fn anchors_round_trip() {
        let anchor = Anchor::multiline_string(TextSize::new(42));
        assert_eq!(
            anchor.kind(),
            AnchorKind::MultilineString(TextSize::new(42))
        );
        assert_eq!(Anchor::synthetic().kind(), AnchorKind::Synthetic);
        assert_eq!(
            Anchor::at(TextSize::new(7)).offset(),
            Some(TextSize::new(7))
        );
    }
}
