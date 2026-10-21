//! VIEW002 duplicate-component-name.

use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{ComponentRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW002",
    name: "duplicate-component-name",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Sibling components share a name",
    explanation: r#"## What it does

Reports a component whose `meta.name` equals the name of an earlier
sibling (a component with the same parent).

## Why is this bad?

Perspective addresses components by name: `getChild('Label')`,
`getSibling('Label')`, `{../Label.props.text}` and `/root/Panel/Label` all
resolve to the *first* sibling with that name, so the second component can
never be reached and bindings silently read the wrong one. The Designer
normally prevents duplicates; they appear after hand edits, merges or
copy-paste between views.

## Example

```json
"children": [
  {"type": "ia.display.label", "meta": {"name": "Label"}},
  {"type": "ia.display.label", "meta": {"name": "Label"}}
]
```

Use instead:

```json
"children": [
  {"type": "ia.display.label", "meta": {"name": "Title"}},
  {"type": "ia.display.label", "meta": {"name": "Subtitle"}}
]
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&DuplicateComponentName)],
    options: None,
};

struct DuplicateComponentName;

impl ComponentRule for DuplicateComponentName {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        view: &View<'_>,
        component: &Component<'_>,
    ) {
        let Some(parent) = component.parent else {
            return;
        };
        if component.name.is_empty() || component.name_value.is_none() {
            return;
        }
        let first = view
            .children(parent)
            .find(|c| c.name == component.name && c.name_value.is_some());
        let Some(first) = first else {
            return;
        };
        if first.id == component.id {
            return;
        }
        let file = cx.primary_file();
        cx.report(
            component.name_range(),
            format!(
                "Component name `{}` is already used by a sibling; references by name reach only the first one",
                component.name
            ),
        )
        .node_path(format!("{}[{}]", view.component(parent).path, component.index_in_parent))
        .label(file, first.name_range(), "first component with this name");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(children: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [{children}]}}}}"#
        )
    }

    #[test]
    fn reports_second_and_later_duplicates() {
        let f = Fixture::view(&view(
            r#"{"type": "ia.display.label", "meta": {"name": "A"}},
               {"type": "ia.display.label", "meta": {"name": "B"}},
               {"type": "ia.display.label", "meta": {"name": "A"}},
               {"type": "ia.display.label", "meta": {"name": "A"}}"#,
        ));
        let d = f.lint(&["VIEW002"]);
        assert_eq!(flagged(&d), vec!["\"A\"", "\"A\""]);
        assert_eq!(d[0].labels.len(), 1);
        assert_eq!(d[0].node_path.as_deref(), Some("root[2]"));
    }

    #[test]
    fn same_name_in_different_parents_is_fine() {
        let f = Fixture::view(&view(
            r#"{"type": "ia.container.flex", "meta": {"name": "P1"},
                "children": [{"type": "ia.display.label", "meta": {"name": "A"}}]},
               {"type": "ia.container.flex", "meta": {"name": "P2"},
                "children": [{"type": "ia.display.label", "meta": {"name": "A"}}]}"#,
        ));
        assert!(f.lint(&["VIEW002"]).is_empty());
    }

    #[test]
    fn names_are_case_sensitive() {
        let f = Fixture::view(&view(
            r#"{"type": "ia.display.label", "meta": {"name": "a"}},
               {"type": "ia.display.label", "meta": {"name": "A"}}"#,
        ));
        assert!(f.lint(&["VIEW002"]).is_empty());
    }
}
