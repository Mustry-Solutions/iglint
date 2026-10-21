//! VIEW010 unknown-action-type.

use iglint_project::view::{ACTION_TYPES, ActionKind, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW010",
    name: "unknown-action-type",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Event action `type` is not one Ignition knows",
    explanation: r#"## What it does

Reports an event action (view or component `events`) whose `type` is not
an action type of Perspective in Ignition 8.3 (`script`, `nav`, `popup`,
`dock`, `alter-dock`, `login`, `logout`, `auth-challenge`, `theme`,
`refresh`, `fullscreen`, `request-print`, `console-log`, `alter-logging`,
`debug`, `workstation/mode` and the `native/*` mobile actions), or that has
no `type` at all.

## Why is this bad?

Perspective cannot run an action it does not know: the event does nothing.
This happens after hand edits or when a view is copied from a newer
Ignition version.

## Example

```json
"onClick": {"type": "navigate", "scope": "C", "config": {"page": "/home"}}
```

Use instead:

```json
"onClick": {"type": "nav", "scope": "C", "config": {"page": "/home"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&UnknownActionType)],
    options: None,
};

struct UnknownActionType;

impl ViewRule for UnknownActionType {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        for handler in &view.events {
            for action in &handler.actions {
                if !matches!(action.kind, ActionKind::Unknown) {
                    continue;
                }
                let (range, message) = match action.type_value {
                    Some(t) if !ACTION_TYPES.contains(&t.value()) => {
                        (t.range(), format!("Unknown action type `{}`", t.value()))
                    }
                    Some(_) => continue,
                    None => (action.value.range, "Event action has no `type`".to_owned()),
                };
                cx.report(range, message)
                    .node_path(action.node_path.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_unknown_and_missing_types() {
        let f = Fixture::view(
            r#"{"events": {"system": {"onStartup": {"type": "navigate", "config": {}}}},
               "root": {"type": "ia.container.flex", "meta": {"name": "root"},
                 "events": {"dom": {"onClick": [{"type": "nav", "config": {"page": "/"}}, {"config": {}},
                                                {"type": "refresh"}, {"type": "native/barcode"}]}}}}"#,
        );
        let d = f.lint(&["VIEW010"]);
        assert_eq!(
            messages(&d).into_iter().map(|m| m.1).collect::<Vec<_>>(),
            vec![
                "Unknown action type `navigate`",
                "Event action has no `type`"
            ]
        );
        assert_eq!(flagged(&d)[0], "\"navigate\"");
    }
}
