//! VIEW017 message-handler-never-receives.

use iglint_project::view::View;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW017",
    name: "message-handler-never-receives",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Message handler listens to no scope",
    explanation: r#"## What it does

Reports a component message handler whose `pageScope`, `sessionScope` and
`viewScope` are all `false`. Handlers where a flag is missing are not
reported.

## Why is this bad?

`system.perspective.sendMessage` delivers a message to the handlers that
listen to the scope it is sent in. A handler that listens to no scope never
receives anything, so its script is dead code; usually a scope checkbox was
unticked by mistake.

## Example

```json
{"messageType": "refresh", "pageScope": false, "sessionScope": false, "viewScope": false, "script": "\tself.refreshBinding('props.data')"}
```

Use instead:

```json
{"messageType": "refresh", "pageScope": true, "sessionScope": false, "viewScope": false, "script": "\tself.refreshBinding('props.data')"}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&HandlerNeverReceives)],
    options: None,
};

struct HandlerNeverReceives;

impl ViewRule for HandlerNeverReceives {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        for handler in &view.message_handlers {
            // Only explicit `false` values: the default of a missing flag is
            // not something to guess.
            let all_off = ["pageScope", "sessionScope", "viewScope"]
                .iter()
                .all(|k| handler.value.get(k).and_then(|v| v.as_bool()) == Some(false));
            if !all_off {
                continue;
            }
            let (range, name) = match handler.message_type {
                Some(t) => (t.range(), t.value()),
                None => (handler.value.range, ""),
            };
            cx.report(
                range,
                format!(
                    "Message handler `{name}` listens to no scope (page, session and view are all off); it never receives messages"
                ),
            )
            .node_path(handler.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn reports_handlers_without_scope() {
        let f = Fixture::view(
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"},
              "scripts": {"messageHandlers": [
                {"messageType": "dead", "pageScope": false, "sessionScope": false, "viewScope": false, "script": "\tpass"},
                {"messageType": "missing-flags", "script": "\tpass"},
                {"messageType": "live", "pageScope": false, "sessionScope": true, "viewScope": false, "script": "\tpass"}]}}}"#,
        );
        let d = f.lint(&["VIEW017"]);
        assert_eq!(flagged(&d), vec!["\"dead\""]);
    }
}
