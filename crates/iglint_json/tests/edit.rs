//! Formatting-preserving edits, checked byte for byte against hand-written
//! expectations in Ignition's layout.

use iglint_json::{EditError, EscapeStyle, InsertPosition, JsonEditor, Value, parse, reindent};
use iglint_text::{TextEdit, TextRange, TextSize, apply_edits};

const VIEW: &str = include_str!("fixtures/view.json");

fn apply(src: &str, edits: &[TextEdit]) -> String {
    let out = apply_edits(src, edits).expect("edits do not overlap");
    if let Err(e) = parse(&out) {
        panic!("edited text is not valid JSON ({e}):\n{out}");
    }
    out
}

fn apply_one(src: &str, edit: TextEdit) -> String {
    apply(src, &[edit])
}

fn editor<'s>(src: &'s str, root: &Value<'_>) -> JsonEditor<'s> {
    JsonEditor::new(src, root)
}

#[test]
fn fixture_style_is_detected() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    assert_eq!(ed.indent_unit(), "  ");
    assert_eq!(ed.key_separator(), ": ");
    assert_eq!(ed.escape_style(), EscapeStyle::Ignition);
}

#[test]
fn remove_middle_member_on_own_line() {
    let src = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
    let root = parse(src).unwrap();
    let out = apply_one(src, editor(src, &root).remove_member(&root, 1).unwrap());
    assert_eq!(out, "{\n  \"a\": 1,\n  \"c\": 3\n}");
}

#[test]
fn remove_first_member_on_own_line() {
    let src = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
    let root = parse(src).unwrap();
    let out = apply_one(src, editor(src, &root).remove_member(&root, 0).unwrap());
    assert_eq!(out, "{\n  \"b\": 2,\n  \"c\": 3\n}");
}

#[test]
fn remove_last_member_fixes_preceding_comma() {
    let src = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
    let root = parse(src).unwrap();
    let out = apply_one(src, editor(src, &root).remove_member(&root, 2).unwrap());
    assert_eq!(out, "{\n  \"a\": 1,\n  \"b\": 2\n}");
}

#[test]
fn remove_only_member_leaves_empty_braces() {
    let src = "{\n  \"x\": {\n    \"only\": true\n  }\n}";
    let root = parse(src).unwrap();
    let x = root.get("x").unwrap();
    let out = apply_one(src, editor(src, &root).remove_member(x, 0).unwrap());
    assert_eq!(out, "{\n  \"x\": {}\n}");
}

#[test]
fn remove_multiline_member_in_fixture() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let child = root.pointer("/root/children/0").unwrap();
    let index = child.as_object().unwrap().position("position").unwrap();
    let out = apply_one(VIEW, ed.remove_member(child, index).unwrap());
    let expected = VIEW.replace(
        "        \"position\": {\n          \"height\": 32,\n          \"width\": 200\n        },\n",
        "",
    );
    assert_ne!(expected, VIEW);
    assert_eq!(out, expected);
}

#[test]
fn remove_unused_custom_property_in_fixture() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let custom = root.get("custom").unwrap();
    let out = apply_one(VIEW, ed.remove_member_by_key(custom, "unusedProp").unwrap());
    let expected = VIEW.replace(
        "    \"counter\": 0,\n    \"unusedProp\": \"x\"\n",
        "    \"counter\": 0\n",
    );
    assert_eq!(out, expected);
}

#[test]
fn remove_last_root_member_in_fixture() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let index = root.as_object().unwrap().position("root").unwrap();
    let out = apply_one(VIEW, ed.remove_member(&root, index).unwrap());
    let cut = VIEW.find(",\n  \"root\": {").unwrap();
    let expected = format!("{}\n}}", VIEW.get(..cut).unwrap());
    assert_eq!(out, expected);
}

#[test]
fn remove_whole_binding_in_fixture() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let pc = root.get("propConfig").unwrap();
    let binding_owner = pc.get("custom.counter").unwrap();
    let out = apply_one(
        VIEW,
        ed.remove_member_by_key(binding_owner, "binding").unwrap(),
    );
    let start = VIEW.find("    \"custom.counter\": {\n").unwrap();
    let end = VIEW.find("    \"params.lineName\"").unwrap();
    let expected = format!(
        "{}    \"custom.counter\": {{}},\n{}",
        VIEW.get(..start).unwrap(),
        VIEW.get(end..).unwrap()
    );
    assert_eq!(out, expected);
}

#[test]
fn remove_array_elements_in_fixture() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let children = root.pointer("/root/children").unwrap();
    let first = children.index(0).unwrap().range;
    let second = children.index(1).unwrap().range;

    // Remove the first child: its lines go, the second keeps its indentation.
    let out = apply_one(VIEW, ed.remove_element(children, 0).unwrap());
    let line_start =
        |offset: TextSize| VIEW.get(..offset.to_usize()).unwrap().rfind('\n').unwrap() + 1;
    let expected = format!(
        "{}{}",
        VIEW.get(..line_start(first.start())).unwrap(),
        VIEW.get(line_start(second.start())..).unwrap()
    );
    assert_eq!(out, expected);

    // Remove the last child: the comma after the first goes with it.
    let out = apply_one(VIEW, ed.remove_element(children, 1).unwrap());
    let expected = format!(
        "{}{}",
        VIEW.get(..first.end().to_usize()).unwrap(),
        VIEW.get(second.end().to_usize()..).unwrap()
    );
    assert_eq!(out, expected);

    // Remove both: `[]`.
    let edits = ed.remove_elements(children, &[1, 0]).unwrap();
    let out = apply(VIEW, &edits);
    let expected = format!(
        "{}[]{}",
        VIEW.get(..children.range.start().to_usize()).unwrap(),
        VIEW.get(children.range.end().to_usize()..).unwrap()
    );
    assert_eq!(out, expected);
}

#[test]
fn remove_several_members_at_once() {
    let src = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4,\n  \"e\": 5\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let cases: &[(&[usize], &str)] = &[
        (&[0, 1], "{\n  \"c\": 3,\n  \"d\": 4,\n  \"e\": 5\n}"),
        (&[1, 3], "{\n  \"a\": 1,\n  \"c\": 3,\n  \"e\": 5\n}"),
        (&[3, 4], "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}"),
        (&[0, 4], "{\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4\n}"),
        (&[1, 2, 4], "{\n  \"a\": 1,\n  \"d\": 4\n}"),
        (&[0, 1, 2, 3], "{\n  \"e\": 5\n}"),
        (&[1, 2, 3, 4], "{\n  \"a\": 1\n}"),
        (&[4, 3, 2, 1, 0], "{}"),
        (
            &[2, 2],
            "{\n  \"a\": 1,\n  \"b\": 2,\n  \"d\": 4,\n  \"e\": 5\n}",
        ),
        (&[], src),
    ];
    for (indices, expected) in cases {
        let edits = ed.remove_members(&root, indices).unwrap();
        assert_eq!(apply(src, &edits), *expected, "{indices:?}");
    }
}

#[test]
fn remove_from_inline_object() {
    let src = r#"{"a": 1, "b": [1, 2], "c": 3}"#;
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let cases: &[(&[usize], &str)] = &[
        (&[0], r#"{"b": [1, 2], "c": 3}"#),
        (&[1], r#"{"a": 1, "c": 3}"#),
        (&[2], r#"{"a": 1, "b": [1, 2]}"#),
        (&[0, 2], r#"{"b": [1, 2]}"#),
        (&[0, 1, 2], r#"{}"#),
    ];
    for (indices, expected) in cases {
        let edits = ed.remove_members(&root, indices).unwrap();
        assert_eq!(apply(src, &edits), *expected, "{indices:?}");
    }
}

#[test]
fn remove_from_compact_text() {
    let src = r#"{"a":1,"b":2,"c":[true,false,null]}"#;
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 0).unwrap()),
        r#"{"b":2,"c":[true,false,null]}"#
    );
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 2).unwrap()),
        r#"{"a":1,"b":2}"#
    );
    let arr = root.get("c").unwrap();
    assert_eq!(
        apply_one(src, ed.remove_element(arr, 1).unwrap()),
        r#"{"a":1,"b":2,"c":[true,null]}"#
    );
    assert_eq!(
        apply_one(src, ed.remove_element(arr, 2).unwrap()),
        r#"{"a":1,"b":2,"c":[true,false]}"#
    );
}

#[test]
fn remove_from_inline_array_in_pretty_document() {
    let src = "{\n  \"a\": [1, 2, 3]\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let a = root.get("a").unwrap();
    assert_eq!(
        apply_one(src, ed.remove_element(a, 0).unwrap()),
        "{\n  \"a\": [2, 3]\n}"
    );
    assert_eq!(
        apply_one(src, ed.remove_element(a, 1).unwrap()),
        "{\n  \"a\": [1, 3]\n}"
    );
    assert_eq!(
        apply_one(src, ed.remove_element(a, 2).unwrap()),
        "{\n  \"a\": [1, 2]\n}"
    );
}

#[test]
fn remove_with_mixed_layout() {
    // Second member shares the line with the first: only its own text goes.
    let src = "{\n  \"a\": 1, \"b\": 2,\n  \"c\": 3\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 0).unwrap()),
        "{\n  \"b\": 2,\n  \"c\": 3\n}"
    );
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 1).unwrap()),
        "{\n  \"a\": 1, \"c\": 3\n}"
    );
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 2).unwrap()),
        "{\n  \"a\": 1, \"b\": 2\n}"
    );
}

#[test]
fn remove_with_crlf_line_endings() {
    let src = "{\r\n  \"a\": 1,\r\n  \"b\": 2,\r\n  \"c\": 3\r\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 1).unwrap()),
        "{\r\n  \"a\": 1,\r\n  \"c\": 3\r\n}"
    );
    assert_eq!(
        apply_one(src, ed.remove_member(&root, 2).unwrap()),
        "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}"
    );
}

#[test]
fn remove_errors() {
    let src = r#"{"a": [1]}"#;
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    assert_eq!(
        ed.remove_member(&root, 1),
        Err(EditError::IndexOutOfBounds { index: 1, len: 1 })
    );
    assert_eq!(ed.remove_element(&root, 0), Err(EditError::NotAnArray));
    assert_eq!(
        ed.remove_member(root.get("a").unwrap(), 0),
        Err(EditError::NotAnObject)
    );
    assert_eq!(
        ed.remove_member_by_key(&root, "zz"),
        Err(EditError::KeyNotFound("zz".to_owned()))
    );
    assert_eq!(
        ed.remove_members(&root, &[0, 5]).unwrap_err().to_string(),
        "index 5 out of bounds for length 1"
    );
}

#[test]
fn remove_duplicate_key_occurrence() {
    let src = "{\n  \"a\": 1,\n  \"a\": 2\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let dups = root.as_object().unwrap().duplicate_keys();
    assert_eq!(dups, vec![vec![0, 1]]);
    // Removing the first, shadowed occurrence keeps the effective value.
    let out = apply_one(src, ed.remove_member(&root, 0).unwrap());
    assert_eq!(out, "{\n  \"a\": 2\n}");
}

#[test]
fn replace_values() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let width = root.pointer("/props/defaultSize/width").unwrap();
    let out = apply_one(VIEW, ed.replace_value(width, "640"));
    assert_eq!(out, VIEW.replace("\"width\": 500", "\"width\": 640"));

    let text = root.pointer("/root/children/0/props/text").unwrap();
    let out = apply_one(VIEW, ed.replace_with_string(text, "Size = 'big' & <b>"));
    assert_eq!(
        out,
        VIEW.replace(
            "\"text\": \"Hello\"",
            "\"text\": \"Size \\u003d \\u0027big\\u0027 \\u0026 \\u003cb\\u003e\""
        )
    );

    // A multi-line replacement is indented to the value's line.
    let props = root.pointer("/root/props").unwrap();
    let out = apply_one(
        VIEW,
        ed.replace_value(
            props,
            "{\n  \"direction\": \"row\",\n  \"wrap\": \"wrap\"\n}",
        ),
    );
    assert_eq!(
        out,
        VIEW.replace(
            "    \"props\": {\n      \"direction\": \"column\"\n    },",
            "    \"props\": {\n      \"direction\": \"row\",\n      \"wrap\": \"wrap\"\n    },"
        )
    );
}

#[test]
fn rename_keys() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let custom = root.get("custom").unwrap().as_object().unwrap();
    let member = custom.member("counter").unwrap();
    let out = apply_one(VIEW, ed.rename_key(member, "clickCount"));
    assert_eq!(out, VIEW.replace("\"counter\": 0", "\"clickCount\": 0"));
    let out = apply_one(VIEW, ed.rename_key(member, "a=\"b\""));
    assert_eq!(
        out,
        VIEW.replace("\"counter\": 0", "\"a\\u003d\\\"b\\\"\": 0")
    );
    let minimal = ed.clone().with_escape_style(EscapeStyle::Minimal);
    let out = apply_one(VIEW, minimal.rename_key(member, "a=b"));
    assert_eq!(out, VIEW.replace("\"counter\": 0", "\"a=b\": 0"));
}

#[test]
fn insert_member_sorted_like_ignition() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let custom = root.get("custom").unwrap();
    // "label" sorts between "counter" and "unusedProp".
    let out = apply_one(
        VIEW,
        ed.insert_member(custom, InsertPosition::Sorted, "label", "\"x\"")
            .unwrap(),
    );
    assert_eq!(
        out,
        VIEW.replace(
            "    \"counter\": 0,\n    \"unusedProp\"",
            "    \"counter\": 0,\n    \"label\": \"x\",\n    \"unusedProp\""
        )
    );
    // "aaa" sorts first.
    let out = apply_one(
        VIEW,
        ed.insert_member(custom, InsertPosition::Sorted, "aaa", "1")
            .unwrap(),
    );
    assert_eq!(
        out,
        VIEW.replace("  \"custom\": {\n", "  \"custom\": {\n    \"aaa\": 1,\n")
    );
    // "zzz" sorts last.
    let out = apply_one(
        VIEW,
        ed.insert_member(custom, InsertPosition::Sorted, "zzz", "null")
            .unwrap(),
    );
    assert_eq!(
        out,
        VIEW.replace(
            "    \"unusedProp\": \"x\"\n",
            "    \"unusedProp\": \"x\",\n    \"zzz\": null\n"
        )
    );
    // Upper case sorts before lower case (UTF-16 order).
    let out = apply_one(
        VIEW,
        ed.insert_member(custom, InsertPosition::Sorted, "Zeta", "2")
            .unwrap(),
    );
    assert_eq!(
        out,
        VIEW.replace("  \"custom\": {\n", "  \"custom\": {\n    \"Zeta\": 2,\n")
    );
}

#[test]
fn insert_member_with_multiline_value() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let child = root.pointer("/root/children/1").unwrap();
    let edit = ed
        .insert_member(
            child,
            InsertPosition::Sorted,
            "props",
            "{\n  \"direction\": \"column\"\n}",
        )
        .unwrap();
    let out = apply_one(VIEW, edit);
    assert_eq!(
        out,
        VIEW.replace(
            "          \"name\": \"Spacer\"\n        },\n        \"type\"",
            "          \"name\": \"Spacer\"\n        },\n        \"props\": {\n          \"direction\": \"column\"\n        },\n        \"type\""
        )
    );
}

#[test]
fn insert_member_into_empty_object() {
    let src = "{\n  \"custom\": {},\n  \"params\": {}\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let custom = root.get("custom").unwrap();
    let out = apply_one(
        src,
        ed.insert_member(custom, InsertPosition::End, "a", "1")
            .unwrap(),
    );
    assert_eq!(
        out,
        "{\n  \"custom\": {\n    \"a\": 1\n  },\n  \"params\": {}\n}"
    );
    let out = apply_one(
        src,
        ed.insert_member(custom, InsertPosition::Sorted, "o", "{\n  \"k\": true\n}")
            .unwrap(),
    );
    assert_eq!(
        out,
        "{\n  \"custom\": {\n    \"o\": {\n      \"k\": true\n    }\n  },\n  \"params\": {}\n}"
    );
    let root_src = "{}";
    let root2 = parse(root_src).unwrap();
    let out = apply_one(
        root_src,
        editor(root_src, &root2)
            .insert_member(&root2, InsertPosition::End, "a", "1")
            .unwrap(),
    );
    assert_eq!(out, "{\"a\": 1}");
}

#[test]
fn insert_member_positions() {
    let src = "{\n  \"b\": 2,\n  \"a\": 1\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    // Unsorted object: Sorted appends.
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::Sorted, "c", "3")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"b\": 2,\n  \"a\": 1,\n  \"c\": 3\n}");
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::Index(0), "c", "3")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"c\": 3,\n  \"b\": 2,\n  \"a\": 1\n}");
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::Index(1), "c", "3")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"b\": 2,\n  \"c\": 3,\n  \"a\": 1\n}");
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::Index(2), "c", "3")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"b\": 2,\n  \"a\": 1,\n  \"c\": 3\n}");
    assert_eq!(
        ed.insert_member(&root, InsertPosition::Index(3), "c", "3"),
        Err(EditError::IndexOutOfBounds { index: 3, len: 2 })
    );
    assert_eq!(
        ed.insert_member(&root, InsertPosition::End, "a", "3"),
        Err(EditError::KeyExists("a".to_owned()))
    );
}

#[test]
fn insert_member_inline_and_compact() {
    let src = r#"{"a": 1, "c": 3}"#;
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::Sorted, "b", "2")
            .unwrap(),
    );
    assert_eq!(out, r#"{"a": 1, "b": 2, "c": 3}"#);
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::End, "d", "4")
            .unwrap(),
    );
    assert_eq!(out, r#"{"a": 1, "c": 3, "d": 4}"#);
    let compact = r#"{"a":1,"c":3}"#;
    let root = parse(compact).unwrap();
    let ed = editor(compact, &root);
    let out = apply_one(
        compact,
        ed.insert_member(&root, InsertPosition::Sorted, "b", "2")
            .unwrap(),
    );
    assert_eq!(out, r#"{"a":1,"b":2,"c":3}"#);
    let out = apply_one(
        compact,
        ed.insert_member(&root, InsertPosition::End, "d", "4")
            .unwrap(),
    );
    assert_eq!(out, r#"{"a":1,"c":3,"d":4}"#);
}

#[test]
fn insert_elements() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let children = root.pointer("/root/children").unwrap();
    let new_child =
        "{\n  \"meta\": {\n    \"name\": \"Footer\"\n  },\n  \"type\": \"ia.display.label\"\n}";
    let out = apply_one(VIEW, ed.insert_element(children, 2, new_child).unwrap());
    let indented = "{\n        \"meta\": {\n          \"name\": \"Footer\"\n        },\n        \"type\": \"ia.display.label\"\n      }";
    let after_second = children.index(1).unwrap().range.end().to_usize();
    let expected = format!(
        "{},\n      {}{}",
        VIEW.get(..after_second).unwrap(),
        indented,
        VIEW.get(after_second..).unwrap()
    );
    assert_eq!(out, expected);

    let out = apply_one(VIEW, ed.insert_element(children, 0, new_child).unwrap());
    let first = children.index(0).unwrap().range.start().to_usize();
    let expected = format!(
        "{}{},\n      {}",
        VIEW.get(..first).unwrap(),
        indented,
        VIEW.get(first..).unwrap()
    );
    assert_eq!(out, expected);

    let src = "{\n  \"list\": [],\n  \"inline\": [1, 3]\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let out = apply_one(
        src,
        ed.insert_element(root.get("list").unwrap(), 0, "\"x\"")
            .unwrap(),
    );
    assert_eq!(
        out,
        "{\n  \"list\": [\n    \"x\"\n  ],\n  \"inline\": [1, 3]\n}"
    );
    let out = apply_one(
        src,
        ed.insert_element(root.get("inline").unwrap(), 1, "2")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"list\": [],\n  \"inline\": [1, 2, 3]\n}");
    let out = apply_one(
        src,
        ed.insert_element(root.get("inline").unwrap(), 2, "4")
            .unwrap(),
    );
    assert_eq!(out, "{\n  \"list\": [],\n  \"inline\": [1, 3, 4]\n}");
    assert_eq!(
        ed.insert_element(root.get("inline").unwrap(), 3, "4"),
        Err(EditError::IndexOutOfBounds { index: 3, len: 2 })
    );
    assert_eq!(ed.insert_element(&root, 0, "4"), Err(EditError::NotAnArray));
}

#[test]
fn insert_with_crlf() {
    let src = "{\r\n  \"a\": 1\r\n}";
    let root = parse(src).unwrap();
    let ed = editor(src, &root);
    let out = apply_one(
        src,
        ed.insert_member(&root, InsertPosition::End, "b", "{\n  \"c\": 2\n}")
            .unwrap(),
    );
    assert_eq!(
        out,
        "{\r\n  \"a\": 1,\r\n  \"b\": {\r\n    \"c\": 2\r\n  }\r\n}"
    );
}

#[test]
fn replace_inside_embedded_expression() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    let expr = root
        .pointer("/propConfig/custom.counter/binding/config/expression")
        .unwrap()
        .as_json_string()
        .unwrap();
    let decoded = expr.value();
    assert_eq!(
        decoded,
        "if({view.params.lineName} = \"\", 0, len({view.params.lineName}))"
    );
    // Replace `= ""` with `!= ""`: the raw range covers the escapes.
    let start = decoded.find("= \"\"").unwrap();
    let range = TextRange::at(TextSize::from(start), TextSize::of("= \"\""));
    assert_eq!(expr.raw_range(range).slice(VIEW), "\\u003d \\\"\\\"");
    let out = apply_one(VIEW, ed.replace_in_string(expr, range, "!= ''"));
    let new_root = parse(&out).unwrap();
    assert_eq!(
        new_root
            .pointer("/propConfig/custom.counter/binding/config/expression")
            .unwrap()
            .as_str(),
        Some("if({view.params.lineName} != '', 0, len({view.params.lineName}))")
    );
    assert!(out.contains("!\\u003d \\u0027\\u0027"));
    // Only the expression line changed.
    let changed: Vec<_> = VIEW
        .lines()
        .zip(out.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1);
}

#[test]
fn reindent_helper() {
    assert_eq!(reindent("1", "  "), "1");
    assert_eq!(
        reindent("{\n  \"a\": 1\n}", "    "),
        "{\n      \"a\": 1\n    }"
    );
    assert_eq!(reindent("[\n\n]", "  "), "[\n\n  ]");
    assert_eq!(reindent("{\n}", ""), "{\n}");
}

#[test]
fn quote_uses_document_style() {
    let root = parse(VIEW).unwrap();
    let ed = editor(VIEW, &root);
    assert_eq!(ed.quote("a=b"), "\"a\\u003db\"");
    let hand = "{\"x\": \"a = b\"}";
    let root = parse(hand).unwrap();
    assert_eq!(editor(hand, &root).quote("c=d"), "\"c=d\"");
}

#[test]
fn indent_unit_detection() {
    let four = "{\n    \"a\": {\n        \"b\": 1\n    }\n}";
    let root = parse(four).unwrap();
    let ed = editor(four, &root);
    assert_eq!(ed.indent_unit(), "    ");
    let a = root.get("a").unwrap();
    let out = apply_one(
        four,
        ed.insert_member(a, InsertPosition::End, "c", "{\n    \"d\": 2\n}")
            .unwrap(),
    );
    assert_eq!(
        out,
        "{\n    \"a\": {\n        \"b\": 1,\n        \"c\": {\n            \"d\": 2\n        }\n    }\n}"
    );
    let tabs = "{\n\t\"a\": {}\n}";
    let root = parse(tabs).unwrap();
    let ed = editor(tabs, &root);
    assert_eq!(ed.indent_unit(), "\t");
    let out = apply_one(
        tabs,
        ed.insert_member(root.get("a").unwrap(), InsertPosition::End, "b", "1")
            .unwrap(),
    );
    assert_eq!(out, "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}");
}
