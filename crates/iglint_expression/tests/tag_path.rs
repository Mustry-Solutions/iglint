//! Tag path parsing. Expectations match Ignition 8.3.3's `TagPathParser`.

use iglint_expression::{TagPath, TagPathErrorKind, parse_tag_path};
use iglint_text::TextSize;

fn tp(path: &str) -> TagPath {
    parse_tag_path(path, TextSize::ZERO)
}

#[track_caller]
fn ok(path: &str, source: Option<&str>, comps: &[&str], property: Option<&str>) {
    let t = tp(path);
    assert!(t.is_valid(), "{path:?}: {:?}", t.error);
    assert_eq!(t.source_name(), source, "{path:?}");
    let c: Vec<&str> = t.components.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(c, comps, "{path:?}");
    assert_eq!(t.property_name(), property, "{path:?}");
}

#[track_caller]
fn err(path: &str, kind: TagPathErrorKind) {
    let t = tp(path);
    let e = t.error.unwrap_or_else(|| panic!("{path:?} should fail"));
    assert_eq!(e.kind, kind, "{path:?}");
}

#[test]
fn valid_paths() {
    ok(
        "[default]Folder/Tag",
        Some("default"),
        &["Folder", "Tag"],
        None,
    );
    ok(
        "[default]Folder/Tag.Quality",
        Some("default"),
        &["Folder", "Tag"],
        Some("Quality"),
    );
    ok(
        "[default]Folder/Tag.value",
        Some("default"),
        &["Folder", "Tag"],
        Some("value"),
    );
    ok(
        "[default]Tag.EngUnit",
        Some("default"),
        &["Tag"],
        Some("EngUnit"),
    );
    ok("[~]Relative/Tag", Some("~"), &["Relative", "Tag"], None);
    ok("[.]Sibling", Some("."), &["Sibling"], None);
    ok("[..]Up/Tag", Some(".."), &["Up", "Tag"], None);
    ok("[.]../Up/Tag", Some("."), &["..", "Up", "Tag"], None);
    ok("[.]../../Tag", Some("."), &["..", "..", "Tag"], None);
    ok(
        "[System]Gateway/CurrentDateTime",
        Some("System"),
        &["Gateway", "CurrentDateTime"],
        None,
    );
    ok("[]Folder/Tag", Some(""), &["Folder", "Tag"], None);
    ok("Folder/Tag", None, &["Folder", "Tag"], None);
    ok("Tag", None, &["Tag"], None);
    ok("Tag.x", None, &["Tag"], Some("x"));
    ok("value", None, &["value"], None);
    ok("1", None, &["1"], None);
    ok("[default]", Some("default"), &[], None);
    ok("[default]Folder/", Some("default"), &["Folder"], None);
    ok(
        "[default]Folder//Tag",
        Some("default"),
        &["Folder", "Tag"],
        None,
    );
    ok("[default]/Tag", Some("default"), &["Tag"], None);
    ok("/Tag", None, &["Tag"], None);
    ok(
        "[default] Folder/Tag",
        Some("default"),
        &[" Folder", "Tag"],
        None,
    );
    ok(
        "[default]Folder/Tag ",
        Some("default"),
        &["Folder", "Tag "],
        None,
    );
    ok(
        "[default]Folder Name/Tag Name",
        Some("default"),
        &["Folder Name", "Tag Name"],
        None,
    );
    ok(
        "[default]Folder/Tag[0]",
        Some("default"),
        &["Folder", "Tag[0]"],
        None,
    );
    ok("[default]Tag[0].x", Some("default"), &["Tag[0]"], Some("x"));
    ok(
        "[default]Folder/Tag[1,2]",
        Some("default"),
        &["Folder", "Tag[1,2]"],
        None,
    );
    ok(
        "[default]Folder/Tag[\"a b\"]",
        Some("default"),
        &["Folder", "Tag[\"a b\"]"],
        None,
    );
    ok(
        "[default]Folder/Tag['x']",
        Some("default"),
        &["Folder", "Tag['x']"],
        None,
    );
    ok("[de fault]X", Some("de fault"), &["X"], None);
    ok("[provider/sub]X", Some("provider/sub"), &["X"], None);
    ok("[de.fa~ult/x]Tag", Some("de.fa~ult/x"), &["Tag"], None);
    ok(
        "[default]Folder\\Tag",
        Some("default"),
        &["Folder", "Tag"],
        None,
    );
    ok("..\\Tag", None, &["..", "Tag"], None);
    ok("../Tag", None, &["..", "Tag"], None);
    ok(
        "[default]Folder/../Tag",
        Some("default"),
        &["Folder", "..", "Tag"],
        None,
    );
    ok(
        "[default]Folder/Tag/Alarms/High.ActivePriority",
        Some("default"),
        &["Folder", "Tag", "Alarms", "High"],
        Some("ActivePriority"),
    );
    ok(
        "[default]Fol\tder/Tag",
        Some("default"),
        &["Fol", "der", "Tag"],
        None,
    );
    ok(
        "[default]Fol\nder/Tag",
        Some("default"),
        &["Fol", "der", "Tag"],
        None,
    );
    ok("[default]Ta'g", Some("default"), &["Ta'g"], None);
    ok("[default]Ta:g", Some("default"), &["Ta:g"], None);
    ok("[default]Ta(g)", Some("default"), &["Ta(g)"], None);
    ok("[default]Ta-g", Some("default"), &["Ta-g"], None);
    ok("[default]Ta*g", Some("default"), &["Ta*g"], None);
    ok("[default]Ta$g", Some("default"), &["Ta$g"], None);
    ok("[default]Ta é g", Some("default"), &["Ta é g"], None);
    ok(
        "[default]Folder/Tag::x",
        Some("default"),
        &["Folder", "Tag::x"],
        None,
    );
    ok(
        "[MQTT Engine]Edge Nodes/G/N/D/Tag",
        Some("MQTT Engine"),
        &["Edge Nodes", "G", "N", "D", "Tag"],
        None,
    );
    ok("[default]Tag.", Some("default"), &["Tag"], None);
    ok("[default].Tag", Some("default"), &[], Some("Tag"));
    ok("[default]Tag/.", Some("default"), &["Tag"], None);
    ok("[default]./Tag", Some("default"), &["Tag"], None);
    ok("[default].", Some("default"), &[], None);
    ok("[default]..", Some("default"), &[], None);
    ok("[default]...", Some("default"), &[], None);
    ok("[default]Tag..", Some("default"), &["Tag"], None);
    ok("[default]Tag/./x", Some("default"), &["Tag", "x"], None);
    ok("[0]", Some("0"), &[], None);
    ok("\n[a b]x", Some("a b"), &["x"], None);
}

#[test]
fn invalid_paths() {
    use TagPathErrorKind::*;
    err("[default]Fo.lder/Tag", AfterProperty);
    err("[default]Folder/Tag.Alarms/High.Name", AfterProperty);
    err("[default]Folder/Tag.a.b", AfterProperty);
    err("[default]Folder/Tag.value[0]", AfterProperty);
    err("this.props.text", AfterProperty);
    err("[default]Tag..x", PropertyWithoutTag);
    err("[default]Folder/Tag/.x", PropertyWithoutTag);
    err(".x", PropertyWithoutTag);
    err("[default][0]", InvalidArrayIndex);
    err("[default]Folder/[0]", InvalidArrayIndex);
    err("[default]Tag[0][1]", InvalidArrayIndex);
    err("[default]Tag.[0]", InvalidArrayIndex);
    err("['k']x['k']", InvalidArrayIndex);
    err("[default]Folder/Tag[1, 2]", IllegalCharacter('['));
    err("[default]Folder/Tag[]", IllegalCharacter('['));
    err(" [default]Folder/Tag", IllegalCharacter('['));
    err("[default", IllegalCharacter('['));
    err("[[default]]X", IllegalCharacter('['));
    err("[a][b]x", IllegalCharacter('['));
    err("x[default]", IllegalCharacter('['));
    err("[d\\e]Tag", IllegalCharacter('['));
    err("default]X", IllegalCharacter(']'));
    err("[default]X]", IllegalCharacter(']'));
    for c in [
        ',', '"', '#', '@', '%', '&', '+', '{', '}', ';', '!', '~', '?', '=', '<', '>', '|', '^',
        '`',
    ] {
        err(&format!("[default]Ta{c}g"), IllegalCharacter(c));
    }
}

#[test]
fn ranges_and_helpers() {
    let src = "[default]Folder/Tag.Quality";
    let t = parse_tag_path(src, TextSize::new(1));
    let shifted =
        |r: iglint_text::TextRange| &src[r.start().to_usize() - 1..r.end().to_usize() - 1];
    assert_eq!(shifted(t.source.as_ref().unwrap().range), "default");
    assert_eq!(shifted(t.components[1].range), "Tag");
    assert_eq!(shifted(t.property.as_ref().unwrap().range), "Quality");
    assert_eq!(t.path_string(), "Folder/Tag");
    assert!(!t.is_relative());
    assert!(tp("[.]x").is_relative());
    assert!(tp("[~]x").is_relative());
    assert!(tp("../x").is_relative());
    let e = tp("[default]Ta#g").error.unwrap();
    assert_eq!(e.range.slice("[default]Ta#g"), "#");
    assert_eq!(e.kind.to_string(), "illegal character '#' in tag path");
}
