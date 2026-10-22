//! SCR306 message-handler-payload-misuse.

use iglint_project::kinds::EventScriptKind;
use iglint_project::summary::covered_by_prefix;
use iglint_project::{ModelData, ScriptKind};
use iglint_python::ast::{CmpOp, Expr, Slice};
use iglint_python_semantic::{BindingKind, SemanticModel};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR306",
    name: "message-handler-payload-misuse",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Message handler reads a payload key that no sender sends",
    explanation: r#"## What it does

Only with `strict`. For component message handlers and gateway and
session message handler scripts, compares the payload keys the handler
reads with `payload['key']` with the keys of every literal payload sent to
it (`system.perspective.sendMessage('type', {'key': ...})` for component
handlers, `system.util.sendMessage(project, 'handler', {...})` for the
others), and reports keys that no sender passes.

It only reports when every send of the type to the project is known and
passes a dict literal: a payload that is a variable, a send with a computed
type, and a handler with no literal sender at all make the rule silent.
Reads with a default (`payload.get('key')`) and keys the handler tests
first (`'key' in payload`) are not reported.

## Why is this bad?

`payload['key']` raises `KeyError` when the sender does not pass the key,
so the handler fails on every message. This is usually a key that was
renamed on one side only.

## Example

```python
# sender
system.perspective.sendMessage('selectLine', {'lineId': 3})
# handler
self.view.custom.line = payload['line']
```

Use instead:

```python
self.view.custom.line = payload['lineId']
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&PayloadMisuse)],
    options: None,
};

struct PayloadMisuse;

/// The message type the script handles and whether it is a component
/// handler (`system.perspective.sendMessage`).
fn handled_type(cx: &Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) -> Option<(String, bool)> {
    match (cx.data(), unit.script.kind) {
        (ModelData::View { view, .. }, ScriptKind::MessageHandler) => view
            .message_handlers
            .iter()
            .find(|h| h.script.is_some_and(|s| s.index() == unit.index))
            .and_then(|h| h.message_type)
            .map(|t| (t.value().to_owned(), true)),
        (
            ModelData::EventScript(e),
            ScriptKind::EventFile(
                EventScriptKind::GatewayMessage | EventScriptKind::SessionMessage,
            ),
        ) if e.script.is_some() => cx.resource().map(|r| (r.path.to_string(), false)),
        _ => None,
    }
}

fn is_payload(model: &SemanticModel<'_>, e: &Expr) -> bool {
    let Expr::Name(n) = e else { return false };
    &*n.id == "payload"
        && model.reference_at(n.range).is_some_and(|r| {
            !r.bindings.is_empty()
                && r.bindings
                    .iter()
                    .all(|b| model.binding(*b).kind == BindingKind::Parameter)
        })
}

fn literal(e: &Expr) -> Option<&str> {
    match e {
        Expr::Str(s) => s.s.as_str(),
        _ => None,
    }
}

impl ScriptRule for PayloadMisuse {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !cx.strict() {
            return;
        }
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        let Some((message_type, perspective)) = handled_type(cx, unit) else {
            return;
        };
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let dynamic = index.dynamic_references(project);
        let name = &cx.workspace().project(project).name;
        let dynamic_send = if perspective {
            covered_by_prefix(&dynamic.perspective_messages, &message_type)
        } else {
            dynamic.gateway_messages.iter().any(|(target, prefix)| {
                target.as_ref().is_none_or(|t| t == name)
                    && message_type.starts_with(prefix.as_str())
            })
        };
        let sends: Vec<_> = index
            .message_sends(project, &message_type)
            .iter()
            .filter(|s| s.perspective == perspective)
            .collect();
        if dynamic_send || sends.is_empty() || sends.iter().any(|s| s.payload_keys.is_none()) {
            return;
        }
        let sent: Vec<&str> = sends
            .iter()
            .flat_map(|s| s.payload_keys.iter().flatten().map(String::as_str))
            .collect();
        let mut reads: Vec<(&str, TextRange)> = Vec::new();
        let mut tested: Vec<&str> = Vec::new();
        walk_exprs(&model.module.body, &mut |e| match e {
            Expr::Subscript(s) if is_payload(model, &s.value) => {
                if let Slice::Index(i) = &*s.slice
                    && let Some(k) = literal(&i.value)
                {
                    reads.push((k, iglint_text::Ranged::range(&*i.value)));
                }
            }
            Expr::Compare(c)
                if c.ops.iter().any(|o| matches!(o, CmpOp::In | CmpOp::NotIn))
                    && c.comparators.iter().any(|x| is_payload(model, x)) =>
            {
                tested.extend(literal(&c.left));
            }
            Expr::Call(c) => {
                if let Expr::Attribute(a) = &*c.func
                    && is_payload(model, &a.value)
                    && matches!(a.attr.as_str(), "has_key" | "get" | "pop" | "setdefault")
                    && let Some(k) = c.args.first().and_then(literal)
                {
                    tested.push(k);
                }
            }
            _ => {}
        });
        for (key, range) in reads {
            if sent.contains(&key) || tested.contains(&key) {
                continue;
            }
            let message = format!(
                "No sender of `{message_type}` passes payload key `{key}` ({} send{} checked)",
                sends.len(),
                if sends.len() == 1 { "" } else { "s" }
            );
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{ViewScript, decoded_flagged, view_json};
    use crate::rules::test_support::Fixture;

    fn project(sender: &str, handler: &str) -> Fixture {
        Fixture::new()
            .with_view("Main", &view_json(ViewScript::Action, sender))
            .with_view("Other", &view_json(ViewScript::Handler, handler))
    }

    fn strict(f: &Fixture) -> Vec<crate::diagnostic::Diagnostic> {
        let mut settings = crate::rules::test_support::settings(f.root(), &[("SCR306", "")]);
        std::sync::Arc::make_mut(&mut settings.base).strict = true;
        crate::engine::check(
            &settings,
            &[f.root().to_path_buf()],
            &crate::engine::CheckOptions::default(),
        )
        .diagnostics
    }

    #[test]
    fn unknown_keys_with_strict() {
        let f = project(
            "\tsystem.perspective.sendMessage('m', {'lineId': 1, 'x': 2})\n\tsystem.perspective.sendMessage('m', dict(lineId=2))",
            "\ta = payload['lineId']\n\tb = payload['line']\n\tc = payload.get('opt')\n\tif 'maybe' in payload:\n\t\td = payload['maybe']",
        );
        assert_eq!(decoded_flagged(&strict(&f)), vec!["'line'"]);
        // Not without strict.
        assert!(f.lint(&["SCR306"]).is_empty());
    }

    #[test]
    fn unknown_senders_make_it_silent() {
        let f = project(
            "\tsystem.perspective.sendMessage('m', {'lineId': 1})\n\tsystem.perspective.sendMessage('m', data)",
            "\tb = payload['line']",
        );
        assert!(strict(&f).is_empty());
        let f = project("\tpass", "\tb = payload['line']");
        assert!(strict(&f).is_empty());
    }
}
