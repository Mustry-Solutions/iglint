//! REF006 unknown-message-type.

use iglint_project::{HandlerSite, ProjectId, ProjectIndex};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::{string_value, string_values};

pub static META: RuleMeta = RuleMeta {
    code: "REF006",
    name: "unknown-message-type",
    category: Category::Ref,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Message sent to a type no handler listens for",
    explanation: r#"## What it does

Checks the literal message types of script calls against the message
handlers of the target project (with inheritance):

* `system.perspective.sendMessage(type, payload, scope)`: component
  message handlers of the project's views. When handlers of the type
  exist but none listens on the call's scope (`page` when not given;
  `session`, `page` or `view` as a literal), that is reported too.
* `system.util.sendMessage(project, handler, payload, scope)`: gateway,
  Perspective session and Vision client message handlers of `project`
  (a literal naming a project of the workspace, or
  `system.util.getProjectName()`); a literal `scope` (`G`, `S`, `C` and
  combinations) limits the handler kinds that count.
* `system.util.sendRequest` / `sendRequestAsync`: gateway message
  handlers of `project`.

Types and projects that are not literals, projects outside the workspace
and projects whose handlers may come from a parent outside the workspace
are not checked. In an inheritable project a handler defined by any
project inheriting from it counts.

## Why is this bad?

A message nobody handles is silently dropped (`sendRequest` fails with
an error). A missing handler is usually a typo in the type or a handler
that was renamed.

## Example

```python
system.perspective.sendMessage('refreshTabel', scope='session')
```

Use instead (the handler listens for `refreshTable` with session scope):

```python
system.perspective.sendMessage('refreshTable', scope='session')
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnknownMessageType)],
    options: None,
};

struct UnknownMessageType;

/// The projects whose handlers a send to `project` may reach: the project
/// and, when it is inheritable, every project inheriting from it. `None`
/// when one of them inherits from a project outside the workspace.
fn reachable(index: &ProjectIndex<'_>, project: ProjectId) -> Option<Vec<ProjectId>> {
    let ws = index.workspace();
    let mut out = vec![project];
    if ws.project(project).inheritable() {
        out.extend(ws.project_descendants(project));
    }
    out.iter()
        .all(|p| ws.project(*p).inheritance.complete)
        .then_some(out)
}

fn handlers<'i>(
    index: &'i ProjectIndex<'_>,
    projects: &[ProjectId],
    message_type: &str,
) -> Vec<&'i HandlerSite> {
    projects
        .iter()
        .flat_map(|p| index.message_handlers(*p, message_type))
        .collect()
}

impl ScriptRule for UnknownMessageType {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some((model, targets)) = super::script_calls::api_targets(cx, unit) else {
            return;
        };
        let Some((index, own)) = cx.project_index() else {
            return;
        };
        for (_, target) in targets {
            match target {
                ApiTarget::PerspectiveMessage {
                    message_type,
                    scope,
                    ..
                } => {
                    let Some(projects) = reachable(index, own) else {
                        continue;
                    };
                    let scope = match scope.given() {
                        None if scope.is_absent() => Some("page".to_owned()),
                        None => None,
                        Some(e) => string_value(model, e).map(|l| l.value),
                    };
                    for literal in string_values(model, message_type).unwrap_or_default() {
                        let t = literal.value.as_str();
                        let sites: Vec<_> = handlers(index, &projects, t)
                            .into_iter()
                            .filter_map(|h| match h {
                                HandlerSite::Component {
                                    page_scope,
                                    session_scope,
                                    view_scope,
                                    ..
                                } => Some((*page_scope, *session_scope, *view_scope)),
                                _ => None,
                            })
                            .collect();
                        let message = if sites.is_empty() {
                            format!("No component message handler listens for `{t}`")
                        } else {
                            let listens = |(page, session, view): (bool, bool, bool)| match scope
                                .as_deref()
                            {
                                Some("page") => page,
                                Some("session") => session,
                                Some("view") => view,
                                _ => true,
                            };
                            if sites.iter().any(|s| listens(*s)) {
                                continue;
                            }
                            format!(
                                "Message handlers of `{t}` do not listen on scope `{}`",
                                scope.as_deref().unwrap_or("")
                            )
                        };
                        crate::rules::script::report(cx, unit, literal.range, message);
                    }
                }
                ApiTarget::GatewayMessage {
                    project,
                    handler,
                    scope,
                    request,
                    ..
                } => {
                    let Some(target) = super::script_calls::project_argument(cx, model, project)
                    else {
                        continue;
                    };
                    let Some(projects) = reachable(index, target) else {
                        continue;
                    };
                    let kinds = if request {
                        Some("G".to_owned())
                    } else {
                        match scope.given() {
                            None if scope.is_absent() => None,
                            None => continue,
                            Some(e) => match string_value(model, e) {
                                Some(l) => Some(l.value.to_ascii_uppercase()),
                                None => continue,
                            },
                        }
                    };
                    let accepts = |h: &&HandlerSite| {
                        let letter = match h {
                            HandlerSite::Gateway(_) => 'G',
                            HandlerSite::Session(_) => 'S',
                            HandlerSite::Client(_) => 'C',
                            HandlerSite::Component { .. } => return false,
                        };
                        kinds.as_deref().is_none_or(|k| k.contains(letter))
                    };
                    let project_name = &cx.workspace().project(target).name;
                    for literal in string_values(model, handler).unwrap_or_default() {
                        let h = literal.value.as_str();
                        let all = handlers(index, &projects, h);
                        if all.iter().any(accepts) {
                            continue;
                        }
                        let message = match (request, kinds.as_deref()) {
                            (true, _) => format!(
                                "Project `{project_name}` has no gateway message handler `{h}`"
                            ),
                            (false, Some(k)) => format!(
                                "Project `{project_name}` has no message handler `{h}` for scope `{k}`"
                            ),
                            (false, None) => {
                                format!("Project `{project_name}` has no message handler `{h}`")
                            }
                        };
                        crate::rules::script::report(cx, unit, literal.range, message);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{ViewScript, decoded_flagged, messages, view_json};
    use crate::rules::test_support::Fixture;

    fn with_handlers(f: Fixture) -> Fixture {
        f.with_view("Other", &view_json(ViewScript::Handler, "\tpass"))
            .resource(
                "projects/p/ignition/message/gw",
                &[("handleMessage.py", "def handleMessage(payload):\n\tpass")],
            )
            .resource(
                "projects/p/com.inductiveautomation.perspective/message/sess",
                &[(
                    "handleMessage.py",
                    "def handleMessage(session, payload):\n\tpass",
                )],
            )
    }

    #[test]
    fn perspective_messages() {
        // The handler `m` listens on session and view scope, not page.
        let body = "\tsystem.perspective.sendMessage('m', scope='session')\n\tsystem.perspective.sendMessage('m')\n\tsystem.perspective.sendMessage('nobody', {}, 'session')\n\tsystem.perspective.sendMessage(t)\n\tsystem.perspective.sendMessage('m', scope=s)";
        let f =
            with_handlers(Fixture::new().with_view("Main", &view_json(ViewScript::Action, body)));
        let d = f.lint(&["REF006"]);
        assert_eq!(decoded_flagged(&d), vec!["'m'", "'nobody'"]);
        assert_eq!(
            messages(&d),
            vec![
                "Message handlers of `m` do not listen on scope `page`",
                "No component message handler listens for `nobody`"
            ]
        );
    }

    #[test]
    fn gateway_messages() {
        let body = "\tp = system.util.getProjectName()\n\tsystem.util.sendMessage(p, 'gw')\n\tsystem.util.sendMessage('p', 'sess', scope='S')\n\tsystem.util.sendMessage('p', 'gw', scope='S')\n\tsystem.util.sendRequest('p', 'sess')\n\tsystem.util.sendMessage('p', 'gone')\n\tsystem.util.sendMessage('elsewhere', 'gone')\n\tsystem.util.sendMessage(name, 'gone')";
        let f =
            with_handlers(Fixture::new().with_view("Main", &view_json(ViewScript::Action, body)));
        let d = f.lint(&["REF006"]);
        assert_eq!(
            messages(&d),
            vec![
                "Project `p` has no message handler `gw` for scope `S`",
                "Project `p` has no gateway message handler `sess`",
                "Project `p` has no message handler `gone`"
            ]
        );
    }
}
