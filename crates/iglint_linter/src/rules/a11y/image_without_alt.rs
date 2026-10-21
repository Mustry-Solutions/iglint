//! A11Y001 image-without-alt.

use iglint_project::Owner;
use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};
use crate::rules::helpers::is_blank_text;

pub static META: RuleMeta = RuleMeta {
    code: "A11Y001",
    name: "image-without-alt",
    category: Category::A11y,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Image component without alternative text",
    explanation: r#"## What it does

Reports an Image component (`ia.display.image`) whose `props.altText` is
missing or blank and not bound.

## Why is this bad?

`altText` becomes the image's `alt` attribute. Screen readers announce an
image without it by its file name, and nothing is shown when the source
fails to load (a missing image or a broken URL on a remote screen leaves an
empty box).

## Example

```json
{"type": "ia.display.image", "meta": {"name": "Logo"}, "props": {"source": "/system/images/logo.png"}}
```

Use instead:

```json
{"type": "ia.display.image", "meta": {"name": "Logo"}, "props": {"source": "/system/images/logo.png", "altText": "Company logo"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&ImageWithoutAlt)],
    options: None,
};

struct ImageWithoutAlt;

impl ComponentRule for ImageWithoutAlt {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        if component.type_id != Some("ia.display.image") {
            return;
        }
        let alt = component.props.and_then(|p| p.get("altText"));
        if !is_blank_text(alt) || view.is_bound(Owner::Component(component.id), "props.altText") {
            return;
        }
        let range = alt.map_or(component.name_range(), |v| v.range);
        cx.report(
            range,
            format!(
                "Image `{}` has no alternative text (`props.altText`)",
                component.name
            ),
        )
        .node_path(component.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn images_need_alt_text() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
              {"type": "ia.display.image", "meta": {"name": "NoAlt"}, "props": {"source": "x.png"}},
              {"type": "ia.display.image", "meta": {"name": "Blank"}, "props": {"altText": " "}},
              {"type": "ia.display.image", "meta": {"name": "Ok"}, "props": {"altText": "Logo"}},
              {"type": "ia.display.image", "meta": {"name": "Bound"},
               "propConfig": {"props.altText": {"binding": {"type": "expr", "config": {"expression": "'x'"}}}}},
              {"type": "ia.display.label", "meta": {"name": "Label"}}]}}"#,
        );
        let d = f.lint(&["A11Y001"]);
        assert_eq!(flagged(&d), vec!["\"NoAlt\"", "\" \""]);
    }
}
