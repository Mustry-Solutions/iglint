use iglint_json::{
    Expected, ParseErrorKind, ParseOptions, PathSegment, Value, ValueKind, parse, parse_with,
};
use iglint_text::{Ranged, TextRange, TextSize};

fn r(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::new(start), TextSize::new(end))
}

fn ok(text: &str) -> Value<'_> {
    match parse(text) {
        Ok(v) => v,
        Err(e) => panic!("{text:?} failed: {e}"),
    }
}

fn err(text: &str) -> (ParseErrorKind, TextRange) {
    match parse(text) {
        Ok(v) => panic!("{text:?} parsed: {v:?}"),
        Err(e) => (e.kind, e.range),
    }
}

#[test]
fn scalars() {
    assert!(ok("null").is_null());
    assert_eq!(ok("true").as_bool(), Some(true));
    assert_eq!(ok("false").as_bool(), Some(false));
    assert_eq!(ok("0").as_i64(), Some(0));
    assert_eq!(ok("-0").as_i64(), Some(0));
    assert_eq!(ok("42").as_i64(), Some(42));
    assert_eq!(ok("-42").as_i64(), Some(-42));
    assert_eq!(ok("1.5").as_f64(), Some(1.5));
    assert_eq!(ok("1.5").as_i64(), None);
    assert_eq!(ok("1e3").as_f64(), Some(1000.0));
    assert_eq!(ok("1E+3").as_f64(), Some(1000.0));
    assert_eq!(ok("2.5e-3").as_f64(), Some(0.0025));
    assert_eq!(ok("\"abc\"").as_str(), Some("abc"));
    assert_eq!(ok("\"\"").as_str(), Some(""));
}

#[test]
fn number_raw_text_is_kept() {
    for raw in [
        "1.0",
        "-2.842170943040401e-14",
        "100",
        "0.000",
        "1E10",
        "-0.0",
    ] {
        let v = ok(raw);
        assert_eq!(v.as_number().unwrap().raw(), raw);
        assert_eq!(v.range, r(0, raw.len() as u32));
    }
    assert!(ok("1.0").as_number().unwrap().as_f64().is_some());
    assert!(!ok("1.0").as_number().unwrap().is_integer_literal());
    assert!(ok("10").as_number().unwrap().is_integer_literal());
    assert_eq!(ok("9223372036854775807").as_i64(), Some(i64::MAX));
    assert_eq!(ok("9223372036854775808").as_i64(), None);
    assert_eq!(
        ok("9223372036854775808").as_number().unwrap().as_u64(),
        Some(9223372036854775808)
    );
    assert_eq!(ok("-1").as_number().unwrap().as_u64(), None);
}

#[test]
fn special_floats_are_tolerated_by_default() {
    assert!(ok("NaN").as_f64().unwrap().is_nan());
    assert_eq!(ok("Infinity").as_f64(), Some(f64::INFINITY));
    assert_eq!(ok("-Infinity").as_f64(), Some(f64::NEG_INFINITY));
    assert!(ok("NaN").as_number().unwrap().is_special());
    assert_eq!(ok("[NaN, -Infinity]").as_array().unwrap().len(), 2);
    let strict = ParseOptions::strict();
    assert!(parse_with("NaN", strict).is_err());
    assert!(parse_with("Infinity", strict).is_err());
    assert!(parse_with("-Infinity", strict).is_err());
    assert!(parse("Nan").is_err());
    assert!(parse("-Inf").is_err());
}

#[test]
fn invalid_numbers() {
    for text in [
        "01", "-", "1.", ".5", "1e", "1e+", "+1", "1.e5", "0x10", "1a", "--1", "-.5", "00",
    ] {
        let e = parse(text);
        assert!(e.is_err(), "{text:?} should fail");
    }
    let (kind, range) = err("01");
    assert_eq!(kind, ParseErrorKind::InvalidNumber);
    assert_eq!(range, r(0, 2));
    let (kind, range) = err("[1.]");
    assert_eq!(kind, ParseErrorKind::InvalidNumber);
    assert_eq!(range, r(1, 3));
    let (kind, _) = err("-");
    assert_eq!(kind, ParseErrorKind::InvalidNumber);
}

#[test]
fn ranges_of_values_keys_and_members() {
    let text = r#"{"a": [1, true], "bc": {"d": null}}"#;
    let v = ok(text);
    assert_eq!(v.range, r(0, text.len() as u32));
    let o = v.as_object().unwrap();
    assert_eq!(o.members[0].key.range(), r(1, 4));
    assert_eq!(o.members[0].key.content_range(), r(2, 3));
    assert_eq!(o.members[0].value.range.slice(text), "[1, true]");
    assert_eq!(o.members[0].range().slice(text), r#""a": [1, true]"#);
    let arr = o.members[0].value.as_array().unwrap();
    assert_eq!(arr.elements[1].range.slice(text), "true");
    assert_eq!(o.members[1].key.range().slice(text), "\"bc\"");
    assert_eq!(
        v.get("bc").unwrap().get("d").unwrap().range.slice(text),
        "null"
    );
}

#[test]
fn whitespace_around_root() {
    let v = ok(" \n\t\r {\"a\": 1} \n");
    assert_eq!(v.range, r(5, 13));
    assert!(parse("").is_err());
    assert!(parse("   ").is_err());
}

#[test]
fn bom_is_skipped() {
    let text = "\u{FEFF}{\"a\": 1}";
    let v = ok(text);
    assert_eq!(v.range.start(), TextSize::new(3));
    assert_eq!(v.get("a").unwrap().as_i64(), Some(1));
}

#[test]
fn trailing_content() {
    let (kind, range) = err("{} x");
    assert_eq!(kind, ParseErrorKind::TrailingContent);
    assert_eq!(range, r(3, 4));
    assert_eq!(err("1 2").0, ParseErrorKind::TrailingContent);
    assert_eq!(err("{}}").0, ParseErrorKind::TrailingContent);
    let (kind, range) = err("{} é");
    assert_eq!(kind, ParseErrorKind::TrailingContent);
    assert_eq!(range, r(3, 5));
}

#[test]
fn trailing_commas_are_errors() {
    let (kind, range) = err("[1, 2,]");
    assert_eq!(kind, ParseErrorKind::TrailingComma);
    assert_eq!(range, r(5, 6));
    let (kind, range) = err("{\"a\": 1,\n}");
    assert_eq!(kind, ParseErrorKind::TrailingComma);
    assert_eq!(range, r(7, 8));
}

#[test]
fn structural_errors() {
    assert_eq!(
        err("[1 2]").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::CommaOrCloseBracket
        }
    );
    assert_eq!(
        err("{\"a\" 1}").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Colon
        }
    );
    assert_eq!(
        err("{a: 1}").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Key
        }
    );
    assert_eq!(
        err("{\"a\": 1 \"b\": 2}").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::CommaOrCloseBrace
        }
    );
    assert_eq!(
        err("[").0,
        ParseErrorKind::UnexpectedEof {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("[1").0,
        ParseErrorKind::UnexpectedEof {
            expected: Expected::CommaOrCloseBracket
        }
    );
    assert_eq!(
        err("{\"a\"").0,
        ParseErrorKind::UnexpectedEof {
            expected: Expected::Colon
        }
    );
    assert_eq!(
        err("{\"a\":").0,
        ParseErrorKind::UnexpectedEof {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("{").0,
        ParseErrorKind::UnexpectedEof {
            expected: Expected::Key
        }
    );
    assert_eq!(
        err("[,]").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("tru").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("nul").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("True").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("'a'").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
    assert_eq!(
        err("// c\n1").0,
        ParseErrorKind::UnexpectedCharacter {
            expected: Expected::Value
        }
    );
}

#[test]
fn non_json_whitespace_is_rejected() {
    assert!(parse("\u{c}1").is_err());
    assert!(parse("1\u{a0}").is_err());
    assert!(parse("[1,\u{b}2]").is_err());
}

#[test]
fn string_errors() {
    let (kind, range) = err("\"abc");
    assert_eq!(kind, ParseErrorKind::UnterminatedString);
    assert_eq!(range, r(0, 4));
    let (kind, range) = err("\"a\nb\"");
    assert_eq!(kind, ParseErrorKind::ControlCharacterInString);
    assert_eq!(range, r(2, 3));
    assert_eq!(err("\"a\tb\"").0, ParseErrorKind::ControlCharacterInString);
    assert_eq!(
        err("\"a\u{0}b\"").0,
        ParseErrorKind::ControlCharacterInString
    );
    let (kind, range) = err(r#""a\qb""#);
    assert_eq!(kind, ParseErrorKind::InvalidEscape);
    assert_eq!(range, r(2, 4));
    let (kind, range) = err(r#""a\u12x4""#);
    assert_eq!(kind, ParseErrorKind::InvalidUnicodeEscape);
    assert_eq!(range, r(2, 6));
    assert_eq!(err(r#""\u""#).0, ParseErrorKind::InvalidUnicodeEscape);
    assert_eq!(err("\"\\").0, ParseErrorKind::UnterminatedString);
    assert_eq!(err("\"\\\"").0, ParseErrorKind::UnterminatedString);
    let (kind, range) = err("\"\\é\"");
    assert_eq!(kind, ParseErrorKind::InvalidEscape);
    assert_eq!(range, r(1, 4));
    // DEL and non-ASCII are allowed raw.
    assert_eq!(ok("\"\u{7f}é😀\"").as_str(), Some("\u{7f}é😀"));
}

#[test]
fn escapes_are_decoded() {
    let cases = [
        (r#""a\"b""#, "a\"b"),
        (r#""a\\b""#, "a\\b"),
        (r#""a\/b""#, "a/b"),
        (r#""\b\f\n\r\t""#, "\u{8}\u{c}\n\r\t"),
        (r#""\u0041\u00e9\u20AC""#, "Aé€"),
        (r#""\u003d\u0027\u0026\u003c\u003e""#, "='&<>"),
        (r#""\ud83d\ude00""#, "😀"),
        (r#""\uD83D\uDE00x""#, "😀x"),
        (r#""\ud83d""#, "\u{FFFD}"),
        (r#""\ude00""#, "\u{FFFD}"),
        (r#""\ud83dx""#, "\u{FFFD}x"),
        (r#""\ud83d\u0041""#, "\u{FFFD}A"),
        (r#""\u0000""#, "\u{0}"),
    ];
    for (raw, decoded) in cases {
        let v = ok(raw);
        let s = v.as_json_string().unwrap();
        assert_eq!(s.value(), decoded, "{raw}");
        assert!(s.has_escapes());
        assert_eq!(s.raw(), &raw[1..raw.len() - 1]);
    }
    let v = ok("\"plain\"");
    assert!(!v.as_json_string().unwrap().has_escapes());
}

#[test]
fn duplicate_keys_are_preserved() {
    let text = r#"{"a": 1, "b": 2, "a": 3, "c": 4, "b": 5, "a": 6}"#;
    let v = ok(text);
    let o = v.as_object().unwrap();
    assert_eq!(o.len(), 6);
    assert_eq!(o.get("a").unwrap().as_i64(), Some(6), "last wins");
    assert_eq!(o.position("b"), Some(4));
    assert_eq!(o.duplicate_keys(), vec![vec![0, 2, 5], vec![1, 4]]);
    assert_eq!(o.keys().collect::<Vec<_>>(), ["a", "b", "a", "c", "b", "a"]);
    assert!(
        ok(r#"{"a": 1, "b": 2}"#)
            .as_object()
            .unwrap()
            .duplicate_keys()
            .is_empty()
    );
    // Keys compare decoded.
    let v = ok(r#"{"a=": 1, "a\u003d": 2}"#);
    assert_eq!(v.as_object().unwrap().duplicate_keys(), vec![vec![0, 1]]);
}

#[test]
fn nesting_limit() {
    // Unlimited by default, like Gson, without recursion: parsed, walked
    // and dropped on a small stack.
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(|| {
            let n = 1_000_000;
            let arrays = "[".repeat(n) + "1" + &"]".repeat(n);
            let objects = r#"{"a": "#.repeat(n) + "[1, {}]" + &"}".repeat(n);
            let mixed = r#"[{"a": "#.repeat(n) + "1" + &"}]".repeat(n);
            for (text, depth, count) in [
                (arrays, n + 1, n + 1),
                (objects, n + 2, n + 3),
                (mixed, 2 * n + 1, 2 * n + 1),
            ] {
                let value = parse(&text).unwrap();
                let mut deepest = 0;
                let mut visited = 0;
                value.walk(|path, _| {
                    deepest = deepest.max(path.len() + 1);
                    visited += 1;
                    true
                });
                assert_eq!(deepest, depth);
                assert_eq!(visited, count);
                let truncated = &text[..text.len() - 1];
                assert!(parse(truncated).is_err());
                drop(value);
            }
        })
        .unwrap()
        .join()
        .unwrap();
    let opts = ParseOptions {
        max_depth: 3,
        ..ParseOptions::default()
    };
    assert!(parse_with("[[[1]]]", opts).is_ok());
    assert_eq!(
        parse_with("[[[[1]]]]", opts).unwrap_err().kind,
        ParseErrorKind::NestingTooDeep
    );
    assert_eq!(
        parse_with("{\"a\":{\"b\":{\"c\":{}}}}", opts)
            .unwrap_err()
            .kind,
        ParseErrorKind::NestingTooDeep
    );
    // Depth is per path, not total.
    assert!(parse_with("[[[1]],[[2]],[[3]]]", opts).is_ok());
}

#[test]
fn paths_and_pointers() {
    let text =
        r#"{"root": {"children": [{"meta": {"name": "Label"}}, {"meta": {"name": "a/b~c"}}]}}"#;
    let v = ok(text);
    let path = [
        PathSegment::from("root"),
        "children".into(),
        1usize.into(),
        "meta".into(),
        "name".into(),
    ];
    assert_eq!(v.get_path(&path).unwrap().as_str(), Some("a/b~c"));
    assert_eq!(
        v.pointer("/root/children/0/meta/name").unwrap().as_str(),
        Some("Label")
    );
    assert!(v.pointer("/root/children/2").is_none());
    assert!(v.pointer("/root/children/x").is_none());
    assert!(v.pointer("root").is_none());
    assert_eq!(v.pointer("").unwrap().range, v.range);
    let o = ok(r#"{"a/b": {"c~d": 1}}"#);
    assert_eq!(o.pointer("/a~1b/c~0d").unwrap().as_i64(), Some(1));
    assert!(o.pointer("/a~2b").is_none());
    let p = iglint_json::JsonPath(path.to_vec());
    assert_eq!(p.to_pointer(), "/root/children/1/meta/name");
    assert_eq!(p.to_dotted(), "root.children[1].meta.name");
    assert_eq!(
        iglint_json::JsonPath::from_pointer("/x~1y/0")
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn walk_visits_in_document_order() {
    let text = r#"{"a": [1, {"b": 2}], "c": 3}"#;
    let v = ok(text);
    let mut seen = Vec::new();
    v.walk(|path, value| {
        seen.push((
            iglint_json::JsonPath(path.to_vec()).to_pointer(),
            value.kind_name(),
        ));
        true
    });
    assert_eq!(
        seen,
        [
            ("".to_owned(), "object"),
            ("/a".to_owned(), "array"),
            ("/a/0".to_owned(), "number"),
            ("/a/1".to_owned(), "object"),
            ("/a/1/b".to_owned(), "number"),
            ("/c".to_owned(), "number"),
        ]
    );
    let mut count = 0;
    v.walk(|path, _| {
        count += 1;
        path.is_empty()
    });
    assert_eq!(count, 3, "skipping children of every non-root value");
}

#[test]
fn value_at_offset_finds_innermost() {
    let text = r#"{"a": [1, {"b": "xyz"}]}"#;
    let v = ok(text);
    let at = text.find("xyz").unwrap() as u32 + 1;
    let (path, value) = v.value_at_offset(TextSize::new(at)).unwrap();
    assert_eq!(iglint_json::JsonPath(path).to_pointer(), "/a/1/b");
    assert_eq!(value.as_str(), Some("xyz"));
    assert!(v.value_at_offset(TextSize::new(100)).is_none());
}

#[test]
fn kinds_and_accessors() {
    let v = ok(r#"[null, true, 1, "s", [], {}]"#);
    let names: Vec<_> = v.as_array().unwrap().iter().map(Value::kind_name).collect();
    assert_eq!(
        names,
        ["null", "boolean", "number", "string", "array", "object"]
    );
    assert!(v.index(4).unwrap().as_array().unwrap().is_empty());
    assert!(v.index(5).unwrap().as_object().unwrap().is_empty());
    assert!(v.get("x").is_none());
    assert!(v.index(9).is_none());
    assert!(matches!(v.index(0).unwrap().kind, ValueKind::Null));
    assert_eq!(v.index(3).unwrap().range(), r(16, 19));
}

#[test]
fn sorted_detection_uses_utf16_order() {
    assert!(
        ok(r#"{"a": 1, "b": 2, "c": 3}"#)
            .as_object()
            .unwrap()
            .is_sorted_like_ignition()
    );
    assert!(
        !ok(r#"{"b": 1, "a": 2}"#)
            .as_object()
            .unwrap()
            .is_sorted_like_ignition()
    );
    assert!(
        ok(r#"{"B": 1, "a": 2}"#)
            .as_object()
            .unwrap()
            .is_sorted_like_ignition()
    );
    // U+FF21 sorts before U+1F600 in UTF-16 order but after it by scalar value.
    assert!(
        ok("{\"\u{1F600}\": 1, \"\u{FF21}\": 2}")
            .as_object()
            .unwrap()
            .is_sorted_like_ignition()
    );
}

#[test]
fn error_display() {
    let e = parse("[1,]").unwrap_err();
    assert_eq!(e.to_string(), "trailing comma at 2..3");
    let e = parse("[").unwrap_err();
    assert_eq!(
        e.to_string(),
        "unexpected end of input, expected a value at 1..1"
    );
}
