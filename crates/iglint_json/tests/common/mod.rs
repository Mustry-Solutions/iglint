//! Shared helpers for the integration tests: a deterministic RNG, an owned
//! JSON tree and a Gson-style writer.
#![allow(dead_code)]

use iglint_json::{EscapeStyle, Value, ValueKind, to_json_string};

/// xorshift64*: deterministic, dependency-free.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// An owned JSON tree used as the expected value.
#[derive(Clone, Debug, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

pub fn from_value(v: &Value<'_>) -> J {
    match &v.kind {
        ValueKind::Null => J::Null,
        ValueKind::Bool(b) => J::Bool(*b),
        ValueKind::Number(n) => J::Num(n.raw().to_owned()),
        ValueKind::String(s) => J::Str(s.value().to_owned()),
        ValueKind::Array(a) => J::Arr(a.iter().map(from_value).collect()),
        ValueKind::Object(o) => J::Obj(
            o.iter()
                .map(|m| (m.key().to_owned(), from_value(&m.value)))
                .collect(),
        ),
    }
}

pub const ALPHABET: &[&str] = &[
    "a", "b", "Z", "0", " ", "_", ".", "/", "\\", "\"", "'", "=", "<", ">", "&", "\n", "\t", "\r",
    "\u{0}", "\u{1f}", "é", "€", "😀", "\u{2028}", "{", "}", "[", "]", ",", ":", "\u{7f}",
    "\u{FFFD}",
];

pub fn gen_string(rng: &mut Rng) -> String {
    let len = rng.below(8);
    (0..len)
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

pub fn gen_number(rng: &mut Rng) -> String {
    let mut s = String::new();
    if rng.chance(30) {
        s.push('-');
    }
    if rng.chance(20) {
        s.push('0');
    } else {
        s.push(char::from(b'1' + rng.below(9) as u8));
        for _ in 0..rng.below(6) {
            s.push(char::from(b'0' + rng.below(10) as u8));
        }
    }
    if rng.chance(30) {
        s.push('.');
        for _ in 0..1 + rng.below(4) {
            s.push(char::from(b'0' + rng.below(10) as u8));
        }
    }
    if rng.chance(15) {
        s.push(if rng.chance(50) { 'e' } else { 'E' });
        match rng.below(3) {
            0 => s.push('+'),
            1 => s.push('-'),
            _ => {}
        }
        s.push(char::from(b'0' + rng.below(10) as u8));
    }
    s
}

pub fn gen_value(rng: &mut Rng, depth: usize) -> J {
    let leaf = depth >= 4 || rng.chance(40);
    if leaf {
        match rng.below(5) {
            0 => J::Null,
            1 => J::Bool(rng.chance(50)),
            2 => J::Num(gen_number(rng)),
            _ => J::Str(gen_string(rng)),
        }
    } else if rng.chance(50) {
        J::Arr(
            (0..rng.below(5))
                .map(|_| gen_value(rng, depth + 1))
                .collect(),
        )
    } else {
        let mut members = Vec::new();
        for _ in 0..rng.below(6) {
            let key = if rng.chance(20) && !members.is_empty() {
                // Duplicate keys must survive.
                let (k, _): &(String, J) = &members[rng.below(members.len())];
                k.clone()
            } else {
                gen_string(rng)
            };
            members.push((key, gen_value(rng, depth + 1)));
        }
        J::Obj(members)
    }
}

/// Writes `j` the way Gson pretty-prints (or compactly).
pub fn write(j: &J, pretty: bool, style: EscapeStyle, indent: usize, out: &mut String) {
    let nl = |out: &mut String, level: usize| {
        if pretty {
            out.push('\n');
            for _ in 0..level {
                out.push_str("  ");
            }
        }
    };
    match j {
        J::Null => out.push_str("null"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Num(n) => out.push_str(n),
        J::Str(s) => out.push_str(&to_json_string(s, style)),
        J::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                nl(out, indent + 1);
                write(item, pretty, style, indent + 1, out);
            }
            if !items.is_empty() {
                nl(out, indent);
            }
            out.push(']');
        }
        J::Obj(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                nl(out, indent + 1);
                out.push_str(&to_json_string(k, style));
                out.push_str(if pretty { ": " } else { ":" });
                write(v, pretty, style, indent + 1, out);
            }
            if !members.is_empty() {
                nl(out, indent);
            }
            out.push('}');
        }
    }
}

pub fn render(j: &J, pretty: bool, style: EscapeStyle) -> String {
    let mut s = String::new();
    write(j, pretty, style, 0, &mut s);
    s
}
