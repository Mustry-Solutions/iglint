//! Randomised tests: no panics on arbitrary input, exact round trips of
//! generated documents, and edits that always produce the expected tree.

use iglint_json::{EscapeStyle, InsertPosition, JsonEditor, Value, ValueKind, parse};
use iglint_text::{TextRange, TextSize, apply_edits};

mod common;

use common::{J, Rng, from_value, gen_value, render};

/// Every container in `j` with a path to it.
fn containers(j: &J, path: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
    match j {
        J::Arr(items) => {
            out.push(path.clone());
            for (i, item) in items.iter().enumerate() {
                path.push(i);
                containers(item, path, out);
                path.pop();
            }
        }
        J::Obj(members) => {
            out.push(path.clone());
            for (i, (_, v)) in members.iter().enumerate() {
                path.push(i);
                containers(v, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn j_at<'a>(j: &'a mut J, path: &[usize]) -> &'a mut J {
    let mut cur = j;
    for &i in path {
        cur = match cur {
            J::Arr(items) => &mut items[i],
            J::Obj(members) => &mut members[i].1,
            _ => unreachable!(),
        };
    }
    cur
}

fn v_at<'v, 'a>(v: &'v Value<'a>, path: &[usize]) -> &'v Value<'a> {
    let mut cur = v;
    for &i in path {
        cur = match &cur.kind {
            ValueKind::Array(a) => &a.elements[i],
            ValueKind::Object(o) => &o.members[i].value,
            _ => unreachable!(),
        };
    }
    cur
}

#[test]
fn generated_documents_round_trip() {
    let mut rng = Rng::new(1);
    for case in 0..3000 {
        let j = gen_value(&mut rng, 0);
        for pretty in [true, false] {
            for style in [EscapeStyle::Ignition, EscapeStyle::Minimal] {
                let text = render(&j, pretty, style);
                let v = parse(&text).unwrap_or_else(|e| panic!("case {case}: {e}\n{text}"));
                assert_eq!(from_value(&v), j, "case {case}\n{text}");
                assert_eq!(
                    v.range,
                    TextRange::new(TextSize::new(0), TextSize::of(&text))
                );
                check_ranges(&v, &text);
            }
        }
    }
}

/// Ranges slice to text that parses to the same value, and decoded offsets
/// map inside the string literal.
fn check_ranges(v: &Value<'_>, text: &str) {
    v.walk(|_, value| {
        let slice = value.range.slice(text);
        let reparsed = parse(slice).unwrap();
        assert_eq!(from_value(&reparsed), from_value(value));
        if let Some(s) = value.as_json_string() {
            let map = s.offset_map();
            let decoded = s.value();
            assert_eq!(map.decoded_len(), TextSize::of(decoded));
            for (offset, ch) in decoded.char_indices() {
                let r = s.raw_range(TextRange::at(
                    TextSize::from(offset),
                    TextSize::from(ch.len_utf8()),
                ));
                let raw = r.slice(text);
                // The raw bytes of one decoded char decode to that char.
                let lit = format!("\"{raw}\"");
                assert_eq!(
                    parse(&lit).unwrap().as_str().unwrap(),
                    ch.to_string(),
                    "{raw:?}"
                );
            }
        }
        if let Some(o) = value.as_object() {
            for m in o {
                assert!(m.key.range().slice(text).starts_with('"'));
                assert_eq!(
                    parse(m.key.range().slice(text)).unwrap().as_str(),
                    Some(m.key())
                );
            }
        }
        true
    });
}

#[test]
fn removals_produce_expected_trees() {
    let mut rng = Rng::new(2);
    let mut checked = 0;
    for _ in 0..1500 {
        let j = gen_value(&mut rng, 0);
        let pretty = rng.chance(75);
        let text = render(&j, pretty, EscapeStyle::Ignition);
        let v = parse(&text).unwrap();
        let editor = JsonEditor::new(&text, &v);
        let mut paths = Vec::new();
        containers(&j, &mut Vec::new(), &mut paths);
        for path in paths {
            let target = v_at(&v, &path);
            let len = match &target.kind {
                ValueKind::Array(a) => a.len(),
                ValueKind::Object(o) => o.len(),
                _ => unreachable!(),
            };
            if len == 0 {
                continue;
            }
            // A random subset of indexes.
            let indices: Vec<usize> = (0..len).filter(|_| rng.chance(40)).collect();
            let edits = match &target.kind {
                ValueKind::Array(_) => editor.remove_elements(target, &indices).unwrap(),
                _ => editor.remove_members(target, &indices).unwrap(),
            };
            let out = apply_edits(&text, &edits).unwrap();
            let mut expected = j.clone();
            match j_at(&mut expected, &path) {
                J::Arr(items) => {
                    let mut i = 0;
                    items.retain(|_| {
                        i += 1;
                        !indices.contains(&(i - 1))
                    });
                }
                J::Obj(members) => {
                    let mut i = 0;
                    members.retain(|_| {
                        i += 1;
                        !indices.contains(&(i - 1))
                    });
                }
                _ => unreachable!(),
            }
            let reparsed = parse(&out).unwrap_or_else(|e| panic!("{e}\n{text}\n=>\n{out}"));
            assert_eq!(from_value(&reparsed), expected);
            if pretty {
                // Ignition layout is reproduced exactly.
                assert_eq!(
                    out,
                    render(&expected, true, EscapeStyle::Ignition),
                    "\n{text}\n=>\n{out}"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 1000);
}

#[test]
fn insertions_produce_expected_trees() {
    let mut rng = Rng::new(3);
    for _ in 0..1500 {
        let j = gen_value(&mut rng, 0);
        let pretty = rng.chance(75);
        let text = render(&j, pretty, EscapeStyle::Ignition);
        let v = parse(&text).unwrap();
        let editor = JsonEditor::new(&text, &v);
        let mut paths = Vec::new();
        containers(&j, &mut Vec::new(), &mut paths);
        for path in paths {
            let target = v_at(&v, &path);
            let new_value = gen_value(&mut rng, 3);
            let new_text = render(&new_value, pretty, EscapeStyle::Ignition);
            let mut expected = j.clone();
            let edit = match (&target.kind, j_at(&mut expected, &path)) {
                (ValueKind::Array(a), J::Arr(items)) => {
                    let index = rng.below(a.len() + 1);
                    items.insert(index, new_value.clone());
                    editor.insert_element(target, index, &new_text).unwrap()
                }
                (ValueKind::Object(o), J::Obj(members)) => {
                    let key = format!("k{}", rng.below(1000));
                    if o.contains_key(&key) {
                        continue;
                    }
                    let index = rng.below(o.len() + 1);
                    members.insert(index, (key.clone(), new_value.clone()));
                    editor
                        .insert_member(target, InsertPosition::Index(index), &key, &new_text)
                        .unwrap()
                }
                _ => unreachable!(),
            };
            let out = apply_edits(&text, &[edit]).unwrap();
            let reparsed = parse(&out).unwrap_or_else(|e| panic!("{e}\n{text}\n=>\n{out}"));
            assert_eq!(from_value(&reparsed), expected);
            if pretty && !text.is_empty() && text.contains('\n') {
                assert_eq!(
                    out,
                    render(&expected, true, EscapeStyle::Ignition),
                    "\n{text}\n=>\n{out}"
                );
            }
        }
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng::new(4);
    let pieces: &[&str] = &[
        "{", "}", "[", "]", ",", ":", "\"", "\\", "u", "d83d", "\\u", "0", "1", "-", ".", "e", "+",
        "true", "fals", "null", "NaN", "Infinity", " ", "\n", "\t", "é", "😀", "\u{0}", "a",
        "\u{FEFF}",
    ];
    for _ in 0..30000 {
        let n = rng.below(24);
        let text: String = (0..n).map(|_| pieces[rng.below(pieces.len())]).collect();
        check_no_panic(&text);
    }
}

#[test]
fn mutated_documents_never_panic() {
    let mut rng = Rng::new(5);
    for _ in 0..3000 {
        let j = gen_value(&mut rng, 0);
        let text = render(&j, rng.chance(50), EscapeStyle::Ignition);
        let mut bytes = text.into_bytes();
        for _ in 0..1 + rng.below(3) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len());
            match rng.below(3) {
                0 => {
                    bytes.remove(at);
                }
                1 => bytes.insert(at, b"{}[],:\"\\0-e.x \n"[rng.below(15)]),
                _ => bytes[at] = b"{}[],:\"\\0-e.x \n"[rng.below(15)],
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        check_no_panic(&text);
    }
}

fn check_no_panic(text: &str) {
    match parse(text) {
        Ok(v) => {
            // Valid input: every range is in bounds and on char boundaries.
            v.walk(|_, value| {
                let _ = value.range.slice(text);
                if let Some(s) = value.as_json_string() {
                    let map = s.offset_map();
                    for d in 0..=s.value().len() + 1 {
                        let raw = map.raw_offset(TextSize::from(d));
                        assert!(raw <= value.range.end());
                    }
                }
                true
            });
        }
        Err(e) => {
            assert!(e.range.end().to_usize() <= text.len(), "{e} for {text:?}");
            assert!(
                text.is_char_boundary(e.range.start().to_usize()),
                "{e} for {text:?}"
            );
            assert!(
                text.is_char_boundary(e.range.end().to_usize()),
                "{e} for {text:?}"
            );
            let _ = e.to_string();
        }
    }
}
