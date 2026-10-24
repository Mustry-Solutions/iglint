//! Reference classification. The accept/reject decisions match Perspective's
//! `PropertyRef` / `LegacyPropertyRef` (checked against the 8.3.3 module).

use iglint_expression::{
    PropertyRef, PropertyTarget, Reference, ReferenceContext, SegmentKind, classify,
    is_property_ref, parse, parse_property_ref,
};
use iglint_text::{TextRange, TextSize};

fn prop(path: &str) -> PropertyRef {
    parse_property_ref(path, TextSize::ZERO)
        .unwrap_or_else(|| panic!("{path:?} should be a property reference"))
}

fn names(p: &PropertyRef) -> Vec<String> {
    p.segments
        .iter()
        .map(|s| match &s.kind {
            SegmentKind::Name(n) => n.clone(),
            SegmentKind::Key(k) => format!("'{k}'"),
            SegmentKind::Index(Some(i)) => format!("[{i}]"),
            SegmentKind::Index(None) => "[]".to_owned(),
        })
        .collect()
}

fn component_names(t: &PropertyTarget) -> Vec<String> {
    match t {
        PropertyTarget::Relative { components, .. } | PropertyTarget::Absolute { components } => {
            components
                .iter()
                .map(|c| c.name().unwrap().to_owned())
                .collect()
        }
        _ => Vec::new(),
    }
}

#[test]
fn accepted_paths() {
    let modern = [
        "this.props.text",
        "this.props",
        "this.custom.a.b",
        "this.meta.name",
        "this.position.x",
        "view.params.x",
        "view.custom.x",
        "view.props.x",
        "view",
        "this",
        "session.props.auth.user.userName",
        "session.custom.x",
        "page.props.path",
        "page.custom.x",
        "parent.custom.x",
        "parent.props.x",
        "this.props.items[0].name",
        "this.props.items[0]",
        "this.props.items[\"a\"]",
        "this.props.items['a']",
        "this.props.items[\"a']",
        "this.props.items[0][1]",
        "this.props.items[]",
        "this.props.a-b",
        "this.x.y",
        "this.PROPS.text",
        "this.propsx.y",
        "this.customx",
        "this[0].props",
        "view.params.list[0].a[1].b",
        "this.props.Text_1",
        "this.props.text.value",
        "../Sibling.props.text",
        "../../Up.custom.x",
        ".../Container/Child.custom.x",
        "..../X.props.y",
        "./Child.props.text",
        "./A/B.props.x",
        "../Sib/Child.props.x",
        "../Sibling.props.items[0].name",
        "../My-Comp.props.x",
        "../X.props",
        "../X.props.",
        "../X.props..a",
        "../X.props.[0]",
        "../X.props[0].a",
        "../X.position.x",
        "../X.meta.name",
        "../X.params.p",
        "/root/Comp.props.x",
        "/root.props.x",
        "/Comp.props.x",
        "/root/a/b/c.custom.d",
    ];
    for path in modern {
        assert!(is_property_ref(path), "{path:?}");
        assert!(!prop(path).legacy, "{path:?} is modern");
    }
    let legacy_only = [
        "this.props.a b",
        "this.props.items['a b']",
        "../My Comp.props.x",
        "this.props.a.b c",
        "/root/My Panel/Label 1.props.text",
        "this.props.text ",
    ];
    for path in legacy_only {
        assert!(is_property_ref(path), "{path:?}");
        assert!(prop(path).legacy, "{path:?} is legacy");
    }
}

#[test]
fn rejected_paths() {
    let rejected = [
        "",
        "value",
        "value.x",
        "x",
        "Folder/Tag",
        "[default]Folder/Tag",
        "THIS.props.text",
        "This.props.text",
        "this .props.text",
        " this.props.text",
        "this.props.1a",
        "this.props.a.1",
        "this.props.",
        "this.props..x",
        "this.custom.$x",
        "this.custom.é",
        "thisx.props",
        "this.props[x]",
        "this.props['a'b']",
        "root/Comp.props.x",
        ".//X.props.y",
        "../X",
        "../X.custom2.a",
        "../X.propsx",
        "../.props.x",
        "/props.x",
        "/root/",
        "this.props.a-b c",
        "{this.props.text}",
        "this.props.a\tb",
    ];
    for path in rejected {
        assert!(!is_property_ref(path), "{path:?} should be rejected");
    }
}

#[test]
fn shortcut_targets_and_segments() {
    let cases = [
        (
            "this.props.text",
            PropertyTarget::This,
            vec!["props", "text"],
        ),
        ("view.params.x", PropertyTarget::View, vec!["params", "x"]),
        (
            "parent.custom.a.b",
            PropertyTarget::Parent,
            vec!["custom", "a", "b"],
        ),
        (
            "session.props.auth.user.userName",
            PropertyTarget::Session,
            vec!["props", "auth", "user", "userName"],
        ),
        (
            "page.props.path",
            PropertyTarget::Page,
            vec!["props", "path"],
        ),
        (
            "this.props.items[0].name",
            PropertyTarget::This,
            vec!["props", "items", "[0]", "name"],
        ),
        (
            "this.props.items['k'][]",
            PropertyTarget::This,
            vec!["props", "items", "'k'", "[]"],
        ),
        ("this[2].props", PropertyTarget::This, vec!["[2]", "props"]),
        ("view", PropertyTarget::View, vec![]),
    ];
    for (path, target, segs) in cases {
        let p = prop(path);
        assert_eq!(p.target, target, "{path}");
        assert_eq!(names(&p), segs, "{path}");
    }
    assert_eq!(prop("this.props.text").category(), Some("props"));
    assert_eq!(prop("view.params.x").property_path(), "params.x");
    assert_eq!(
        prop("this.props.items[0]['a b']").property_path(),
        "props.items[0].a b"
    );
    assert_eq!(PropertyTarget::Session.scope_name(), Some("session"));
}

#[test]
fn relative_targets() {
    let cases = [
        (
            "./Child.props.text",
            0,
            vec!["Child"],
            vec!["props", "text"],
        ),
        (
            "../Sibling.props.text",
            1,
            vec!["Sibling"],
            vec!["props", "text"],
        ),
        (
            ".../Container/Child.custom.x",
            2,
            vec!["Container", "Child"],
            vec!["custom", "x"],
        ),
        ("../../Up.custom.x", 2, vec!["Up"], vec!["custom", "x"]),
        (
            "../../../A/B.meta.name",
            3,
            vec!["A", "B"],
            vec!["meta", "name"],
        ),
        ("..../X.props.y", 3, vec!["X"], vec!["props", "y"]),
        ("../X.props..a", 1, vec!["X"], vec!["props", "a"]),
        ("../X.props.[0]", 1, vec!["X"], vec!["props", "[0]"]),
        ("../X.props", 1, vec!["X"], vec!["props"]),
    ];
    for (path, up, comps, segs) in cases {
        let p = prop(path);
        let PropertyTarget::Relative { up: u, .. } = &p.target else {
            panic!("{path} not relative")
        };
        assert_eq!(*u, up, "{path}");
        assert_eq!(component_names(&p.target), comps, "{path}");
        assert_eq!(names(&p), segs, "{path}");
    }
}

#[test]
fn absolute_targets() {
    let p = prop("/root/Panel/Label.props.text");
    assert!(matches!(p.target, PropertyTarget::Absolute { .. }));
    assert_eq!(component_names(&p.target), ["root", "Panel", "Label"]);
    assert_eq!(names(&p), ["props", "text"]);
    let p = prop("/Label.custom.x");
    assert_eq!(component_names(&p.target), ["Label"]);
}

#[test]
fn segment_ranges_point_into_the_expression() {
    let src = "1 + {../Panel/Label.props.items[3].text}";
    let parsed = parse(src);
    let r = parsed.expr.references()[0];
    let Reference::Property(p) = r.resolve(ReferenceContext::PerspectiveBinding) else {
        panic!()
    };
    assert_eq!(p.range.slice(src), "../Panel/Label.props.items[3].text");
    let slices: Vec<_> = p.segments.iter().map(|s| s.range.slice(src)).collect();
    assert_eq!(slices, ["props", "items", "[3]", "text"]);
    let PropertyTarget::Relative {
        prefix_range,
        components,
        ..
    } = &p.target
    else {
        panic!()
    };
    assert_eq!(prefix_range.slice(src), "../");
    assert_eq!(components[1].range.slice(src), "Label");
}

#[test]
fn classification_by_context() {
    let at = TextSize::ZERO;
    let b = ReferenceContext::PerspectiveBinding;
    let t = ReferenceContext::PerspectiveTransform;
    let g = ReferenceContext::Gateway;
    assert!(matches!(
        classify("this.props.text", at, b),
        Reference::Property(_)
    ));
    assert!(matches!(classify("[default]A/B", at, b), Reference::Tag(_)));
    // Not a property reference: Perspective subscribes to a tag instead.
    let Reference::Tag(tag) = classify("thiss.props.text", at, b) else {
        panic!("typo becomes a tag")
    };
    assert!(!tag.is_valid());
    // Any names after a scope keyword are accepted by the grammar.
    let Reference::Property(p) = classify("this.prop.text", at, b) else {
        panic!()
    };
    assert_eq!(p.category(), Some("prop"));
    let Reference::Tag(tag) = classify("Folder/Tag", at, b) else {
        panic!()
    };
    assert_eq!(tag.source_name(), None);
    assert_eq!(tag.path_string(), "Folder/Tag");
    let Reference::Tag(tag) = classify("value", at, b) else {
        panic!()
    };
    assert_eq!(tag.path_string(), "value");
    assert_eq!(classify("value", at, t), Reference::TransformValue);
    assert!(matches!(classify("Value", at, t), Reference::Tag(_)));
    assert!(matches!(classify(" value", at, t), Reference::Tag(_)));
    let Reference::Tag(tag) = classify("value.x", at, t) else {
        panic!()
    };
    assert_eq!(tag.property_name(), Some("x"));
    assert!(matches!(
        classify("this.props.text", at, t),
        Reference::Property(_)
    ));
    assert!(matches!(classify("[.]Sibling", at, g), Reference::Tag(_)));
    assert_eq!(
        classify("ParamName", at, g),
        Reference::Other("ParamName".to_owned())
    );
    // Leading spaces make it a tag path with a space in the component.
    let Reference::Tag(tag) = classify(" this.props.text", at, b) else {
        panic!()
    };
    assert!(!tag.is_valid());
}

#[test]
fn ranges_are_offset() {
    let p = parse_property_ref("view.params.x", TextSize::new(10)).unwrap();
    assert_eq!(
        p.range,
        TextRange::new(TextSize::new(10), TextSize::new(23))
    );
    assert_eq!(
        p.segments[1].range,
        TextRange::new(TextSize::new(22), TextSize::new(23))
    );
}

/// `{ view.custom.x }`: Ignition's lexer keeps the spaces and neither
/// `PropertyRef` nor `LegacyPropertyRef` accepts a leading space (checked
/// with `oracle/expression/ignition_oracle.py` on 8.3.3: `" view.custom.vx "`
/// is rejected by both, `"view.custom.vx "` only by `PropertyRef`), so
/// Perspective reads it as a tag path.
#[test]
fn spaces_inside_the_braces_make_a_tag_path() {
    use iglint_expression::{
        Reference, ReferenceContext, is_property_ref, misspaced_property_ref, parse,
    };
    assert!(!is_property_ref(" view.custom.vx "));
    assert!(!is_property_ref(" view.custom.vx"));
    assert!(!is_property_ref("\tview.custom.vx"));
    assert!(is_property_ref("view.custom.vx "));
    let parsed = parse("{ view.custom.vx }");
    let refs = parsed.expr.references();
    assert_eq!(refs[0].path, " view.custom.vx ");
    assert!(matches!(
        refs[0].resolve(ReferenceContext::PerspectiveBinding),
        Reference::Tag(_)
    ));
    let p = misspaced_property_ref(&refs[0].path, refs[0].path_range.start()).unwrap();
    assert_eq!(p.range.start().to_u32(), 2);
    assert!(misspaced_property_ref("view.custom.vx", iglint_text::TextSize::new(0)).is_none());
    assert!(misspaced_property_ref(" [default]A/B ", iglint_text::TextSize::new(0)).is_none());
    assert!(misspaced_property_ref(" Folder/Tag ", iglint_text::TextSize::new(0)).is_none());
    assert!(
        misspaced_property_ref(" ../Label.props.text", iglint_text::TextSize::new(0)).is_some()
    );
}
