//! Python 2 `repr()` of literal values, as printed by `ast.dump`.

use std::fmt::Write as _;

use crate::ast::{Number, StringValue};

/// `repr()` of a number value (`10`, `10L`, `1e+16`, `2j`).
pub fn number_repr(number: &Number) -> String {
    match number {
        Number::Int(value) => value.to_string(),
        Number::Long(digits) => format!("{digits}L"),
        Number::Float(value) => float_repr(*value),
        Number::Complex(imag) => format!("{}j", float_repr_style(*imag, false)),
    }
}

/// `repr()` of a float as Jython prints it: the digits of Java's
/// `Double.toString`, in exponent notation below 1e-4 and from 1e16.
pub fn float_repr(value: f64) -> String {
    float_repr_style(value, true)
}

fn float_repr_style(value: f64, add_dot_zero: bool) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if value == 0.0 {
        let sign = if value.is_sign_negative() { "-" } else { "" };
        return if add_dot_zero {
            format!("{sign}0.0")
        } else {
            format!("{sign}0")
        };
    }
    // Jython formats the digits of Java's `Double.toString`, rounded to at
    // most 17 significant digits.
    let (mut digits, dec_exponent) = crate::java_double::digits(value.abs());
    let mut exponent = dec_exponent - 1;
    if digits.len() > 17 {
        let next = digits[17];
        digits.truncate(17);
        if next >= 5 {
            let mut carried = true;
            for digit in digits.iter_mut().rev() {
                if *digit == 9 {
                    *digit = 0;
                } else {
                    *digit += 1;
                    carried = false;
                    break;
                }
            }
            if carried {
                digits[0] = 1;
                exponent += 1;
            }
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    let digits: String = digits
        .iter()
        .map(|&digit| char::from(b'0' + digit))
        .collect();
    let mut out = String::new();
    if value < 0.0 {
        out.push('-');
    }
    if !(-4..16).contains(&exponent) {
        // `digits` is ASCII and not empty.
        let (first, rest) = digits.split_at_checked(1).unwrap_or((&digits, ""));
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let sign = if exponent < 0 { '-' } else { '+' };
        let _ = write!(out, "e{sign}{:02}", exponent.abs());
    } else if exponent < 0 {
        out.push_str("0.");
        for _ in 0..(-exponent - 1) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        let whole = exponent as usize + 1;
        // `digits` is ASCII, so `whole` is a boundary.
        if let Some((int, frac)) = digits.split_at_checked(whole)
            && !frac.is_empty()
        {
            out.push_str(int);
            out.push('.');
            out.push_str(frac);
        } else {
            out.push_str(&digits);
            for _ in digits.len()..whole {
                out.push('0');
            }
            if add_dot_zero {
                out.push_str(".0");
            }
        }
    }
    out
}

/// `repr()` of a string value (`'x'`, `u'x'`).
pub fn string_repr(value: &StringValue) -> String {
    match value {
        StringValue::Bytes(bytes) => {
            let units: Vec<u32> = bytes.iter().map(|&byte| u32::from(byte)).collect();
            quote(&units, false)
        }
        StringValue::Unicode(text) => {
            let units: Vec<u32> = text.chars().map(|c| c as u32).collect();
            quote(&units, true)
        }
    }
}

/// `repr()` of a plain identifier string (`'x'`).
pub fn str_repr(text: &str) -> String {
    let units: Vec<u32> = text.bytes().map(u32::from).collect();
    quote(&units, false)
}

fn quote(code_points: &[u32], unicode: bool) -> String {
    let single = u32::from(b'\'');
    let double = u32::from(b'"');
    let quote = if code_points.contains(&single) && !code_points.contains(&double) {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(code_points.len() + 3);
    if unicode {
        out.push('u');
    }
    out.push(quote);
    for &point in code_points {
        match point {
            0x5C => out.push_str("\\\\"),
            0x09 => out.push_str("\\t"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            _ if point == quote as u32 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7E => out.push(char::from_u32(point).unwrap_or('?')),
            0x00..=0xFF => {
                let _ = write!(out, "\\x{point:02x}");
            }
            0x100..=0xFFFF => {
                let _ = write!(out, "\\u{point:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{point:08x}");
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats() {
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1e15), "1000000000000000.0");
        assert_eq!(float_repr(0.1), "0.1");
        assert_eq!(float_repr(1e-5), "1e-05");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(f64::INFINITY), "inf");
        assert_eq!(float_repr(1.2345678901234568e17), "1.2345678901234568e+17");
        assert_eq!(float_repr(2.5), "2.5");
        assert_eq!(float_repr(1e22), "1e+22");
        assert_eq!(float_repr(5e-324), "4.9e-324");
        assert_eq!(float_repr(6198704651e9), "6.1987046510000005e+18");
        assert_eq!(number_repr(&Number::Complex(2.0)), "2j");
        assert_eq!(number_repr(&Number::Complex(-0.0)), "-0j");
        assert_eq!(number_repr(&Number::Complex(1e20)), "1e+20j");
        assert_eq!(number_repr(&Number::Complex(1.5)), "1.5j");
    }

    #[test]
    fn strings() {
        assert_eq!(
            string_repr(&StringValue::Bytes(b"it's".to_vec().into())),
            "\"it's\""
        );
        assert_eq!(
            string_repr(&StringValue::Bytes(b"both ' and \"".to_vec().into())),
            "'both \\' and \"'"
        );
        assert_eq!(
            string_repr(&StringValue::Unicode("\u{e9}\u{1F600}\u{2022}".into())),
            "u'\\xe9\\U0001f600\\u2022'"
        );
    }
}
