//! The Unicode character names of `\N{...}` escapes, resolved the way IA's
//! Jython 2.7.3 resolves them (`org.python.modules.ucnhash`).
//!
//! Jython splits a name into words (at a space, and after a hyphen that does
//! not start a word), replaces every word of its word list by the word's
//! index, spells out the other words, and looks the resulting bytes up in a
//! table of about 29,500 names (Unicode 9.0, without algorithmic names such as
//! `CJK UNIFIED IDEOGRAPH-4E00` or `HANGUL SYLLABLE GA`, and without aliases).
//! Letters are matched case-insensitively. The encoding has a few quirks
//! that this module repeats: a space before a listed word may be doubled or
//! lead the name, a listed final word may be followed by one space, and some
//! names are spelled with a hyphen joined to the next word.
//!
//! A name starting with `CJK COMPATIBILITY IDEOGRAPH-` (exact case) is not
//! looked up: its rest is parsed with Java's `Integer.parseInt(rest, 16)`
//! and the result is the code point.
//!
//! `table.bin` and `table.rs` are generated from Jython's table by
//! `oracle/generate-unicode-names.sh`. `table.bin` holds:
//!
//! * the word list sorted by text: per word its end offset in the word text
//!   (u16 LE) and Jython's index (u16 LE), then the word text;
//! * per block of `BLOCK_LEN` names, the offset of its first entry (u32 LE);
//! * the encoded names sorted by encoding, front coded within a block: per
//!   name the length shared with the previous name, the length of the rest,
//!   the rest, and the code point as a varint (the first of a block) or a
//!   zigzag varint difference from the previous name.

mod table;

use std::cmp::Ordering;

use table::{
    BLOCK_COUNT, BLOCK_LEN, BLOCKS, DATA, ENTRIES, ENTRY_COUNT, HEX_DIGITS, MAX_NAME_LEN,
    WORD_COUNT, WORD_CUTOFF, WORD_START, WORDS_TEXT,
};

/// The longest name (in UTF-16 units) that Jython looks for before its
/// closing brace.
pub(crate) const MAX_LENGTH: usize = MAX_NAME_LEN + 1;

/// Jython's encoding buffer; longer encodings make its lookup fail.
const RAW_LEN: usize = 32;

const CJK_PREFIX: &str = "CJK COMPATIBILITY IDEOGRAPH-";

/// Characters of encoded names, by code.
const CHARMAP: &[u8; 40] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-()";

/// The value Jython gives the name of a `\N{...}` escape (`ucnhash.getValue`),
/// if any. The value may still be no valid character.
pub(crate) fn lookup(name: &[u16]) -> Option<u32> {
    let prefix = CJK_PREFIX.as_bytes();
    if name.len() >= prefix.len()
        && name
            .iter()
            .zip(prefix)
            .all(|(&unit, &byte)| unit == u16::from(byte))
    {
        return parse_java_hex(&name[prefix.len()..]);
    }
    let (raw, length) = encode(name)?;
    find(&raw[..length])
}

/// `Integer.parseInt(text, 16)`, mapped to `None` where that fails or the
/// result is negative.
fn parse_java_hex(text: &[u16]) -> Option<u32> {
    let (&first, _) = text.split_first()?;
    let (negative, digits) = match first {
        0x2D => (true, &text[1..]),
        0x2B => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: u64 = 0;
    for &unit in digits {
        value = value * 16 + u64::from(java_hex_digit(unit)?);
        if value > 1 << 31 {
            return None;
        }
    }
    match (negative, value) {
        (_, 0) => Some(0),
        (true, _) => None,
        (false, v) => u32::try_from(v).ok().filter(|&v| v <= i32::MAX as u32),
    }
}

/// `Character.digit(unit, 16)`.
fn java_hex_digit(unit: u16) -> Option<u8> {
    let index = HEX_DIGITS.partition_point(|&(_, last, _)| last < unit);
    let &(first, _, value) = HEX_DIGITS.get(index)?;
    (first <= unit).then(|| value + (unit - first) as u8)
}

/// Encodes a name like `ucnhash.lookup`; `None` where Jython gives up (a
/// character outside letters, digits, space, `-`, `(` and `)`, or an
/// encoding longer than its buffer).
fn encode(name: &[u16]) -> Option<([u8; RAW_LEN], usize)> {
    let end = name.len();
    let mut raw = [0u8; RAW_LEN];
    let mut ridx = 0;
    let mut rstart = 0;
    let mut start = 0;
    loop {
        let rbegin = ridx;
        let mut i = start;
        while i < end {
            let unit = name[i];
            let code = match u8::try_from(unit).ok()? {
                b' ' => {
                    start = i + 1;
                    break;
                }
                c @ b'a'..=b'z' => c - b'a' + 1,
                c @ b'A'..=b'Z' => c - b'A' + 1,
                c @ b'0'..=b'9' => c - b'0' + 27,
                b'-' => 37,
                b'(' => 38,
                b')' => 39,
                _ => return None,
            };
            *raw.get_mut(ridx)? = code;
            ridx += 1;
            if unit == u16::from(b'-') && start != i {
                i += 1;
                start = i;
                break;
            }
            i += 1;
        }
        let word = if ridx - rbegin > 1 {
            word_index(&raw[rbegin..ridx])
        } else {
            None
        };
        match word {
            Some(index) => {
                let value = index + WORD_START;
                ridx = rstart;
                if value > WORD_CUTOFF {
                    *raw.get_mut(ridx)? = ((value >> 8) + WORD_CUTOFF) as u8;
                    *raw.get_mut(ridx + 1)? = (value & 0xFF) as u8;
                    ridx += 2;
                } else {
                    *raw.get_mut(ridx)? = value as u8;
                    ridx += 1;
                }
                rstart = ridx;
                if i >= end {
                    break;
                }
            }
            None => {
                rstart = ridx;
                if i >= end {
                    break;
                }
                *raw.get_mut(ridx)? = 0;
                ridx += 1;
            }
        }
    }
    Some((raw, ridx))
}

fn read_u16(at: usize) -> u16 {
    u16::from_le_bytes([DATA[at], DATA[at + 1]])
}

fn read_u32(at: usize) -> usize {
    u32::from_le_bytes([DATA[at], DATA[at + 1], DATA[at + 2], DATA[at + 3]]) as usize
}

/// Jython's index of the word spelled by `codes`, if it is listed.
fn word_index(codes: &[u8]) -> Option<u16> {
    let word = |i: usize| {
        let start = if i == 0 {
            0
        } else {
            usize::from(read_u16(4 * (i - 1)))
        };
        let end = usize::from(read_u16(4 * i));
        &DATA[WORDS_TEXT + start..WORDS_TEXT + end]
    };
    let compare = |i: usize| {
        let text = word(i);
        let spelled = codes.iter().map(|&code| CHARMAP[usize::from(code)]);
        text.iter().copied().cmp(spelled)
    };
    let (mut low, mut high) = (0, WORD_COUNT);
    while low < high {
        let middle = (low + high) / 2;
        match compare(middle) {
            Ordering::Less => low = middle + 1,
            Ordering::Greater => high = middle,
            Ordering::Equal => return Some(read_u16(4 * middle + 2)),
        }
    }
    None
}

fn varint(at: &mut usize) -> u32 {
    let mut value = 0u32;
    let mut shift = 0;
    loop {
        let byte = DATA[*at];
        *at += 1;
        value |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// The code point of an encoded name.
fn find(raw: &[u8]) -> Option<u32> {
    let block_start = |block: usize| ENTRIES + read_u32(BLOCKS + 4 * block);
    // The first name of a block shares nothing with its predecessor.
    let first_key = |block: usize| {
        let at = block_start(block);
        let length = usize::from(DATA[at + 1]);
        &DATA[at + 2..at + 2 + length]
    };
    // The last block whose first name is not after `raw`.
    let (mut low, mut high) = (0, BLOCK_COUNT);
    while low < high {
        let middle = (low + high) / 2;
        if first_key(middle) <= raw {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let block = low.checked_sub(1)?;
    let mut at = block_start(block);
    let count = BLOCK_LEN.min(ENTRY_COUNT - block * BLOCK_LEN);
    let mut key = [0u8; RAW_LEN];
    let mut code_point = 0u32;
    for index in 0..count {
        let shared = usize::from(DATA[at]);
        let rest = usize::from(DATA[at + 1]);
        at += 2;
        key[shared..shared + rest].copy_from_slice(&DATA[at..at + rest]);
        at += rest;
        let value = varint(&mut at);
        code_point = if index == 0 {
            value
        } else {
            // Zigzag difference from the previous name.
            let delta = i64::from(value >> 1) ^ -i64::from(value & 1);
            (i64::from(code_point) + delta) as u32
        };
        match key[..shared + rest].cmp(raw) {
            Ordering::Equal => return Some(code_point),
            Ordering::Greater => return None,
            Ordering::Less => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(name: &str) -> Option<u32> {
        lookup(&name.encode_utf16().collect::<Vec<_>>())
    }

    #[test]
    fn names_resolve_like_jython() {
        assert_eq!(value("BULLET"), Some(0x2022));
        assert_eq!(value("bullet"), Some(0x2022));
        assert_eq!(value("PILE OF POO"), Some(0x1F4A9));
        assert_eq!(value("LATIN CAPITAL LETTER A"), Some(0x41));
        assert_eq!(value("SINGLE LOW-9 QUOTATION MARK"), Some(0x201A));
        assert_eq!(value("FORM FEED (FF)"), Some(0x0C));
        assert_eq!(
            value("TIBETAN MARK GTER YIG MGO -UM RNAM BCAD MA"),
            Some(0x0F02)
        );
        assert_eq!(
            value("BYZANTINE MUSICAL SYMBOL FHTORA SKLIRON CHROMA VASIS"),
            Some(0x1D0C5)
        );
        assert_eq!(value("THIS IS NOT A UNICODE NAME"), None);
        assert_eq!(value("CJK UNIFIED IDEOGRAPH-4E00"), None);
        assert_eq!(value("HANGUL SYLLABLE GA"), None);
        assert_eq!(value("LATIN CAPITAL LETTER GHA"), None);
        assert_eq!(value(""), None);
    }

    #[test]
    fn encoding_quirks_match_jython() {
        assert_eq!(value("BULLET "), Some(0x2022));
        assert_eq!(value(" BULLET"), Some(0x2022));
        assert_eq!(value("BLACK  STAR"), Some(0x2605));
        assert_eq!(value("BLACK   STAR"), None);
        assert_eq!(value("LATIN SMALL LETTER A "), None);
        assert_eq!(value("LATIN SMALL LETTER  A"), None);
        assert_eq!(value("TIBETAN MARK BKA- SHOG YIG MGO"), None);
    }

    #[test]
    fn cjk_compatibility_ideographs_are_parsed() {
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH-F900"), Some(0xF900));
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH-41"), Some(0x41));
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH-+41"), Some(0x41));
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH--0"), Some(0));
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH--1"), None);
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH-"), None);
        assert_eq!(
            value("CJK COMPATIBILITY IDEOGRAPH-\u{FF26}\u{0661}"),
            Some(0xF1)
        );
        assert_eq!(
            value("CJK COMPATIBILITY IDEOGRAPH-7FFFFFFF"),
            Some(0x7FFF_FFFF)
        );
        assert_eq!(value("CJK COMPATIBILITY IDEOGRAPH-80000000"), None);
    }
}
