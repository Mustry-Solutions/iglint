//! REF014 unhandled-message-handler.

use iglint_project::kinds::EventScriptKind;
use iglint_project::model::ModelData;
use iglint_project::summary::covered_by_prefix;
use iglint_project::{ProjectId, ProjectIndex};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF014",
    name: "unhandled-message-handler",
    category: Category::Ref,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Message handler whose type nothing sends",
    explanation: r#"## What it does

Reports message handlers whose type no script sends:

* component message handlers (views): no `system.perspective.sendMessage`
  with that literal type in the project;
* gateway, Perspective session and Vision client message handlers: no
  `system.util.sendMessage` / `sendRequest` / `sendRequestAsync` with that
  literal handler name to the project (a literal project name,
  `system.util.getProjectName()`, or a project that is not a literal,
  which may be any project). Gateway resources (tag event scripts) send to
  every project.

The rule stays silent for a handler when a send with a computed type could
produce its name (`'refresh' + suffix` covers `refreshTable`), in
inheritable projects (projects outside the workspace may send to them),
and in projects that have resources whose scripts are not analysed
(Vision windows and templates, reports, alarm pipelines, SFC charts, event
streams).

## Why is this bad?

A handler nothing sends to is dead code, or a sign that the sender was
renamed. This rule is not in the default set: messages are also sent from
other gateways (`remoteServers`), from Vision clients and from scripts
that build the type from data.

## Example

A view whose button has a message handler `refreshTrend` while every
script sends `refreshTrends`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnhandledMessageHandler)],
    options: None,
};

struct UnhandledMessageHandler;

/// Whether a handler of `message_type` in `project` may receive a message.
fn is_sent(
    index: &ProjectIndex<'_>,
    project: ProjectId,
    message_type: &str,
    perspective: bool,
) -> bool {
    if index
        .message_sends(project, message_type)
        .iter()
        .any(|s| s.perspective == perspective)
    {
        return true;
    }
    let dynamic = index.dynamic_references(project);
    if perspective {
        covered_by_prefix(&dynamic.perspective_messages, message_type)
    } else {
        let name = &index.workspace().project(project).name;
        dynamic.gateway_messages.iter().any(|(target, prefix)| {
            target.as_ref().is_none_or(|t| t == name) && message_type.starts_with(prefix.as_str())
        })
    }
}

impl ResourceRule for UnhandledMessageHandler {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
            return;
        };
        let ws = cx.workspace();
        if ws.project(project).inheritable()
            || !ws.project(project).inheritance.complete
            || super::has_unmodelled_scripts(ws, project)
        {
            return;
        }
        match data {
            ModelData::View { file, view } => {
                for h in &view.message_handlers {
                    let Some(t) = h.message_type else {
                        continue;
                    };
                    if t.value().is_empty() || is_sent(index, project, t.value(), true) {
                        continue;
                    }
                    cx.report_in(
                        *file,
                        t.range(),
                        format!("No script sends message type `{}`", t.value()),
                    )
                    .node_path(h.node_path.clone());
                }
            }
            ModelData::EventScript(e)
                if matches!(
                    e.kind,
                    EventScriptKind::GatewayMessage
                        | EventScriptKind::SessionMessage
                        | EventScriptKind::ClientMessage
                ) =>
            {
                let name = resource.path.to_string();
                if is_sent(index, project, &name, false) {
                    return;
                }
                let file = e.script.as_ref().map_or(resource.manifest.file, |s| s.file);
                let range = cx
                    .file(file)
                    .map_or(TextRange::default(), |f| TextRange::first_char(f.text()));
                cx.report_in(
                    file,
                    range,
                    format!("No script sends to message handler `{name}`"),
                )
                .node_path(name);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{ViewScript, messages, view_json};
    use crate::rules::test_support::Fixture;

    fn project(sender: &str) -> Fixture {
        Fixture::new()
            .with_view("Main", &view_json(ViewScript::Action, sender))
            .with_view("Other", &view_json(ViewScript::Handler, "\tpass"))
            .resource(
                "projects/p/ignition/message/gw",
                &[("handleMessage.py", "def handleMessage(payload):\n\tpass")],
            )
    }

    #[test]
    fn unsent_handlers() {
        let d = project("\tpass").lint(&["REF014"]);
        assert_eq!(
            messages(&d),
            vec![
                "No script sends message type `m`",
                "No script sends to message handler `gw`"
            ]
        );
    }

    #[test]
    fn literal_and_dynamic_sends() {
        let sends = "\tsystem.perspective.sendMessage('m')\n\tsystem.util.sendMessage(system.util.getProjectName(), 'gw')";
        assert!(project(sends).lint(&["REF014"]).is_empty());
        let dynamic =
            "\tsystem.perspective.sendMessage(name)\n\tsystem.util.sendMessage(proj, 'g' + x)";
        assert!(project(dynamic).lint(&["REF014"]).is_empty());
        // A gateway send does not reach component handlers.
        let wrong = "\tsystem.util.sendMessage('p', 'm')\n\tsystem.perspective.sendMessage('gw')";
        assert_eq!(project(wrong).lint(&["REF014"]).len(), 2);
    }
}
