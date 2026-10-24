//! The same body gives the same findings in every script location: the
//! five Perspective wrappers (inside JSON strings with escapes) and a
//! library function.

use crate::rules::script::testing::*;

const BODY: &str = "\ttry:\n\t\tresult \u{3d} undefinedHelper(self)\n\texcept:\n\t\tpass\n\tif self is 'on':\n\t\tself.text \u{3d} \"<b>\" + str(1)\n\t\treturn\n\t\tself.text \u{3d} ''\n\td \u{3d} {'a': 1, 'a': 2}\n\tassert (d, 'msg')";

#[test]
fn findings_agree_across_locations() {
    assert_eq!(flagged_everywhere("SCR101", BODY), ["undefinedHelper"]);
    assert_eq!(flagged_everywhere("SCR103", BODY), ["result"]);
    assert_eq!(flagged_everywhere("SCR105", BODY), ["self.text = ''"]);
    assert_eq!(flagged_everywhere("SCR106", BODY), ["except"]);
    assert_eq!(flagged_everywhere("SCR108", BODY), ["'a'"]);
    assert_eq!(flagged_everywhere("SCR110", BODY), ["self is 'on'"]);
    assert_eq!(flagged_everywhere("SCR111", BODY), ["(d, 'msg')"]);
}

#[test]
fn header_parameters_differ_per_location() {
    // `payload` is only a parameter of message handlers.
    let code = "\tself.x \u{3d} payload";
    assert!(view(ViewScript::Handler, code).lint(&["SCR101"]).is_empty());
    for kind in [
        ViewScript::Action,
        ViewScript::Transform,
        ViewScript::Change,
        ViewScript::Method,
    ] {
        assert_eq!(
            decoded_flagged(&view(kind, code).lint(&["SCR101"])),
            ["payload"],
            "{kind:?}"
        );
    }
}

#[test]
fn transform_findings_point_into_the_json_string() {
    let f = view(
        ViewScript::Transform,
        "\tx \u{3d} '<a \u{26} b>'\n\treturn undefinedThing",
    );
    let d = f.lint(&["SCR101"]);
    assert_eq!(d.len(), 1);
    // The raw file text at the range is the escaped-free name, after the
    // escaped characters of the first line.
    assert_eq!(d[0].range.slice(d[0].file.text()), "undefinedThing");
    let text = d[0].file.text();
    assert!(
        text.get(..d[0].range.start().to_usize())
            .unwrap()
            .ends_with("\\n\\treturn ")
    );
}
