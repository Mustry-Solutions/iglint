//! Randomised robustness tests: no panics, sane ranges, printer round trips.

use iglint_expression::{
    ExprKind, ReferenceContext, dump, parse, parse_property_ref, parse_tag_path, to_source,
};
use iglint_text::TextSize;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

const PIECES: &[&str] = &[
    "(",
    ")",
    ",",
    "[",
    "]",
    "{",
    "}",
    "+",
    "-",
    "*",
    "/",
    "%",
    "^",
    "!",
    "~",
    "&&",
    "||",
    "&",
    "|",
    "xor",
    "=",
    "!=",
    "<",
    "<=",
    ">",
    ">=",
    "<<",
    ">>",
    "like",
    "true",
    "None",
    "1",
    "0x1F",
    "007",
    "1.5e3",
    ".5",
    "\"s\"",
    "'t'",
    "\"\\q\"",
    "{this.props.text}",
    "{[default]A/B.Quality}",
    "{../X.custom.y}",
    "{value}",
    "if",
    "f",
    "abc",
    " ",
    "\n",
    "\t",
    "//c\n",
    "//",
    ".",
    "#",
    "é",
    "😀",
    "\u{a0}",
    "\\",
    "\"",
    "'",
    "9223372036854775808",
    "==",
    "\r",
];

const BINARY: &[&str] = &[
    "+", "-", "*", "/", "%", "^", "&&", "||", "&", "|", "xor", "=", "!=", "<", "<=", ">", ">=",
    "<<", ">>", "like",
];

fn generate(rng: &mut Rng, depth: usize) -> String {
    let r = rng.below(100);
    if depth > 5 || r < 30 {
        return rng
            .pick(&[
                "1",
                "42",
                "0x2A",
                "2.5",
                "1e3",
                "\"a\"",
                "'b'",
                "true",
                "null",
                "{view.params.p}",
                "{[default]T}",
                "{value}",
            ])
            .to_owned();
    }
    match r {
        30..=59 => format!(
            "{} {} {}",
            generate(rng, depth + 1),
            rng.pick(BINARY),
            generate(rng, depth + 1)
        ),
        60..=69 => format!("{}{}", rng.pick(&["-", "!", "~"]), generate(rng, depth + 1)),
        70..=79 => format!("({})", generate(rng, depth + 1)),
        80..=91 => {
            let n = rng.below(4);
            let args: Vec<String> = (0..n).map(|_| generate(rng, depth + 1)).collect();
            format!(
                "{}({})",
                rng.pick(&["if", "len", "toStr", "coalesce"]),
                args.join(", ")
            )
        }
        _ => format!("{}[{}]", generate(rng, depth + 1), generate(rng, depth + 1)),
    }
}

#[test]
fn random_input_never_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..40_000 {
        let n = 1 + rng.below(12);
        let src: String = (0..n).map(|_| rng.pick(PIECES)).collect();
        let p = parse(&src);
        let len = TextSize::of(src.as_str());
        assert_eq!(p.is_valid(), p.errors.is_empty());
        for e in &p.errors {
            assert!(e.range.end() <= len, "{src:?}: {e}");
            assert!(src.is_char_boundary(e.range.start().to_usize()));
            assert!(src.is_char_boundary(e.range.end().to_usize()));
            let _ = e.to_string();
        }
        p.expr.walk(&mut |e| {
            assert!(e.range.end() <= len, "{src:?}");
            let _ = e.range.slice(&src);
            if let ExprKind::Reference(r) = &e.kind {
                assert_eq!(r.path_range.slice(&src), r.path);
                for ctx in [
                    ReferenceContext::PerspectiveBinding,
                    ReferenceContext::PerspectiveTransform,
                    ReferenceContext::Gateway,
                ] {
                    let _ = r.resolve(ctx);
                }
            }
        });
        let _ = dump(&p.expr);
        let _ = to_source(&p.expr);
        if p.is_valid() {
            let again = parse(&to_source(&p.expr));
            assert!(again.is_valid(), "{src:?}");
            assert_eq!(dump(&again.expr), dump(&p.expr), "{src:?}");
        }
    }
}

#[test]
fn generated_expressions_round_trip() {
    let mut rng = Rng(42);
    for _ in 0..5000 {
        let src = generate(&mut rng, 0);
        let p = parse(&src);
        assert!(p.is_valid(), "{src:?}: {:?}", p.errors);
        assert_eq!(p.expr.range.slice(&src), src.trim());
        let printed = to_source(&p.expr);
        let again = parse(&printed);
        assert_eq!(dump(&again.expr), dump(&p.expr), "{src:?} -> {printed:?}");
        // Every child range lies inside its parent's.
        p.expr.walk(&mut |e| {
            for c in e.children() {
                assert!(e.range.contains_range(c.range), "{src:?}");
            }
        });
    }
}

#[test]
fn random_reference_paths_never_panic() {
    let pieces = [
        "this",
        "view",
        "parent",
        ".",
        "/",
        "../",
        "./",
        "props",
        "custom",
        "x",
        "a b",
        "a-b",
        "[0]",
        "[]",
        "['k']",
        "[\"k\"]",
        "[",
        "]",
        "'",
        "\"",
        "é",
        "😀",
        "\n",
        "\t",
        "\\",
        ":",
        "~",
        "*",
        "[default]",
        "[.]",
        ",",
    ];
    let mut rng = Rng(7);
    for _ in 0..40_000 {
        let n = 1 + rng.below(8);
        let path: String = (0..n).map(|_| rng.pick(&pieces)).collect();
        let off = TextSize::new(3);
        if let Some(p) = parse_property_ref(&path, off) {
            for s in &p.segments {
                assert!(s.range.start() >= off);
                assert!(s.range.end().to_usize() <= path.len() + 3);
            }
        }
        let t = parse_tag_path(&path, off);
        for c in &t.components {
            assert!(c.range.end().to_usize() <= path.len() + 3);
        }
        if let Some(e) = &t.error {
            assert!(e.range.end().to_usize() <= path.len() + 3);
        }
    }
}
