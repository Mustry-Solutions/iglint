//! NAME002 method-shadows-component-api.

use iglint_project::view::View;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "NAME002",
    name: "method-shadows-component-api",
    category: Category::Name,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Custom method has the name of a built-in component member",
    explanation: r#"## What it does

Reports component custom methods named like a member of Perspective's
component scripting object: `getChild`, `getChildren`, `getSibling`,
`refreshBinding`, `focus`, `requestPrint`, `requestTooltip`,
`removeTooltip`, `requestContextMenu`, `removeContextMenu`, and the
attributes `props`, `custom`, `meta`, `position`, `children`, `parent`,
`view`, `session` and `page`. The list is the component descriptor shipped
with Perspective 8.3 plus the documented navigation attributes.

## Why is this bad?

Custom methods are called as `self.<name>(...)` on the same object that
provides these members. Two things with one name cannot both be reached:
either the custom method is never called, or every script that relies on
the built-in member (`self.getChild('X')`, `self.props.text`) breaks for
this component.

## Example

```json
"customMethods": [
  {"name": "refreshBinding", "params": ["path"], "script": "\tpass"}
]
```

Use instead:

```json
"customMethods": [
  {"name": "reloadData", "params": ["path"], "script": "\tpass"}
]
```

## ign-lint

No equivalent.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&MethodShadowsApi)],
    options: None,
};

/// Members of the component scripting object (`descriptors/component.json`
/// of perspective-common 8.3, plus `parent`, `view`, `session`, `page`).
const MEMBERS: &[&str] = &[
    "children",
    "custom",
    "focus",
    "getChild",
    "getChildren",
    "getSibling",
    "meta",
    "page",
    "parent",
    "position",
    "props",
    "refreshBinding",
    "removeContextMenu",
    "removeTooltip",
    "requestContextMenu",
    "requestPrint",
    "requestTooltip",
    "session",
    "view",
];

struct MethodShadowsApi;

impl ViewRule for MethodShadowsApi {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        for m in &view.custom_methods {
            let Some(name) = m.name else { continue };
            if MEMBERS.binary_search(&name.value()).is_ok() {
                cx.report(
                    name.range(),
                    format!(
                        "Custom method `{}` has the name of a built-in component member",
                        name.value()
                    ),
                )
                .node_path(m.node_path.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(methods: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}},
  "scripts": {{"customMethods": [{methods}], "messageHandlers": [], "extensionFunctions": null}}}}}}"#
        ))
    }

    #[test]
    fn reports_built_in_names() {
        let f = view(
            r#"{"name": "refreshBinding", "params": [], "script": "\tpass"},
               {"name": "props", "params": [], "script": "\tpass"},
               {"name": "reload", "params": [], "script": "\tpass"},
               {"name": "getchild", "params": [], "script": "\tpass"}"#,
        );
        let d = f.lint(&["NAME002"]);
        assert_eq!(flagged(&d), vec!["\"refreshBinding\"", "\"props\""]);
        assert_eq!(
            d[0].node_path.as_deref(),
            Some("root@customMethods.refreshBinding")
        );
    }

    #[test]
    fn members_are_sorted() {
        let mut sorted = super::MEMBERS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, super::MEMBERS);
    }
}
