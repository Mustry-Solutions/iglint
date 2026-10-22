//! SCR302 sleep-in-event-script.

use iglint_python::ast::{Expr, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::SemanticModel;
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};
use iglint_project::ScriptKind;

pub static META: RuleMeta = RuleMeta {
    code: "SCR302",
    name: "sleep-in-event-script",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`time.sleep` or `Thread.sleep` in a Perspective event script",
    explanation: r#"## What it does

Reports calls to `time.sleep` and `java.lang.Thread.sleep` (also through
imports and aliases such as `from time import sleep`) in Perspective
event script actions, script transforms, property change scripts,
component message handlers and component extension functions.

Only calls that run directly when the script runs are reported: a call
inside a nested `def` or `lambda` (typically a function handed to
`system.util.invokeAsynchronous`) runs elsewhere and is not reported.
Custom methods are not checked, because callers may run them on a
background thread.

## Why is this bad?

These scripts run on the gateway in the session's (or binding's) thread.
Sleeping blocks it: other events of the session queue up behind the
sleeping script, bindings stop updating and the page appears frozen.
Schedule delayed work with `system.util.invokeAsynchronous` or a timer
instead.

## Example

```python
def runAction(self, event):
    system.tag.writeBlocking(["[default]Line/Start"], [True])
    time.sleep(2)
    system.tag.writeBlocking(["[default]Line/Start"], [False])
```

Use instead:

```python
def runAction(self, event):
    system.tag.writeBlocking(["[default]Line/Start"], [True])
    def reset():
        time.sleep(2)
        system.tag.writeBlocking(["[default]Line/Start"], [False])
    system.util.invokeAsynchronous(reset)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&SleepInEventScript)],
    options: None,
};

struct SleepInEventScript;

/// Calls that block the calling thread.
pub(crate) const SLEEPS: &[&str] = &["time.sleep", "java.lang.Thread.sleep"];

impl ScriptRule for SleepInEventScript {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !matches!(
            unit.script.kind,
            ScriptKind::EventAction
                | ScriptKind::Transform
                | ScriptKind::PropertyChange
                | ScriptKind::MessageHandler
                | ScriptKind::ExtensionFunction { .. }
        ) {
            return;
        }
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let name: &str = &unit.script.function_name;
        let Some(function) = unit.parsed.module.body.iter().find_map(|s| match s {
            Stmt::FunctionDef(f) if &*f.name.id == name => Some(f),
            _ => None,
        }) else {
            return;
        };
        let mut v = Sleeps {
            model,
            found: Vec::new(),
        };
        for stmt in &function.body {
            v.visit_stmt(stmt);
        }
        for (range, callee) in v.found {
            report(
                cx,
                unit,
                range,
                format!("`{callee}` blocks the Perspective session's thread while it sleeps"),
            );
        }
    }
}

/// Collects sleep calls that run when the enclosing code runs: not those
/// in nested function bodies or lambdas.
struct Sleeps<'m, 's> {
    model: &'m SemanticModel<'s>,
    found: Vec<(TextRange, String)>,
}

impl<'a> Visitor<'a> for Sleeps<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if let Stmt::FunctionDef(f) = stmt {
            // Decorators and defaults run now; the body runs later.
            for d in &f.decorator_list {
                self.visit_expr(d);
            }
            for d in &f.args.defaults {
                self.visit_expr(d);
            }
            return;
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Lambda(l) => {
                for d in &l.args.defaults {
                    self.visit_expr(d);
                }
                return;
            }
            Expr::Call(call) => {
                if let Some(callee) = self.model.callee(call)
                    && SLEEPS.contains(&callee.as_str())
                {
                    self.found.push((call.range, callee));
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn sleeps_in_every_event_location() {
        let body =
            "\timport time\n\tfrom java.lang import Thread\n\ttime.sleep(1)\n\tThread.sleep(100)";
        for kind in [
            ViewScript::Action,
            ViewScript::Transform,
            ViewScript::Change,
            ViewScript::Handler,
        ] {
            let d = view(kind, body).lint(&["SCR302"]);
            assert_eq!(
                decoded_flagged(&d),
                ["time.sleep(1)", "Thread.sleep(100)"],
                "{kind:?}"
            );
        }
        let d = view(ViewScript::Action, "\timport time\n\ttime.sleep(1)").lint(&["SCR302"]);
        assert_eq!(
            messages(&d),
            ["`time.sleep` blocks the Perspective session's thread while it sleeps"]
        );
    }

    #[test]
    fn aliases() {
        let d = view(
            ViewScript::Action,
            "\tfrom time import sleep as nap\n\timport java.lang.Thread as T\n\tnap(1)\n\tT.sleep(5)\n\tjava.lang.Thread.sleep(1)",
        )
        .lint(&["SCR302"]);
        assert_eq!(decoded_flagged(&d), ["nap(1)", "T.sleep(5)"]);
    }

    #[test]
    fn nested_functions_and_lambdas_are_not_reported() {
        let body = "\timport time\n\tdef later():\n\t\ttime.sleep(5)\n\tsystem.util.invokeAsynchronous(later)\n\tf \u{3d} lambda: time.sleep(1)\n\tclass C:\n\t\tdef m(self):\n\t\t\ttime.sleep(1)";
        assert!(view(ViewScript::Action, body).lint(&["SCR302"]).is_empty());
    }

    #[test]
    fn other_locations_are_not_reported() {
        let body = "\timport time\n\ttime.sleep(1)";
        assert!(view(ViewScript::Method, body).lint(&["SCR302"]).is_empty());
        assert!(lint_module("SCR302", "import time\ntime.sleep(1)\n").is_empty());
        assert!(
            gateway_timer("def handleTimerEvent():\n\timport time\n\ttime.sleep(1)\n")
                .lint(&["SCR302"])
                .is_empty()
        );
        assert!(tag_event(body).lint(&["SCR302"]).is_empty());
        // A local `time` that is not the module.
        assert!(
            view(ViewScript::Action, "\ttime \u{3d} Clock()\n\ttime.sleep(1)")
                .lint(&["SCR302"])
                .is_empty()
        );
    }
}
