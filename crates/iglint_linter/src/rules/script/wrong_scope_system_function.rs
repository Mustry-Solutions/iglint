//! SCR205 wrong-scope-system-function.

use iglint_project::RunScope;
use iglint_python::ast::{Expr, ExprContext};
use iglint_stubs::{Scope, system_member};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::system_api::{
    called_function, is_checkable, keyword_names, scope_names, script_scope, system_path,
};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR205",
    name: "wrong-scope-system-function",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`system.*` function not available where the script runs",
    explanation: r#"## What it does

Reports `system.*` members that do not exist in the scope the script runs
in, and Perspective functions called without a session where there is
none.

* Perspective scripts (component and view events, transforms, change
  scripts, message handlers, custom methods, session and page events) run
  on the gateway inside a session: `system.perspective.*` is available,
  `system.gui.*`, `system.nav.*` and other Vision Client functions are
  not.
* Gateway events, WebDev handlers and tag event scripts run on the gateway
  without a Perspective session. `system.perspective` exists there, but
  its functions that act on "the current session or page" (those with a
  `sessionId` parameter, such as `navigate`, `openPopup`, `sendMessage`
  or `print`) fail unless `sessionId` is given, positionally or by
  keyword. Calls that unpack arguments (`*args`, `**kwargs`) are not
  reported.
* Vision Client scripts run in the Vision Client scope: gateway-only
  functions such as `system.perspective.*` are not available.

Only the first member that is missing is reported (`system.gui` in
`system.gui.messageBox`). Library modules are not reported: they run in
the scope of whatever imports them, and their `hintScope` only controls
the Designer's code hints.

## Why is this bad?

The script raises `AttributeError` (the member does not exist in that
scope) or fails because there is no session to act on.

## Example

```python
def runAction(self, event):
    system.gui.messageBox("Saved")
```

Use instead:

```python
def runAction(self, event):
    system.perspective.openPopup("saved", "Popups/Saved")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&WrongScopeSystemFunction)],
    options: None,
};

struct WrongScopeSystemFunction;

fn scope_description(run_scope: RunScope) -> &'static str {
    match run_scope {
        RunScope::PerspectiveComponent => "Perspective scripts, which run on the gateway",
        RunScope::PerspectiveSession => "Perspective session events, which run on the gateway",
        RunScope::Gateway => "gateway scripts",
        RunScope::Tag => "tag event scripts, which run on the gateway",
        RunScope::VisionClient => "Vision Client scripts",
        RunScope::Library => "library modules",
    }
}

impl ScriptRule for WrongScopeSystemFunction {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(scope) = script_scope(unit) else {
            return;
        };
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let run_scope = unit.script.run_scope;
        let no_session = matches!(run_scope, RunScope::Gateway | RunScope::Tag);
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| match e {
            Expr::Attribute(a) if a.ctx == ExprContext::Load => {
                let Some(path) = system_path(model, e) else {
                    return;
                };
                let Some(member) = system_member(&path) else {
                    return;
                };
                if member.scopes().contains(scope) {
                    return;
                }
                // Report only the first missing member of the chain.
                let parent = path.rsplit_once('.').map_or("system", |(p, _)| p);
                if parent != "system"
                    && !system_member(parent).is_some_and(|m| m.scopes().contains(scope))
                {
                    return;
                }
                found.push((
                    a.range,
                    format!(
                        "`{path}` is not available in {}; it exists in {}",
                        scope_description(run_scope),
                        scope_names(member.scopes())
                    ),
                ));
            }
            Expr::Call(call) if no_session && is_checkable(call) => {
                let Some((path, function)) = called_function(model, call) else {
                    return;
                };
                if !path.starts_with("system.perspective.") {
                    return;
                }
                let Some(overload) = function
                    .variant(Scope::Gateway)
                    .and_then(|v| v.keyword_overload())
                else {
                    return;
                };
                let Some(index) = overload
                    .params
                    .iter()
                    .position(|p| p.name == Some("sessionId"))
                else {
                    return;
                };
                if call.args.len() > index || keyword_names(call).contains(&"sessionId") {
                    return;
                }
                found.push((
                    call.range,
                    format!(
                        "`{path}` without `sessionId` fails in {}: there is no current Perspective session",
                        scope_description(run_scope)
                    ),
                ));
            }
            _ => {}
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;
    use crate::rules::test_support::Fixture;

    #[test]
    fn vision_functions_in_perspective() {
        let d = view(
            ViewScript::Action,
            "\tsystem.gui.messageBox('x')\n\tsystem.nav.openWindow('w')\n\tsystem.perspective.navigate('/')\n\tsystem.vision.showMessage('m')",
        )
        .lint(&["SCR205"]);
        assert_eq!(
            decoded_flagged(&d),
            ["system.gui", "system.nav", "system.vision"]
        );
        assert_eq!(
            messages(&d)[0],
            "`system.gui` is not available in Perspective scripts, which run on the gateway; it exists in the Vision Client and the Designer"
        );
    }

    #[test]
    fn members_missing_in_a_known_package() {
        // `system.util.invokeLater` is a Vision Client and Designer function.
        let d = view(
            ViewScript::Transform,
            "\tsystem.util.invokeLater(f)\n\treturn value",
        )
        .lint(&["SCR205"]);
        assert_eq!(decoded_flagged(&d), ["system.util.invokeLater"]);
    }

    #[test]
    fn every_perspective_location() {
        assert_eq!(
            flagged_everywhere_but_library("\tsystem.gui.warningBox('x')"),
            ["system.gui"]
        );
        let d = session_event("def onStartup(session):\n\tsystem.gui.errorBox('x')\n\tsystem.perspective.navigate('/')\n")
            .lint(&["SCR205"]);
        assert_eq!(flagged(&d), ["system.gui"]);
    }

    fn flagged_everywhere_but_library(body: &str) -> Vec<String> {
        let mut all = Vec::new();
        for kind in [
            ViewScript::Action,
            ViewScript::Transform,
            ViewScript::Change,
            ViewScript::Handler,
            ViewScript::Method,
        ] {
            let d = view(kind, body).lint(&["SCR205"]);
            let f = decoded_flagged(&d);
            if all.is_empty() {
                all = f;
            } else {
                assert_eq!(f, all, "{kind:?}");
            }
        }
        all
    }

    #[test]
    fn perspective_without_session_in_gateway_scripts() {
        let code = "def handleTimerEvent():\n\tsystem.perspective.navigate('/x')\n\tsystem.perspective.sendMessage('m', {}, 'session', 'abc')\n\tsystem.perspective.sendMessage('m', sessionId\u{3d}'abc', pageId\u{3d}'p')\n\tsystem.perspective.print('x')\n\tsystem.perspective.getSessionInfo()\n\tsystem.perspective.openPopup(**kw)\n";
        let d = gateway_timer(code).lint(&["SCR205"]);
        assert_eq!(
            flagged(&d),
            [
                "system.perspective.navigate('/x')",
                "system.perspective.print('x')"
            ]
        );
        assert_eq!(
            messages(&d)[0],
            "`system.perspective.navigate` without `sessionId` fails in gateway scripts: there is no current Perspective session"
        );
        let d = tag_event("\tsystem.perspective.sendMessage('m')").lint(&["SCR205"]);
        assert_eq!(decoded_flagged(&d), ["system.perspective.sendMessage('m')"]);
        let d = webdev("def doGet(request, session):\n\tsystem.perspective.closeSession()\n")
            .lint(&["SCR205"]);
        assert_eq!(flagged(&d), ["system.perspective.closeSession()"]);
        // Inside a session the current session is used.
        assert!(
            view(ViewScript::Action, "\tsystem.perspective.navigate('/x')")
                .lint(&["SCR205"])
                .is_empty()
        );
    }

    #[test]
    fn gateway_functions_in_vision_client_scripts() {
        let f = Fixture::new().resource(
            "projects/p/com.inductiveautomation.vision/timer/t",
            &[(
                "handleTimerEvent.py",
                "def handleTimerEvent():\n\tsystem.perspective.navigate('/')\n\tsystem.gui.messageBox('x')\n",
            )],
        );
        let d = f.lint(&["SCR205"]);
        assert_eq!(flagged(&d), ["system.perspective"]);
        assert!(messages(&d)[0].contains("Vision Client scripts"));
    }

    #[test]
    fn library_modules_are_not_reported() {
        assert!(
            lint_module(
                "SCR205",
                "def f():\n    system.gui.messageBox('x')\n    system.perspective.navigate('/')\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn local_system_and_aliases() {
        let d = view(
            ViewScript::Action,
            "\tg \u{3d} system.gui\n\tg.messageBox('x')",
        )
        .lint(&["SCR205"]);
        assert_eq!(decoded_flagged(&d), ["system.gui"]);
        assert!(
            view(
                ViewScript::Action,
                "\tsystem \u{3d} Helper()\n\tsystem.gui.messageBox('y')"
            )
            .lint(&["SCR205"])
            .is_empty()
        );
    }
}
