//! String decoding, decoded-to-raw offset mapping and escaping.

use iglint_text::{TextRange, TextSize};

/// Why a string literal is invalid. Offsets are relative to the start of the
/// raw content (after the opening quote).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StringError {
    /// `\` followed by a character that is not a JSON escape.
    InvalidEscape { at: usize, len: usize },
    /// `\u` not followed by four hex digits.
    InvalidUnicodeEscape { at: usize, len: usize },
}

/// One escape sequence: where it starts in decoded and raw text and how long
/// it is in each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Escape {
    decoded_start: u32,
    raw_start: u32,
    decoded_len: u32,
    raw_len: u32,
}

/// Maps byte offsets in a decoded JSON string to byte offsets in the file.
///
/// Built from the raw string content. Lookups are `O(log e)` in the number of
/// escapes `e`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffsetMap {
    /// File offset of the first raw content byte (just after the opening quote).
    base: TextSize,
    decoded_len: u32,
    escapes: Vec<Escape>,
}

impl OffsetMap {
    /// Builds a map for `raw` (the text between the quotes, assumed valid)
    /// whose first byte is at file offset `base`.
    pub fn build(raw: &str, base: TextSize) -> Self {
        let bytes = raw.as_bytes();
        let mut escapes = Vec::new();
        let mut decoded: u32 = 0;
        let mut i = 0usize;
        let mut run_start = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'\\' {
                i += 1;
                continue;
            }
            decoded += (i - run_start) as u32;
            let (decoded_len, raw_len) = escape_lengths(bytes, i);
            escapes.push(Escape {
                decoded_start: decoded,
                raw_start: i as u32,
                decoded_len: decoded_len as u32,
                raw_len: raw_len as u32,
            });
            decoded += decoded_len as u32;
            i += raw_len;
            run_start = i;
        }
        decoded += (bytes.len() - run_start) as u32;
        OffsetMap {
            base,
            decoded_len: decoded,
            escapes,
        }
    }

    /// Length of the decoded string in bytes.
    pub fn decoded_len(&self) -> TextSize {
        TextSize::new(self.decoded_len)
    }

    /// File offset for a decoded offset. An offset inside the UTF-8 bytes of a
    /// character produced by an escape maps to the start of the escape.
    pub fn raw_offset(&self, decoded: TextSize) -> TextSize {
        self.map(decoded.to_u32(), false)
    }

    /// Like [`OffsetMap::raw_offset`], but an offset inside an escaped
    /// character maps to the end of the escape. Use for range ends.
    pub fn raw_offset_end(&self, decoded: TextSize) -> TextSize {
        self.map(decoded.to_u32(), true)
    }

    /// File range for a decoded range: covers every raw byte that produces a
    /// byte of the decoded range.
    pub fn raw_range(&self, decoded: TextRange) -> TextRange {
        let start = self.raw_offset(decoded.start());
        if decoded.is_empty() {
            return TextRange::empty(start);
        }
        let end = self.raw_offset_end(decoded.end());
        TextRange::new(start, end.max(start))
    }

    fn map(&self, decoded: u32, round_up: bool) -> TextSize {
        let x = decoded.min(self.decoded_len);
        // Last escape starting at or before x.
        let idx = self.escapes.partition_point(|e| e.decoded_start <= x);
        let raw = if idx == 0 {
            x
        } else {
            let e = self.escapes[idx - 1];
            let decoded_end = e.decoded_start + e.decoded_len;
            if x == e.decoded_start {
                e.raw_start
            } else if x < decoded_end {
                if round_up {
                    e.raw_start + e.raw_len
                } else {
                    e.raw_start
                }
            } else {
                e.raw_start + e.raw_len + (x - decoded_end)
            }
        };
        self.base + TextSize::new(raw)
    }
}

/// Decoded and raw length of the (valid) escape starting at `bytes[i] == '\\'`.
fn escape_lengths(bytes: &[u8], i: usize) -> (usize, usize) {
    match bytes.get(i + 1) {
        Some(b'u') => {
            let Some(first) = hex4(bytes, i + 2) else {
                return (0, 2.min(bytes.len() - i));
            };
            if (0xD800..0xDC00).contains(&first)
                && bytes.get(i + 6) == Some(&b'\\')
                && bytes.get(i + 7) == Some(&b'u')
                && let Some(second) = hex4(bytes, i + 8)
                && (0xDC00..0xE000).contains(&second)
            {
                return (4, 12);
            }
            let len = match first {
                0..=0x7F => 1,
                0x80..=0x7FF => 2,
                // Lone surrogates decode to U+FFFD (3 bytes).
                _ => 3,
            };
            (len, 6)
        }
        Some(_) => (1, 2),
        None => (0, 1),
    }
}

fn hex4(bytes: &[u8], at: usize) -> Option<u32> {
    let digits = bytes.get(at..at + 4)?;
    let mut value = 0u32;
    for &d in digits {
        value = value * 16 + (d as char).to_digit(16)?;
    }
    Some(value)
}

/// Decodes the raw content of a string literal that contains at least one
/// backslash. `raw` must not contain the closing quote.
pub(crate) fn decode(raw: &str) -> Result<String, StringError> {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0usize;
    let mut run_start = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        // `i` is at a backslash and `run_start` after an ASCII escape.
        #[allow(clippy::string_slice)]
        out.push_str(&raw[run_start..i]);
        let Some(&c) = bytes.get(i + 1) else {
            return Err(StringError::InvalidEscape { at: i, len: 1 });
        };
        let simple = match c {
            b'"' => Some('"'),
            b'\\' => Some('\\'),
            b'/' => Some('/'),
            b'b' => Some('\u{8}'),
            b'f' => Some('\u{c}'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            _ => None,
        };
        if let Some(ch) = simple {
            out.push(ch);
            i += 2;
        } else if c == b'u' {
            let Some(first) = hex4(bytes, i + 2) else {
                let len = 2 + bytes[i + 2..]
                    .iter()
                    .take(4)
                    .take_while(|b| b.is_ascii_hexdigit())
                    .count();
                return Err(StringError::InvalidUnicodeEscape { at: i, len });
            };
            if (0xD800..0xDC00).contains(&first)
                && bytes.get(i + 6) == Some(&b'\\')
                && bytes.get(i + 7) == Some(&b'u')
                && let Some(second) = hex4(bytes, i + 8)
                && (0xDC00..0xE000).contains(&second)
            {
                let cp = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                i += 12;
            } else {
                out.push(char::from_u32(first).unwrap_or('\u{FFFD}'));
                i += 6;
            }
        } else {
            // `i` is at a one-byte backslash.
            #[allow(clippy::string_slice)]
            let len = 1 + raw[i + 1..].chars().next().map_or(0, char::len_utf8);
            return Err(StringError::InvalidEscape { at: i, len });
        }
        run_start = i;
    }
    // `run_start` is 0 or follows an ASCII escape.
    #[allow(clippy::string_slice)]
    out.push_str(&raw[run_start..]);
    Ok(out)
}

/// How to escape strings when writing JSON.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EscapeStyle {
    /// What Ignition's Gson writer produces: `"` and `\` with a backslash,
    /// `\t \n \r \b \f` short forms, other control characters and U+2028,
    /// U+2029 as unicode escapes, and the HTML-sensitive characters
    /// `< > & = '` as unicode escapes too (backslash, `u`, four lowercase hex
    /// digits). Other characters are written as is.
    #[default]
    Ignition,
    /// Only what RFC 8259 requires, using short forms where they exist.
    Minimal,
}

/// Writes `value` as a JSON string literal, including the quotes.
pub fn to_json_string(value: &str, style: EscapeStyle) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    write_json_string(&mut out, value, style);
    out
}

/// Appends `value` as a JSON string literal, including the quotes.
pub fn write_json_string(out: &mut String, value: &str, style: EscapeStyle) {
    out.push('"');
    escape_into(out, value, style);
    out.push('"');
}

/// Appends the escaped content of `value` (without quotes).
pub fn escape_into(out: &mut String, value: &str, style: EscapeStyle) {
    let html = style == EscapeStyle::Ignition;
    let mut run_start = 0;
    for (i, c) in value.char_indices() {
        let replacement: Option<&str> = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\t' => Some("\\t"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            '<' if html => Some("\\u003c"),
            '>' if html => Some("\\u003e"),
            '&' if html => Some("\\u0026"),
            '=' if html => Some("\\u003d"),
            '\'' if html => Some("\\u0027"),
            _ => None,
        };
        let needs_hex = replacement.is_none()
            && ((c as u32) < 0x20 || (html && (c == '\u{2028}' || c == '\u{2029}')));
        if replacement.is_none() && !needs_hex {
            continue;
        }
        // Both offsets come from `char_indices`.
        #[allow(clippy::string_slice)]
        out.push_str(&value[run_start..i]);
        match replacement {
            Some(r) => out.push_str(r),
            None => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
        }
        run_start = i + c.len_utf8();
    }
    // `run_start` comes from `char_indices`.
    #[allow(clippy::string_slice)]
    out.push_str(&value[run_start..]);
}

/// Guesses the escape style a document was written with: [`EscapeStyle::Minimal`]
/// when any string literal contains a raw `<`, `>`, `&`, `=` or `'`, otherwise
/// [`EscapeStyle::Ignition`].
pub fn detect_escape_style(value: &crate::Value<'_>) -> EscapeStyle {
    let mut minimal = false;
    let check = |s: &str| {
        s.bytes()
            .any(|b| matches!(b, b'<' | b'>' | b'&' | b'=' | b'\''))
    };
    value.walk(|_, v| {
        if minimal {
            return false;
        }
        match &v.kind {
            crate::ValueKind::String(s) if check(s.raw()) => minimal = true,
            crate::ValueKind::Object(o) if o.members.iter().any(|m| check(m.key.raw())) => {
                minimal = true;
            }
            _ => {}
        }
        true
    });
    if minimal {
        EscapeStyle::Minimal
    } else {
        EscapeStyle::Ignition
    }
}
