//! STYLE001 unknown-style-class.

use iglint_project::kinds::ResourceKind;
use iglint_project::model::ModelData;
use iglint_project::view::StyleClassRef;
use iglint_project::{Existence, FileId};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};
use crate::rules::refs::case_insensitive_match;

pub static META: RuleMeta = RuleMeta {
    code: "STYLE001",
    name: "unknown-style-class",
    category: Category::Style,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Style class does not exist",
    explanation: r#"## What it does

Checks every name in a literal `classes` list (`props.style.classes`,
`elementStyle.classes`, ... of views, components and session properties)
and in the outputs of `style-list` map transforms. A name resolves when
the project (with inheritance) has a style class resource of that path, or
when the project stylesheet or a gateway theme in the workspace defines
the CSS class Perspective renders for it (`psc-<name>`, so a stylesheet
rule `.psc-alarm-blink` makes `alarm-blink` valid).

## Why is this bad?

Perspective silently ignores an unknown class, so the component is not
styled the way the author intended. Unknown classes are usually style
classes that were renamed, moved or never imported. Class names are
case-sensitive: `PID/Error` does not use the style class `pid/Error`.

Bound class lists (the literal is only the Designer's cached value) are
not checked. When no gateway theme is in the workspace, a name without a
style class resource may still be defined by a theme, so it is only
reported with `strict = true`.

## Example

```json
{"props": {"style": {"classes": "Brand/Primry"}}}
```

Use instead:

```json
{"props": {"style": {"classes": "Brand/Primary"}}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnknownStyleClass)],
    options: None,
};

struct UnknownStyleClass;

impl ResourceRule for UnknownStyleClass {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let (file, refs) = match data {
            ModelData::View { file, view } => (*file, &view.style_refs),
            ModelData::SessionProps { file, props } => (*file, &props.style_refs),
            _ => return,
        };
        for r in refs {
            if !r.bound {
                check(cx, file, r);
            }
        }
    }
}

fn check(cx: &mut Checker<'_, '_>, file: FileId, r: &StyleClassRef<'_>) {
    let Some((index, project)) = cx.project_index() else {
        return;
    };
    for (name, range) in &r.classes {
        let existence = index.style_class_reference(project, name);
        let report = match existence {
            Existence::Missing => true,
            Existence::Unknown => cx.strict(),
            Existence::Exists => false,
        };
        if !report || name.contains('{') {
            continue;
        }
        let mut message = format!("Style class `{name}` does not exist");
        if let Some(similar) =
            case_insensitive_match(cx.workspace(), project, ResourceKind::StyleClass, name)
        {
            message.push_str(&format!(
                " (class names are case-sensitive; did you mean `{similar}`?)"
            ));
        } else if existence == Existence::Unknown {
            message.push_str(" (no style class resource; gateway themes are not in the workspace)");
        }
        cx.report_in(file, *range, message).node_path(&r.node_path);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn classes(list: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
                {{"type": "ia.display.label", "meta": {{"name": "L"}}, "props": {{"style": {{"classes": "{list}"}}}}}}
            ]}}}}"#
        )
    }

    fn style(f: Fixture, path: &str) -> Fixture {
        f.resource(
            &format!("projects/p/com.inductiveautomation.perspective/style-classes/{path}"),
            &[("style.json", r#"{"base": {"style": {"color": "red"}}}"#)],
        )
    }

    /// A gateway with one theme, so names without a resource are decidable.
    fn with_theme(f: Fixture, css: &str) -> Fixture {
        f.file("services/gw/config-mode.json", r#"{"title": "core"}"#)
            .resource(
                "services/gw/com.inductiveautomation.perspective/themes/light",
                &[("index.css", css)],
            )
    }

    #[test]
    fn resources_stylesheet_and_theme_classes_resolve() {
        let f = style(
            Fixture::view(&classes("Brand/Primary blink themed gone")),
            "Brand/Primary",
        )
        .resource(
            "projects/p/com.inductiveautomation.perspective/stylesheet",
            &[("stylesheet.css", ".psc-blink { opacity: 0 }")],
        );
        let f = with_theme(f, ".psc-themed { color: blue }");
        let d = f.lint(&["STYLE001"]);
        assert_eq!(flagged(&d), vec!["gone"]);
        assert_eq!(
            d[0].node_path.as_deref(),
            Some("root/L.props.style.classes")
        );
    }

    #[test]
    fn case_mismatch_hint() {
        let f = with_theme(style(Fixture::view(&classes("pid/error")), "PID/Error"), "");
        let d = f.lint(&["STYLE001"]);
        assert!(
            d[0].message.contains("did you mean `PID/Error`"),
            "{}",
            d[0].message
        );
    }

    #[test]
    fn without_themes_only_strict_reports() {
        let f = Fixture::view(&classes("gone"));
        assert!(f.lint(&["STYLE001"]).is_empty());
    }

    #[test]
    fn bound_class_lists_are_skipped() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.label", "meta": {"name": "L"}, "props": {"style": {"classes": "gone"}},
             "propConfig": {"props.style.classes": {"binding": {"type": "expr", "config": {"expression": "'x'"}}}}}
        ]}}"#;
        assert!(
            with_theme(Fixture::view(view), "")
                .lint(&["STYLE001"])
                .is_empty()
        );
    }

    #[test]
    fn map_transform_outputs() {
        let view = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.label", "meta": {"name": "L"}, "props": {"style": {"classes": ""}},
             "propConfig": {"props.style.classes": {"binding": {"type": "expr", "config": {"expression": "1"},
                "transforms": [{"type": "map", "inputType": "scalar", "outputType": "style-list",
                    "mappings": [{"input": 1, "output": "gone"}], "fallback": ""}]}}}}
        ]}}"#;
        let d = with_theme(Fixture::view(view), "").lint(&["STYLE001"]);
        assert_eq!(flagged(&d), vec!["gone"]);
    }
}
