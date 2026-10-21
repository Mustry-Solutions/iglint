//! Values of number and string literals, computed the way Jython's
//! `GrammarActions` does.

use crate::ast::{Number, StringValue};

/// Why Jython cannot evaluate a literal. Jython fails with an exception other
/// than `SyntaxError` in these cases (`NumberFormatException`,
/// `UnicodeDecodeError`, ...), so the source cannot be compiled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiteralError {
    /// An integer literal without digits (`0o`, `0b`, `0oL`).
    EmptyInteger,
    /// A malformed or out-of-range escape sequence.
    InvalidEscape,
    /// An octal escape above `\377` in a byte string.
    ByteOutOfRange,
    /// A unicode string with an unpaired surrogate.
    UnpairedSurrogate,
}

impl LiteralError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            LiteralError::EmptyInteger => "integer literal without digits",
            LiteralError::InvalidEscape => "invalid escape sequence in string literal",
            LiteralError::ByteOutOfRange => "octal escape above \\377 in byte string",
            LiteralError::UnpairedSurrogate => "unpaired surrogate in unicode string",
        }
    }
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// The value of an `INT` or `LONGINT` token (`GrammarActions.makeInt`).
pub(crate) fn integer(text: &str) -> Result<Number, LiteralError> {
    let (digits, radix) = if let Some(rest) = strip_prefix_ci(text, "0x") {
        (rest, 16)
    } else if let Some(rest) = strip_prefix_ci(text, "0o") {
        (rest, 8)
    } else if let Some(rest) = strip_prefix_ci(text, "0b") {
        (rest, 2)
    } else if text.starts_with('0') {
        (text, 8)
    } else {
        (text, 10)
    };
    if let Some(digits) = digits
        .strip_suffix('L')
        .or_else(|| digits.strip_suffix('l'))
    {
        if digits.is_empty() {
            return Err(LiteralError::EmptyInteger);
        }
        return Ok(Number::Long(to_decimal(digits, radix, false).into()));
    }
    if digits.is_empty() {
        return Err(LiteralError::EmptyInteger);
    }
    let significant = digits.trim_start_matches('0').len();
    if significant > 11 {
        return Ok(Number::Long(to_decimal(digits, radix, false).into()));
    }
    // At most 11 digits: fits in an i64 for every radix.
    let value = digits.bytes().fold(0i64, |value, digit| {
        value * i64::from(radix) + i64::from(digit_value(digit))
    });
    if value > i64::from(i32::MAX) {
        Ok(Number::Long(value.to_string().into()))
    } else {
        Ok(Number::Int(value))
    }
}

/// The value of a `FLOAT` token (Java's `Double.valueOf`).
pub(crate) fn float(text: &str) -> f64 {
    text.parse().unwrap_or(0.0)
}

/// The imaginary part of a `COMPLEX` token.
pub(crate) fn imaginary(text: &str) -> f64 {
    float(&text[..text.len().saturating_sub(1)])
}

/// Jython's constant folding of a unary minus on a literal
/// (`GrammarActions.negate`). Returns `None` when the minus stays a
/// `UnaryOp`.
pub(crate) fn negate(number: &Number) -> Option<Number> {
    match number {
        Number::Int(value) if *value >= 0 => Some(Number::Int(-value)),
        Number::Long(digits) if !digits.starts_with('-') && digits.as_ref() != "0" => {
            Some(Number::Long(format!("-{digits}").into()))
        }
        // `v >= 0` is true for -0.0, so `- -0.0` folds to 0.0.
        Number::Float(value) if *value >= 0.0 => Some(Number::Float(-value)),
        Number::Complex(value) if *value >= 0.0 => Some(Number::Complex(-value)),
        _ => None,
    }
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

fn digit_value(digit: u8) -> u32 {
    match digit {
        b'0'..=b'9' => u32::from(digit - b'0'),
        b'a'..=b'f' => u32::from(digit - b'a') + 10,
        b'A'..=b'F' => u32::from(digit - b'A') + 10,
        _ => 0,
    }
}

/// Converts digits in `radix` to a decimal string of arbitrary size.
fn to_decimal(digits: &str, radix: u32, negative: bool) -> String {
    // Little-endian limbs in base 10^9.
    const BASE: u64 = 1_000_000_000;
    let mut limbs: Vec<u64> = vec![0];
    for digit in digits.bytes() {
        let mut carry = u64::from(digit_value(digit));
        for limb in &mut limbs {
            let value = *limb * u64::from(radix) + carry;
            *limb = value % BASE;
            carry = value / BASE;
        }
        while carry > 0 {
            limbs.push(carry % BASE);
            carry /= BASE;
        }
    }
    let mut out = String::with_capacity(limbs.len() * 9 + 1);
    if negative {
        out.push('-');
    }
    let mut iter = limbs.iter().rev();
    if let Some(first) = iter.next() {
        out.push_str(&first.to_string());
    }
    for limb in iter {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// The charset of the source, which plain (`str`) literals are encoded with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceEncoding {
    /// UTF-8; also used for unicode sources (what Ignition compiles).
    #[default]
    Utf8,
    /// ISO-8859-1.
    Latin1,
    /// US-ASCII; other characters become `?`.
    Ascii,
    /// Windows-1252.
    Cp1252,
}

impl SourceEncoding {
    /// The charset for a PEP 263 encoding name, if supported.
    pub fn from_name(name: &str) -> Option<Self> {
        let normalized = name.to_ascii_lowercase().replace('_', "-");
        Some(match normalized.as_str() {
            "utf-8" | "utf8" | "u8" | "utf" => SourceEncoding::Utf8,
            "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" | "8859" | "cp819" | "l1"
            | "iso-latin-1" => SourceEncoding::Latin1,
            "ascii" | "us-ascii" | "646" => SourceEncoding::Ascii,
            "cp1252" | "windows-1252" => SourceEncoding::Cp1252,
            _ => return None,
        })
    }

    fn encode(self, text: &str, out: &mut Vec<u16>) {
        match self {
            SourceEncoding::Utf8 => {
                for &byte in text.as_bytes() {
                    out.push(u16::from(byte));
                }
            }
            SourceEncoding::Latin1 => {
                for c in text.chars() {
                    out.push(if (c as u32) < 0x100 {
                        c as u16
                    } else {
                        u16::from(b'?')
                    });
                }
            }
            SourceEncoding::Ascii => {
                for c in text.chars() {
                    out.push(if c.is_ascii() {
                        c as u16
                    } else {
                        u16::from(b'?')
                    });
                }
            }
            SourceEncoding::Cp1252 => {
                for c in text.chars() {
                    out.push(cp1252_byte(c).map_or(u16::from(b'?'), u16::from));
                }
            }
        }
    }
}

fn cp1252_byte(c: char) -> Option<u8> {
    const HIGH: [u16; 32] = [
        0x20AC, 0, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
        0x0152, 0, 0x017D, 0, 0, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC,
        0x2122, 0x0161, 0x203A, 0x0153, 0, 0x017E, 0x0178,
    ];
    let code = c as u32;
    if code < 0x80 || (0xA0..0x100).contains(&code) {
        return Some(code as u8);
    }
    HIGH.iter()
        .position(|&mapped| mapped != 0 && u32::from(mapped) == code)
        .map(|index| 0x80 + index as u8)
}

/// Decodes the string tokens of one `Str` node (`extractStrings`).
pub(crate) fn decode_strings<'a>(
    tokens: impl IntoIterator<Item = &'a str>,
    encoding: SourceEncoding,
    unicode_literals: bool,
) -> Result<StringValue, LiteralError> {
    let mut units: Vec<u16> = Vec::new();
    let mut unicode = false;
    for token in tokens {
        unicode |= decode_string(token, encoding, unicode_literals, &mut units)?;
    }
    if unicode {
        let text = String::from_utf16(&units).map_err(|_| LiteralError::UnpairedSurrogate)?;
        Ok(StringValue::Unicode(text.into()))
    } else {
        let bytes = units
            .iter()
            .map(|&unit| u8::try_from(unit).map_err(|_| LiteralError::ByteOutOfRange))
            .collect::<Result<Vec<u8>, _>>()?;
        Ok(StringValue::Bytes(bytes.into()))
    }
}

/// Appends the value of one string token (as UTF-16 code units; a `str`
/// contributes one unit per byte) and returns whether it is unicode.
fn decode_string(
    token: &str,
    encoding: SourceEncoding,
    unicode_literals: bool,
    out: &mut Vec<u16>,
) -> Result<bool, LiteralError> {
    let bytes = token.as_bytes();
    let mut start = 0;
    let mut unicode = unicode_literals;
    match bytes.first() {
        Some(b'u' | b'U') => {
            unicode = true;
            start += 1;
        }
        Some(b'b' | b'B') => {
            unicode = false;
            start += 1;
        }
        _ => {}
    }
    let raw = matches!(bytes.get(start), Some(b'r' | b'R'));
    if raw {
        start += 1;
    }
    let quotes = if bytes.len() - start == 2 || bytes.get(start) != bytes.get(start + 1) {
        1
    } else {
        3
    };
    let body_start = (start + quotes).min(bytes.len());
    let body_end = bytes.len().saturating_sub(quotes).max(body_start);
    let body = &token[body_start..body_end];

    if !unicode {
        let mut encoded = Vec::with_capacity(body.len());
        encoding.encode(body, &mut encoded);
        if raw {
            out.extend_from_slice(&encoded);
        } else {
            decode_escapes(&encoded, false, out)?;
        }
    } else if raw {
        let mut units: Vec<u16> = Vec::with_capacity(body.len());
        units.extend(body.encode_utf16());
        decode_raw_unicode_escapes(&units, out)?;
    } else {
        let mut units: Vec<u16> = Vec::with_capacity(body.len());
        units.extend(body.encode_utf16());
        let before = out.len();
        decode_escapes(&units, true, out)?;
        if char::decode_utf16(out[before..].iter().copied()).any(|c| c.is_err()) {
            return Err(LiteralError::UnpairedSurrogate);
        }
    }
    Ok(unicode)
}

/// `PyString.decode_UnicodeEscape`. In byte strings `\uXXXX` with four hex
/// digits is replaced by the UTF-8 encoding of the character (IA's Jython
/// does this; other `\u`, `\U` and `\N` sequences stay as they are).
fn decode_escapes(input: &[u16], unicode: bool, out: &mut Vec<u16>) -> Result<(), LiteralError> {
    let end = input.len();
    let mut s = 0;
    while s < end {
        let ch = input[s];
        if ch != u16::from(b'\\') {
            out.push(ch);
            s += 1;
            continue;
        }
        s += 1;
        if s == end {
            return Err(LiteralError::InvalidEscape);
        }
        let ch = input[s];
        s += 1;
        let Ok(byte) = u8::try_from(ch) else {
            out.push(u16::from(b'\\'));
            out.push(ch);
            continue;
        };
        match byte {
            b'\n' => {}
            b'\\' => out.push(u16::from(b'\\')),
            b'\'' => out.push(u16::from(b'\'')),
            b'"' => out.push(u16::from(b'"')),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b't' => out.push(u16::from(b'\t')),
            b'n' => out.push(u16::from(b'\n')),
            b'r' => out.push(u16::from(b'\r')),
            b'v' => out.push(0x0b),
            b'a' => out.push(0x07),
            b'0'..=b'7' => {
                let mut value = u16::from(byte - b'0');
                for _ in 0..2 {
                    match input.get(s) {
                        Some(&digit @ 0x30..=0x37) => {
                            value = (value << 3) + (digit - 0x30);
                            s += 1;
                        }
                        _ => break,
                    }
                }
                out.push(value);
            }
            b'x' => {
                let value = hex_escape(input, s, 2).ok_or(LiteralError::InvalidEscape)?;
                s += 2;
                out.push(value as u16);
            }
            b'u' if unicode => {
                let value = hex_escape(input, s, 4).ok_or(LiteralError::InvalidEscape)?;
                s += 4;
                out.push(value as u16);
            }
            b'u' => match hex_escape(input, s, 4).and_then(char::from_u32) {
                Some(c) => {
                    s += 4;
                    let mut buffer = [0u8; 4];
                    for &byte in c.encode_utf8(&mut buffer).as_bytes() {
                        out.push(u16::from(byte));
                    }
                }
                None => {
                    out.push(u16::from(b'\\'));
                    out.push(u16::from(b'u'));
                }
            },
            b'U' if unicode => {
                let value = hex_escape(input, s, 8).ok_or(LiteralError::InvalidEscape)?;
                s += 8;
                push_code_point(value, out)?;
            }
            b'N' if unicode => {
                if input.get(s) != Some(&u16::from(b'{')) {
                    return Err(LiteralError::InvalidEscape);
                }
                let name_start = s + 1;
                let Some(length) = input[name_start..]
                    .iter()
                    .position(|&unit| unit == u16::from(b'}'))
                else {
                    return Err(LiteralError::InvalidEscape);
                };
                let name = String::from_utf16_lossy(&input[name_start..name_start + length]);
                match crate::unicode_names::lookup(&name) {
                    Some(c) => {
                        s = name_start + length + 1;
                        push_code_point(c as u32, out)?;
                    }
                    None if crate::unicode_names::is_complete() => {
                        return Err(LiteralError::InvalidEscape);
                    }
                    None => {
                        // Not in our table: keep the escape as written.
                        out.push(u16::from(b'\\'));
                        out.push(u16::from(b'N'));
                    }
                }
            }
            _ => {
                out.push(u16::from(b'\\'));
                out.push(ch);
            }
        }
    }
    Ok(())
}

/// `digits` hex digits at `start`, if all present and valid.
fn hex_escape(input: &[u16], start: usize, digits: usize) -> Option<u32> {
    let slice = input.get(start..start + digits)?;
    let mut value: u32 = 0;
    for &unit in slice {
        let digit = char::from_u32(u32::from(unit))?.to_digit(16)?;
        value = (value << 4) | digit;
    }
    Some(value)
}

fn push_code_point(value: u32, out: &mut Vec<u16>) -> Result<(), LiteralError> {
    if value > 0x10FFFF {
        return Err(LiteralError::InvalidEscape);
    }
    if let Some(c) = char::from_u32(value) {
        let mut buffer = [0u16; 2];
        out.extend_from_slice(c.encode_utf16(&mut buffer));
    } else {
        // A surrogate code point.
        out.push(value as u16);
    }
    Ok(())
}

/// `codecs.PyUnicode_DecodeRawUnicodeEscape`: only `\uXXXX` and
/// `\UXXXXXXXX` preceded by an odd number of backslashes are decoded.
fn decode_raw_unicode_escapes(input: &[u16], out: &mut Vec<u16>) -> Result<(), LiteralError> {
    let size = input.len();
    let backslash = u16::from(b'\\');
    let mut i = 0;
    while i < size {
        let ch = input[i];
        if ch != backslash {
            out.push(ch);
            i += 1;
            continue;
        }
        let run_start = i;
        while i < size && input[i] == backslash {
            out.push(backslash);
            i += 1;
        }
        let next = input.get(i).copied();
        let odd = (i - run_start) % 2 == 1;
        let count = match next {
            Some(0x75) => 4, // u
            Some(0x55) => 8, // U
            _ => 0,
        };
        if !odd || count == 0 {
            continue;
        }
        out.pop();
        i += 1;
        let value = hex_escape(input, i, count).ok_or(LiteralError::InvalidEscape)?;
        i += count;
        if value > 0x10FFFF {
            return Err(LiteralError::InvalidEscape);
        }
        push_code_point(value, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_follow_jython_typing() {
        assert_eq!(integer("2147483647"), Ok(Number::Int(2147483647)));
        assert_eq!(integer("2147483648"), Ok(Number::Long("2147483648".into())));
        assert_eq!(integer("0777"), Ok(Number::Int(511)));
        assert_eq!(integer("0777L"), Ok(Number::Long("511".into())));
        assert_eq!(
            integer("0xffffffffffff"),
            Ok(Number::Long("281474976710655".into()))
        );
        // Twelve significant binary digits make a long even though small.
        assert_eq!(integer("0b111111111111"), Ok(Number::Long("4095".into())));
        assert_eq!(integer("0o"), Err(LiteralError::EmptyInteger));
        assert_eq!(
            integer("0x123456789abcdef0123456789"),
            Ok(Number::Long("90144042682896311822508713865".into()))
        );
    }

    #[test]
    fn strings_decode_like_jython() {
        let decode = |token: &str| decode_strings([token], SourceEncoding::Utf8, false);
        assert_eq!(
            decode("'\\u00e9'"),
            Ok(StringValue::Bytes(b"\xc3\xa9".to_vec().into()))
        );
        assert_eq!(
            decode("'\\U0001F600'"),
            Ok(StringValue::Bytes(b"\\U0001F600".to_vec().into()))
        );
        assert_eq!(decode("'\\400'"), Err(LiteralError::ByteOutOfRange));
        assert_eq!(
            decode("u'\\ud83d\\ude00'"),
            Ok(StringValue::Unicode("\u{1F600}".into()))
        );
        assert_eq!(decode("u'\\ud83d'"), Err(LiteralError::UnpairedSurrogate));
        assert_eq!(
            decode("ur'\\u1234\\\\u1234'"),
            Ok(StringValue::Unicode("\u{1234}\\\\u1234".into()))
        );
    }
}
