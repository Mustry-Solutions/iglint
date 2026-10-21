//! STYLE003 empty-style-class.

use iglint_project::model::ModelData;

use super::css_mentions;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "STYLE003",
    name: "empty-style-class",
    category: Category::Style,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "Style class defines no styles",
    explanation: r#"## What it does

Reports style classes whose base style and every variant (pseudo-class
and media-query variants) define no style properties and no animation.
Classes that the project stylesheet or a gateway theme uses in a selector
(`.psc-<name>`) are marker classes and are not reported.

## Why is this bad?

An empty style class does nothing: components that use it look exactly
the same without it. It is usually a class whose styles were moved
elsewhere, or a placeholder that was never filled in.

## Example

```json
{"base": {"style": {}}, "variants": []}
```

Give the class styles, or delete it and remove it from the components
that use it.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&EmptyStyleClass)],
    options: None,
};

struct EmptyStyleClass;

impl ResourceRule for EmptyStyleClass {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::StyleClass(s) = data else {
            return;
        };
        if !s.is_empty() || !s.problems.is_empty() {
            return;
        }
        let Some(resource) = cx.resource() else {
            return;
        };
        if let Some((index, project)) = cx.project_index()
            && css_mentions(index, project, &format!("psc-{}", resource.path))
        {
            return;
        }
        let range = s.base_style.or(s.base).map_or(s.root.range, |v| v.range);
        cx.report_in(
            s.file,
            range,
            format!("Style class `{}` defines no styles", resource.path),
        )
        .node_path(resource.path.to_string());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn class(f: Fixture, name: &str, json: &str) -> Fixture {
        f.resource(
            &format!("projects/p/com.inductiveautomation.perspective/style-classes/{name}"),
            &[("style.json", json)],
        )
    }

    #[test]
    fn empty_and_non_empty_classes() {
        let f = class(
            Fixture::new(),
            "Empty",
            r#"{"base": {"style": {}}, "variants": []}"#,
        );
        let f = class(f, "Full", r#"{"base": {"style": {"color": "red"}}}"#);
        let f = class(
            f,
            "Hover",
            r#"{"base": {"style": {}}, "variants": [{"pseudo": "hover", "style": {"color": "red"}}]}"#,
        );
        let d = f.lint(&["STYLE003"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec!["Style class `Empty` defines no styles".to_owned()]
        );
    }

    #[test]
    fn marker_classes_used_by_the_stylesheet_are_fine() {
        let f = class(Fixture::new(), "Marker", r#"{"base": {"style": {}}}"#).resource(
            "projects/p/com.inductiveautomation.perspective/stylesheet",
            &[("stylesheet.css", ".psc-Marker .x { color: red }")],
        );
        assert!(f.lint(&["STYLE003"]).is_empty());
    }
}
