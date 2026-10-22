//! Source text primitives shared by every iglint crate: byte offsets, ranges,
//! line/column lookup and text edits.
//!
//! All offsets are UTF-8 byte offsets into the original file contents. Line and
//! column conversion happens only at the edges (reporting, LSP).

use std::fmt;
use std::ops::{Add, Range, Sub};

/// A UTF-8 byte offset into a source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextSize(u32);

impl TextSize {
    pub const ZERO: TextSize = TextSize(0);

    #[inline]
    pub const fn new(offset: u32) -> Self {
        TextSize(offset)
    }

    #[inline]
    pub fn of(text: &str) -> Self {
        TextSize::from(text.len())
    }

    #[inline]
    pub const fn to_u32(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn to_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u32> for TextSize {
    #[inline]
    fn from(value: u32) -> Self {
        TextSize(value)
    }
}

impl From<usize> for TextSize {
    #[inline]
    fn from(value: usize) -> Self {
        TextSize(u32::try_from(value).expect("source files larger than 4 GiB are not supported"))
    }
}

impl From<TextSize> for usize {
    #[inline]
    fn from(value: TextSize) -> Self {
        value.0 as usize
    }
}

impl Add for TextSize {
    type Output = TextSize;
    #[inline]
    fn add(self, rhs: TextSize) -> TextSize {
        TextSize(self.0 + rhs.0)
    }
}

impl Sub for TextSize {
    type Output = TextSize;
    #[inline]
    fn sub(self, rhs: TextSize) -> TextSize {
        TextSize(self.0 - rhs.0)
    }
}

impl fmt::Display for TextSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A half-open byte range `[start, end)` in a source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextRange {
    start: TextSize,
    end: TextSize,
}

impl TextRange {
    #[inline]
    pub fn new(start: TextSize, end: TextSize) -> Self {
        assert!(start <= end, "invalid range {start}..{end}");
        TextRange { start, end }
    }

    #[inline]
    pub fn at(start: TextSize, len: TextSize) -> Self {
        TextRange::new(start, start + len)
    }

    #[inline]
    pub fn empty(offset: TextSize) -> Self {
        TextRange::new(offset, offset)
    }

    #[inline]
    pub const fn start(self) -> TextSize {
        self.start
    }

    #[inline]
    pub const fn end(self) -> TextSize {
        self.end
    }

    #[inline]
    pub fn len(self) -> TextSize {
        self.end - self.start
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    #[inline]
    pub fn contains(self, offset: TextSize) -> bool {
        self.start <= offset && offset < self.end
    }

    #[inline]
    pub fn contains_inclusive(self, offset: TextSize) -> bool {
        self.start <= offset && offset <= self.end
    }

    #[inline]
    pub fn contains_range(self, other: TextRange) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// The smallest range covering both `self` and `other`.
    #[inline]
    pub fn cover(self, other: TextRange) -> TextRange {
        TextRange::new(self.start.min(other.start), self.end.max(other.end))
    }

    #[inline]
    pub fn add_start(self, offset: TextSize) -> TextRange {
        TextRange::new(self.start + offset, self.end)
    }

    #[inline]
    pub fn shift(self, offset: TextSize) -> TextRange {
        TextRange::new(self.start + offset, self.end + offset)
    }

    #[inline]
    pub fn to_std(self) -> Range<usize> {
        self.start.to_usize()..self.end.to_usize()
    }

    #[inline]
    pub fn slice(self, text: &str) -> &str {
        &text[self.to_std()]
    }
}

impl From<Range<usize>> for TextRange {
    fn from(value: Range<usize>) -> Self {
        TextRange::new(value.start.into(), value.end.into())
    }
}

impl fmt::Display for TextRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// Anything that has a source range.
pub trait Ranged {
    fn range(&self) -> TextRange;

    fn start(&self) -> TextSize {
        self.range().start()
    }

    fn end(&self) -> TextSize {
        self.range().end()
    }
}

impl Ranged for TextRange {
    fn range(&self) -> TextRange {
        *self
    }
}

/// A one-based line and column. `column` counts Unicode scalar values, which is
/// what humans and most terminals expect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineColumn {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for LineColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// A zero-based line and UTF-16 code unit column, as used by the Language
/// Server Protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16Position {
    pub line: u32,
    pub character: u32,
}

/// Maps byte offsets to line/column positions for one source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of the start of every line. Always starts with 0.
    line_starts: Vec<TextSize>,
    /// True when the text is pure ASCII, which makes column math trivial.
    ascii: bool,
    len: TextSize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = Vec::with_capacity(text.len() / 32 + 1);
        line_starts.push(TextSize::ZERO);
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'\n' => line_starts.push(TextSize::from(i + 1)),
                b'\r' => {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        i += 1;
                    }
                    line_starts.push(TextSize::from(i + 1));
                }
                _ => {}
            }
            i += 1;
        }
        LineIndex {
            line_starts,
            ascii: text.is_ascii(),
            len: TextSize::of(text),
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Zero-based line containing `offset`.
    pub fn line_of(&self, offset: TextSize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    pub fn line_start(&self, line: usize) -> TextSize {
        self.line_starts[line]
    }

    /// The range of a zero-based line, excluding its terminator.
    pub fn line_range(&self, line: usize, text: &str) -> TextRange {
        let start = self.line_starts[line];
        let mut end = self.line_starts.get(line + 1).copied().unwrap_or(self.len);
        let bytes = text.as_bytes();
        while end > start && matches!(bytes[end.to_usize() - 1], b'\n' | b'\r') {
            end = end - TextSize::new(1);
        }
        TextRange::new(start, end)
    }

    /// One-based line and character column of `offset`.
    pub fn line_column(&self, offset: TextSize, text: &str) -> LineColumn {
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let column = if self.ascii {
            (offset - start).to_u32()
        } else {
            text[start.to_usize()..offset.to_usize()].chars().count() as u32
        };
        LineColumn {
            line: line as u32 + 1,
            column: column + 1,
        }
    }

    pub fn utf16_position(&self, offset: TextSize, text: &str) -> Utf16Position {
        let line = self.line_of(offset);
        let start = self.line_starts[line];
        let character = if self.ascii {
            (offset - start).to_u32()
        } else {
            text[start.to_usize()..offset.to_usize()]
                .chars()
                .map(|c| c.len_utf16() as u32)
                .sum()
        };
        Utf16Position {
            line: line as u32,
            character,
        }
    }

    /// Converts an LSP position back to a byte offset, clamping to the line end.
    pub fn offset_of_utf16(&self, position: Utf16Position, text: &str) -> TextSize {
        let line = position.line as usize;
        if line >= self.line_starts.len() {
            return self.len;
        }
        let range = self.line_range(line, text);
        let mut remaining = position.character;
        let mut offset = range.start();
        for c in range.slice(text).chars() {
            let width = c.len_utf16() as u32;
            if remaining < width {
                break;
            }
            remaining -= width;
            offset = offset + TextSize::from(c.len_utf8());
        }
        offset
    }
}

/// A replacement of `range` with `content` in one file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextEdit {
    pub range: TextRange,
    pub content: String,
}

impl TextEdit {
    pub fn replace(range: TextRange, content: impl Into<String>) -> Self {
        TextEdit {
            range,
            content: content.into(),
        }
    }

    pub fn delete(range: TextRange) -> Self {
        TextEdit::replace(range, String::new())
    }

    pub fn insert(offset: TextSize, content: impl Into<String>) -> Self {
        TextEdit::replace(TextRange::empty(offset), content)
    }
}

/// Error returned when edits overlap and cannot be applied together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlappingEdits {
    pub first: TextRange,
    pub second: TextRange,
}

impl fmt::Display for OverlappingEdits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "edits overlap: {} and {}", self.first, self.second)
    }
}

impl std::error::Error for OverlappingEdits {}

/// The byte that stands in for each byte of an invalid UTF-8 sequence in
/// [`decode_utf8`]. Like U+FFFD (what Ignition's UTF-8 decoding produces),
/// it is not valid in Python code outside strings and comments, and it is
/// one byte, so every offset in the decoded text is the offset on disk.
pub const INVALID_UTF8_SUBSTITUTE: char = '?';

/// Decodes a file's bytes as UTF-8 without changing any offset: each byte
/// of an invalid sequence becomes [`INVALID_UTF8_SUBSTITUTE`]. Also returns
/// the range of each invalid sequence (maximal subparts, as decoders that
/// replace with U+FFFD delimit them), empty for valid UTF-8.
pub fn decode_utf8(bytes: Vec<u8>) -> (String, Vec<TextRange>) {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return (text, Vec::new()),
        Err(error) => error.into_bytes(),
    };
    let mut text = String::with_capacity(bytes.len());
    let mut invalid = Vec::new();
    for chunk in bytes.utf8_chunks() {
        text.push_str(chunk.valid());
        let bad = chunk.invalid();
        if !bad.is_empty() {
            let start = TextSize::from(text.len());
            text.extend(std::iter::repeat_n(INVALID_UTF8_SUBSTITUTE, bad.len()));
            invalid.push(TextRange::new(start, TextSize::from(text.len())));
        }
    }
    (text, invalid)
}

/// Applies non-overlapping edits to `text`. Edits may be given in any order.
/// Two insertions at the same offset are applied in the given order.
pub fn apply_edits(text: &str, edits: &[TextEdit]) -> Result<String, OverlappingEdits> {
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by_key(|&i| (edits[i].range.start(), edits[i].range.end(), i));
    for pair in order.windows(2) {
        let (a, b) = (edits[pair[0]].range, edits[pair[1]].range);
        if a.end() > b.start() {
            return Err(OverlappingEdits {
                first: a,
                second: b,
            });
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for i in order {
        let edit = &edits[i];
        out.push_str(&text[cursor..edit.range.start().to_usize()]);
        out.push_str(&edit.content);
        cursor = edit.range.end().to_usize();
    }
    out.push_str(&text[cursor..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_utf8_keeps_offsets() {
        let (text, invalid) = decode_utf8(b"x = 'caf\xe9'\n".to_vec());
        assert_eq!(text, "x = 'caf?'\n");
        assert_eq!(invalid, [TextRange::from(8..9)]);
        // A truncated three-byte sequence is one invalid sequence of two
        // bytes; valid multi-byte characters are kept.
        let (text, invalid) = decode_utf8(b"\xe2\x82 \xc3\xa9\xff\xfe".to_vec());
        assert_eq!(text, "?? \u{e9}??");
        assert_eq!(
            invalid,
            [
                TextRange::from(0..2),
                TextRange::from(5..6),
                TextRange::from(6..7)
            ]
        );
        let (text, invalid) = decode_utf8("caf\u{e9}".as_bytes().to_vec());
        assert_eq!(text, "caf\u{e9}");
        assert!(invalid.is_empty());
    }

    #[test]
    fn line_column_ascii_and_unicode() {
        let text = "ab\ncé\r\nx";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        assert_eq!(
            index.line_column(TextSize::new(0), text),
            LineColumn { line: 1, column: 1 }
        );
        assert_eq!(
            index.line_column(TextSize::new(3), text),
            LineColumn { line: 2, column: 1 }
        );
        // 'é' is two bytes; the offset after it is column 3.
        assert_eq!(
            index.line_column(TextSize::new(6), text),
            LineColumn { line: 2, column: 3 }
        );
        assert_eq!(
            index.line_column(TextSize::new(8), text),
            LineColumn { line: 3, column: 1 }
        );
        assert_eq!(index.line_range(1, text).slice(text), "cé");
    }

    #[test]
    fn utf16_roundtrip() {
        let text = "a😀b\nz";
        let index = LineIndex::new(text);
        let offset = TextSize::new(5); // after the emoji
        let pos = index.utf16_position(offset, text);
        assert_eq!(
            pos,
            Utf16Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(index.offset_of_utf16(pos, text), offset);
    }

    #[test]
    fn apply_edits_in_any_order() {
        let text = "hello world";
        let edits = vec![
            TextEdit::replace(TextRange::from(6..11), "there"),
            TextEdit::insert(TextSize::new(0), ">> "),
        ];
        assert_eq!(apply_edits(text, &edits).unwrap(), ">> hello there");
    }

    #[test]
    fn apply_edits_rejects_overlap() {
        let text = "hello";
        let edits = vec![
            TextEdit::delete(TextRange::from(0..3)),
            TextEdit::delete(TextRange::from(2..4)),
        ];
        assert!(apply_edits(text, &edits).is_err());
    }
}
