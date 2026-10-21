//! Mapping from generated (wrapped) source text back to bytes in the file.

use iglint_json::{JsonString, OffsetMap};
use iglint_text::{TextRange, TextSize};

/// Maps offsets in a generated source (a wrapped script or an expression)
/// to byte offsets in the file it came from.
///
/// A generated source is an optional synthetic prefix (a `def` header and its
/// newline, which do not exist in the file) followed by a body that does: either
/// the decoded value of a JSON string, or a slice of a file.
///
/// * Offsets in the prefix map to the **anchor**: the file range that stands for
///   the generated header, usually the JSON key of the script property or the
///   dropped first line of a `.py` file.
/// * Offsets in a JSON string body go through the string's escape map, so an
///   offset after `=` in the file lands after the six escape bytes.
/// * Offsets in a file body are shifted by the body's start offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceMap {
    prefix_len: TextSize,
    anchor: TextRange,
    body_len: TextSize,
    body: BodyMap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum BodyMap {
    /// The body is the file slice starting at `file_start`.
    File { file_start: TextSize },
    /// The body is the decoded value of a JSON string.
    Json(OffsetMap),
}

impl SourceMap {
    /// The generated text is exactly `len` bytes of the file starting at
    /// `file_start` (a `.py` file linted as written).
    pub fn identity(file_start: TextSize, len: TextSize) -> Self {
        SourceMap {
            prefix_len: TextSize::ZERO,
            anchor: TextRange::empty(file_start),
            body_len: len,
            body: BodyMap::File { file_start },
        }
    }

    /// `prefix_len` synthetic bytes mapped to `anchor`, then `body_len` file
    /// bytes starting at `file_start`.
    pub fn file_with_prefix(
        prefix_len: TextSize,
        anchor: TextRange,
        file_start: TextSize,
        body_len: TextSize,
    ) -> Self {
        SourceMap {
            prefix_len,
            anchor,
            body_len,
            body: BodyMap::File { file_start },
        }
    }

    /// `prefix_len` synthetic bytes mapped to `anchor`, then the decoded value
    /// of `body`.
    pub fn json(prefix_len: TextSize, anchor: TextRange, body: &JsonString<'_>) -> Self {
        let map = body.offset_map();
        SourceMap {
            prefix_len,
            anchor,
            body_len: map.decoded_len(),
            body: BodyMap::Json(map),
        }
    }

    /// Length of the synthetic prefix; the body starts at this offset.
    pub fn body_start(&self) -> TextSize {
        self.prefix_len
    }

    /// Length of the generated text.
    pub fn generated_len(&self) -> TextSize {
        self.prefix_len + self.body_len
    }

    /// The file range that synthetic text maps to.
    pub fn anchor(&self) -> TextRange {
        self.anchor
    }

    /// True when `generated` lies in the synthetic prefix.
    pub fn is_synthetic(&self, generated: TextSize) -> bool {
        generated < self.prefix_len
    }

    /// The file offset of a generated offset. Synthetic offsets map to the
    /// start of the anchor; offsets inside an escaped character map to the
    /// start of the escape; offsets past the end clamp to the body end.
    pub fn file_offset(&self, generated: TextSize) -> TextSize {
        self.map(generated, false)
    }

    /// Like [`SourceMap::file_offset`] but for range ends: synthetic offsets
    /// map to the end of the anchor, offsets inside an escaped character to
    /// the end of the escape.
    pub fn file_offset_end(&self, generated: TextSize) -> TextSize {
        self.map(generated, true)
    }

    /// The file range covering every byte that produced `generated`. A range
    /// entirely inside the synthetic prefix maps to the anchor.
    pub fn file_range(&self, generated: TextRange) -> TextRange {
        let start = self.file_offset(generated.start());
        if generated.is_empty() {
            return TextRange::empty(start);
        }
        if generated.end() <= self.prefix_len {
            return self.anchor;
        }
        let end = self.file_offset_end(generated.end());
        TextRange::new(start, end.max(start))
    }

    /// The generated offset of a file offset inside the body, or `None` when
    /// the file offset is outside the body. A file offset inside an escape
    /// sequence maps to the decoded character it produces.
    pub fn generated_offset(&self, file: TextSize) -> Option<TextSize> {
        match &self.body {
            BodyMap::File { file_start } => {
                if file < *file_start || file > *file_start + self.body_len {
                    return None;
                }
                Some(self.prefix_len + (file - *file_start))
            }
            BodyMap::Json(map) => {
                let first = map.raw_offset(TextSize::ZERO);
                let last = map.raw_offset(self.body_len);
                if file < first || file > last {
                    return None;
                }
                // `raw_offset` is monotonic: find the last decoded offset whose
                // raw offset is at or before `file`.
                let (mut lo, mut hi) = (0u32, self.body_len.to_u32());
                while lo < hi {
                    let mid = lo + (hi - lo).div_ceil(2);
                    if map.raw_offset(TextSize::new(mid)) <= file {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                    }
                }
                // Offsets inside a character decoded from an escape share
                // the escape's raw offset; return the character start.
                let raw = map.raw_offset(TextSize::new(lo));
                while lo > 0 && map.raw_offset(TextSize::new(lo - 1)) == raw {
                    lo -= 1;
                }
                Some(self.prefix_len + TextSize::new(lo))
            }
        }
    }

    fn map(&self, generated: TextSize, end: bool) -> TextSize {
        if generated < self.prefix_len {
            return if end {
                self.anchor.end()
            } else {
                self.anchor.start()
            };
        }
        let offset = (generated - self.prefix_len).min(self.body_len);
        match &self.body {
            BodyMap::File { file_start } => *file_start + offset,
            BodyMap::Json(map) => {
                if end {
                    map.raw_offset_end(offset)
                } else {
                    map.raw_offset(offset)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(n: u32) -> TextSize {
        TextSize::new(n)
    }

    #[test]
    fn json_body_with_prefix() {
        let text = r#"{"script": "\ta \u003d 1"}"#;
        let root = iglint_json::parse(text).unwrap();
        let member = root.as_object().unwrap().member("script").unwrap();
        let s = member.value.as_json_string().unwrap();
        let header = "def f():\n";
        let map = SourceMap::json(TextSize::of(header), member.key.range(), s);
        let generated = format!("{header}{}", s.value());
        assert_eq!(generated, "def f():\n\ta = 1");
        // Header maps to the key.
        assert_eq!(
            map.file_range(TextRange::new(ts(0), ts(3))).slice(text),
            "\"script\""
        );
        // `=` maps to the escape.
        let eq = generated.find('=').unwrap() as u32;
        assert_eq!(
            map.file_range(TextRange::at(ts(eq), ts(1))).slice(text),
            "\\u003d"
        );
        let one = generated.find('1').unwrap() as u32;
        assert_eq!(
            map.file_range(TextRange::at(ts(one), ts(1))).slice(text),
            "1"
        );
        // Reverse mapping.
        let file_one = text.find('1').unwrap() as u32;
        assert_eq!(map.generated_offset(ts(file_one)), Some(ts(one)));
        let file_escape = text.find("\\u003d").unwrap() as u32;
        assert_eq!(map.generated_offset(ts(file_escape + 3)), Some(ts(eq)));
        assert_eq!(map.generated_offset(ts(0)), None);
    }

    #[test]
    fn file_body_with_prefix() {
        let text = "def f(bogus):\n\tpass";
        let map = SourceMap::file_with_prefix(ts(9), TextRange::new(ts(0), ts(13)), ts(14), ts(5));
        let generated = "def f():\n\tpass";
        assert_eq!(
            map.file_range(TextRange::new(ts(0), ts(5))).slice(text),
            "def f(bogus):"
        );
        let p = generated.find("pass").unwrap() as u32;
        assert_eq!(
            map.file_range(TextRange::at(ts(p), ts(4))).slice(text),
            "pass"
        );
        assert_eq!(map.generated_offset(ts(15)), Some(ts(p)));
    }

    #[test]
    fn identity() {
        let map = SourceMap::identity(ts(0), ts(10));
        assert_eq!(map.file_offset(ts(4)), ts(4));
        assert_eq!(map.file_offset(ts(40)), ts(10));
        assert_eq!(map.generated_offset(ts(11)), None);
    }
}
