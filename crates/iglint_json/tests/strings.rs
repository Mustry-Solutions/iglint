use iglint_json::{EscapeStyle, OffsetMap, detect_escape_style, parse, to_json_string};
use iglint_text::{TextRange, TextSize};

fn s(n: u32) -> TextSize {
    TextSize::new(n)
}

fn r(a: u32, b: u32) -> TextRange {
    TextRange::new(s(a), s(b))
}

/// Parses `{"k": <literal>}` and returns the file text plus the string.
fn with_string(literal: &str) -> String {
    format!("{{\"k\": {literal}}}")
}

#[test]
fn offsets_without_escapes_are_shifted() {
    let text = with_string("\"abc def\"");
    let v = parse(&text).unwrap();
    let js = v.get("k").unwrap().as_json_string().unwrap();
    // Content starts after `{"k": "` = 7 bytes.
    assert_eq!(js.content_range(), r(7, 14));
    assert_eq!(js.raw_offset(s(0)), s(7));
    assert_eq!(js.raw_offset(s(4)), s(11));
    assert_eq!(js.raw_offset(s(7)), s(14));
    assert_eq!(js.raw_offset(s(99)), s(14), "clamped to the closing quote");
    assert_eq!(js.raw_range(r(4, 7)).slice(&text), "def");
}

#[test]
fn offsets_across_simple_escapes() {
    // decoded: a \n b " c \\ d / e \t f
    let text = with_string(r#""a\nb\"c\\d\/e\tf""#);
    let v = parse(&text).unwrap();
    let js = v.get("k").unwrap().as_json_string().unwrap();
    assert_eq!(js.value(), "a\nb\"c\\d/e\tf");
    let decoded = js.value();
    let map = js.offset_map();
    assert_eq!(map.decoded_len(), TextSize::of(decoded));
    // Every decoded char maps to the raw bytes that produce it.
    let expected_raw = [
        "a", "\\n", "b", "\\\"", "c", "\\\\", "d", "\\/", "e", "\\t", "f",
    ];
    for (i, (offset, ch)) in decoded.char_indices().enumerate() {
        let range = TextRange::at(TextSize::from(offset), TextSize::from(ch.len_utf8()));
        assert_eq!(
            js.raw_range(range).slice(&text),
            expected_raw[i],
            "char {i}"
        );
    }
    assert_eq!(js.raw_range(r(0, decoded.len() as u32)), js.content_range());
}

#[test]
fn offsets_across_unicode_escapes() {
    // é escaped (2 decoded bytes from 6 raw), € raw (3 bytes), 😀 as an
    // escaped surrogate pair (4 decoded bytes from 12 raw), `=` escaped.
    let text = with_string("\"x\\u00e9€\\ud83d\\ude00\\u003dy\"");
    let v = parse(&text).unwrap();
    let js = v.get("k").unwrap().as_json_string().unwrap();
    let decoded = js.value();
    assert_eq!(decoded, "xé€😀=y");
    let expect = ["x", "\\u00e9", "€", "\\ud83d\\ude00", "\\u003d", "y"];
    for (i, (offset, ch)) in decoded.char_indices().enumerate() {
        let range = TextRange::at(TextSize::from(offset), TextSize::from(ch.len_utf8()));
        assert_eq!(js.raw_range(range).slice(&text), expect[i], "char {i}");
    }
    // Offsets inside a multi-byte escaped char: start rounds down, end up.
    let e_start = decoded.find('é').unwrap() as u32;
    let map = js.offset_map();
    assert_eq!(map.raw_offset(s(e_start + 1)), map.raw_offset(s(e_start)));
    assert_eq!(
        map.raw_offset_end(s(e_start + 1)),
        map.raw_offset(s(e_start + 2))
    );
    let emoji = decoded.find('😀').unwrap() as u32;
    for k in 1..4 {
        assert_eq!(map.raw_offset(s(emoji + k)), map.raw_offset(s(emoji)));
        assert_eq!(
            map.raw_offset_end(s(emoji + k)),
            map.raw_offset(s(emoji + 4))
        );
    }
    // A range that starts inside the emoji covers the whole escape pair.
    assert_eq!(
        js.raw_range(r(emoji + 1, emoji + 2)).slice(&text),
        "\\ud83d\\ude00"
    );
}

#[test]
fn lone_surrogate_maps_to_its_escape() {
    let text = with_string(r#""a\ud800b""#);
    let v = parse(&text).unwrap();
    let js = v.get("k").unwrap().as_json_string().unwrap();
    assert_eq!(js.value(), "a\u{FFFD}b");
    assert_eq!(js.raw_range(r(1, 4)).slice(&text), "\\ud800");
    assert_eq!(js.raw_range(r(4, 5)).slice(&text), "b");
}

#[test]
fn empty_ranges_map_to_points() {
    let text = with_string(r#""a\nb""#);
    let v = parse(&text).unwrap();
    let js = v.get("k").unwrap().as_json_string().unwrap();
    assert_eq!(js.raw_range(r(1, 1)), TextRange::empty(s(8)));
    assert_eq!(js.raw_range(r(2, 2)), TextRange::empty(s(10)));
    assert_eq!(js.raw_range(r(3, 3)), TextRange::empty(s(11)));
}

#[test]
fn offset_map_build_directly() {
    let map = OffsetMap::build(r"ab\tc", s(100));
    assert_eq!(map.decoded_len(), s(4));
    assert_eq!(map.raw_offset(s(0)), s(100));
    assert_eq!(map.raw_offset(s(2)), s(102));
    assert_eq!(map.raw_offset(s(3)), s(104));
    assert_eq!(map.raw_offset(s(4)), s(105));
    let empty = OffsetMap::build("", s(5));
    assert_eq!(empty.raw_offset(s(0)), s(5));
    assert_eq!(empty.raw_offset(s(3)), s(5));
}

#[test]
fn script_line_maps_to_file_line() {
    // A two-line script stored as one JSON string: diagnostics on line 2 of
    // the script must land after the `\n` escape in the file.
    let text = "{\n  \"script\": \"\\tx \\u003d 1\\n\\tsystem.util.foo(x)\"\n}";
    let v = parse(text).unwrap();
    let js = v.get("script").unwrap().as_json_string().unwrap();
    let decoded = js.value();
    assert_eq!(decoded, "\tx = 1\n\tsystem.util.foo(x)");
    let at = decoded.find("system").unwrap();
    let range = TextRange::at(TextSize::from(at), TextSize::of("system.util.foo"));
    assert_eq!(js.raw_range(range).slice(text), "system.util.foo");
    let eq = decoded.find('=').unwrap();
    assert_eq!(
        js.raw_range(TextRange::at(TextSize::from(eq), s(1)))
            .slice(text),
        "\\u003d"
    );
}

#[test]
fn every_offset_is_monotonic_and_in_bounds() {
    let literals = [
        r#""""#,
        r#""\"\"\"""#,
        "\"\\u0041\\u0042\"",
        "\"\\ud83d\\ude00\\ud83d\\ude01\"",
        "\"é\\u00e9é\"",
        r#""a\\\\b""#,
    ];
    for lit in literals {
        let text = with_string(lit);
        let v = parse(&text).unwrap();
        let js = v.get("k").unwrap().as_json_string().unwrap();
        let map = js.offset_map();
        let mut prev = map.raw_offset(s(0));
        assert_eq!(prev, js.content_range().start());
        for d in 0..=js.value().len() as u32 {
            let raw = map.raw_offset(s(d));
            assert!(raw >= prev);
            assert!(js.content_range().contains_inclusive(raw));
            assert!(map.raw_offset_end(s(d)) >= raw);
            prev = raw;
        }
        assert_eq!(prev, js.content_range().end());
    }
}

#[test]
fn escaping_ignition_style() {
    let cases = [
        ("plain", "\"plain\""),
        ("a\"b", r#""a\"b""#),
        ("a\\b", r#""a\\b""#),
        ("a/b", r#""a/b""#),
        ("\t\n\r\u{8}\u{c}", r#""\t\n\r\b\f""#),
        ("\u{0}\u{1f}", "\"\\u0000\\u001f\""),
        (
            "x = 'y' & <z>",
            "\"x \\u003d \\u0027y\\u0027 \\u0026 \\u003cz\\u003e\"",
        ),
        ("é€😀", "\"é€😀\""),
        ("\u{2028}\u{2029}", "\"\\u2028\\u2029\""),
        ("\u{7f}", "\"\u{7f}\""),
    ];
    for (input, expected) in cases {
        assert_eq!(
            to_json_string(input, EscapeStyle::Ignition),
            expected,
            "{input:?}"
        );
    }
}

#[test]
fn escaping_minimal_style() {
    assert_eq!(
        to_json_string("x = 'y' & <z>", EscapeStyle::Minimal),
        "\"x = 'y' & <z>\""
    );
    assert_eq!(
        to_json_string("a\"\\\n\u{1}", EscapeStyle::Minimal),
        r#""a\"\\\n\u0001""#
    );
    assert_eq!(
        to_json_string("\u{2028}", EscapeStyle::Minimal),
        "\"\u{2028}\""
    );
}

#[test]
fn escaping_round_trips() {
    let inputs = [
        "",
        "a",
        "\"\\/",
        "\u{0}\u{1}\u{1f} ",
        "x='1'<2>&3",
        "é😀\u{2028}",
        "line1\nline2\r\n\ttab",
    ];
    for input in inputs {
        for style in [EscapeStyle::Ignition, EscapeStyle::Minimal] {
            let json = to_json_string(input, style);
            let v = parse(&json).unwrap();
            assert_eq!(v.as_str(), Some(input), "{style:?} {json}");
        }
    }
}

#[test]
fn escape_style_detection() {
    let gson = parse("{\"expression\": \"{a} \\u003d 1\", \"b\": [\"x \\u0026 y\"]}").unwrap();
    assert_eq!(detect_escape_style(&gson), EscapeStyle::Ignition);
    let hand = parse(r#"{"expression": "{a} = 1"}"#).unwrap();
    assert_eq!(detect_escape_style(&hand), EscapeStyle::Minimal);
    let key = parse(r#"{"it's": 1}"#).unwrap();
    assert_eq!(detect_escape_style(&key), EscapeStyle::Minimal);
    let neutral = parse(r#"{"a": "b"}"#).unwrap();
    assert_eq!(detect_escape_style(&neutral), EscapeStyle::Ignition);
}
