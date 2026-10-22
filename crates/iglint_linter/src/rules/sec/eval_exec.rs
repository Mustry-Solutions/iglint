//! SEC003 eval-exec.

use iglint_python::ast::{Expr, Stmt};
use iglint_text::{Ranged, TextRange, TextSize};

use super::{MAX_DEPTH, is_fixed, single_assigned_value};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{is_builtin_reference, report, semantic, walk_exprs, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SEC003",
    name: "eval-exec",
    category: Category::Sec,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`eval`, `exec`, `execfile` or `compile` on code built at run time",
    explanation: r#"## What it does

Reports calls of the builtins `eval`, `execfile` and `compile`, and the
`exec` statement, when the code (or file name) they receive is not a
literal: a parameter, a tag value, a payload field, text built from
values, ...

Only the builtins count: a local function or variable that shadows
`eval` is not reported, and neither are methods such as `re.compile`.
Literal code (`eval("1 + 2")`), names whose every reaching assignment is
a literal, and text concatenated from literals are not reported.

## Why is this bad?

The scripts run on the gateway with the gateway's permissions. Code that
comes from a tag, a database row, a message payload or a user input field
can do anything the gateway can: read files, call other systems, change
tags. Even when the source is trusted today, such code escapes review,
linting and the Designer's search.

There is almost always a direct alternative: `getattr` for a name chosen
at run time, a dict of functions for a command chosen at run time,
`system.util.jsonDecode` or `ast.literal_eval`-style parsing for data.

## Example

```python
def runAction(self, event):
    handler = self.view.params.handler
    eval("project.handlers.%s(event)" % handler)
```

Use instead:

```python
HANDLERS = {"start": handlers.start, "stop": handlers.stop}

def runAction(self, event):
    HANDLERS[self.view.params.handler](event)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&EvalExec)],
    options: None,
};

struct EvalExec;

impl ScriptRule for EvalExec {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Call(call) = e else { return };
            let Expr::Name(n) = &*call.func else { return };
            let name = &*n.id;
            if !matches!(name, "eval" | "execfile" | "compile")
                || !is_builtin_reference(model, &call.func, name)
            {
                return;
            }
            let Some(code) = call.args.first() else {
                return;
            };
            if is_fixed(model, code) {
                return;
            }
            let what = if name == "execfile" {
                "a file name"
            } else {
                "code"
            };
            found.push((
                call.func.range(),
                format!("`{name}` runs {what} that is not a literal"),
            ));
        });
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::Exec(exec) = s else { return };
            // `exec(code, namespace)` is the tuple form of `exec code in
            // namespace`.
            let code = match &*exec.body {
                Expr::Tuple(t) if exec.globals.is_none() && matches!(t.elts.len(), 2 | 3) => {
                    &t.elts[0]
                }
                body => body,
            };
            // `exec compile(...)`, `code = compile(...)` then `exec code`:
            // the code is checked at `compile`.
            if let Some(Expr::Call(c)) =
                std::iter::successors(Some(code), |e| single_assigned_value(model, e))
                    .take(MAX_DEPTH as usize)
                    .last()
                && is_builtin_reference(model, &c.func, "compile")
            {
                return;
            }
            if is_fixed(model, code) {
                return;
            }
            found.push((
                TextRange::at(exec.range.start(), TextSize::of("exec")),
                "`exec` runs code that is not a literal".to_owned(),
            ));
        });
        found.sort_by_key(|(r, _)| r.start());
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_dynamic_code() {
        let code = r#"def f(text, path):
    eval(text)
    exec text
    exec "x = " + text in {}
    execfile(path)
    compile(text, "<string>", "exec")
    eval("1 + " + text)
    exec compile(text, "<s>", "exec") in {}
    exec(text, {})
"#;
        let d = lint_module("SEC003", code);
        assert_eq!(
            lines(&d),
            [
                (2, "eval".to_owned()),
                (3, "exec".to_owned()),
                (4, "exec".to_owned()),
                (5, "execfile".to_owned()),
                (6, "compile".to_owned()),
                (7, "eval".to_owned()),
                (8, "compile".to_owned()),
                (9, "exec".to_owned()),
            ]
        );
        assert_eq!(
            d[3].message,
            "`execfile` runs a file name that is not a literal"
        );
    }

    #[test]
    fn literals_shadowing_and_methods_are_fine() {
        let code = r#"import re
CODE = "a = 1"
def eval(x):
    return x
def f(text):
    eval(text)
    exec "a = 1"
    exec CODE
    compile("1", "<s>", "eval")
    exec compile("a = 1", "<s>", "exec")
    compiled = compile(text, "<s>", "exec")
    exec compiled in {}
    exec("a = 1", {})
    re.compile(text)
    obj.eval(text)
def g(text, compile):
    compile(text)
"#;
        let d = lint_module("SEC003", code);
        assert_eq!(lines(&d), [(11, "compile".to_owned())]);
    }

    #[test]
    fn everywhere_and_gateway_events() {
        assert_eq!(
            flagged_everywhere(
                "SEC003",
                "\tx = eval(self.custom.expr)\n\texec \"y = \\\"a\\\"\""
            ),
            ["eval"]
        );
        let f = gateway_message("def handleMessage(payload):\n\texec payload['code']\n");
        assert_eq!(flagged(&f.lint(&["SEC003"])), ["exec"]);
    }
}
