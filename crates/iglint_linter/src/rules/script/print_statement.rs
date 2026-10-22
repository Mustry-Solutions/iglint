//! SCR122 print-statement.

use iglint_project::RunScope;
use iglint_python::ast::Stmt;
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR122",
    name: "print-statement",
    category: Category::Script,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "`print` in a script that runs on the gateway",
    explanation: r#"## What it does

Reports `print` statements in scripts that run on the gateway: gateway
events, WebDev handlers, tag events, Perspective scripts (Perspective runs
all Python on the gateway) and library modules. Vision client scripts are
not reported.

## Why is this bad?

On the gateway `print` goes to the wrapper log (`wrapper.log`), without a
logger name, level or timestamp from the gateway's logging system, and
cannot be filtered or viewed in the gateway's log page. Use a logger
(`system.util.getLogger("MyProject.Area")`), or
`system.perspective.print(...)` to write to the browser console of a
Perspective session.

## Example

```python
def onTagChange(initialChange, newValue, previousValue, event, executionCount):
    print "new value", newValue.value
```

Use instead:

```python
def onTagChange(initialChange, newValue, previousValue, event, executionCount):
    system.util.getLogger("Plant.TagEvents").info("new value %s" % newValue.value)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&PrintStatement)],
    options: None,
};

struct PrintStatement;

impl ScriptRule for PrintStatement {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() || unit.script.run_scope == RunScope::VisionClient {
            return;
        }
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            if let Stmt::Print(p) = s {
                found.push(TextRange::at(p.range.start(), TextSize::of("print")));
            }
        });
        for range in found {
            report(
                cx,
                unit,
                range,
                "`print` on the gateway goes to the wrapper log; use `system.util.getLogger(...)`",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_prints() {
        let d = gateway_message("def handleMessage(payload):\n\tprint payload\n").lint(&["SCR122"]);
        assert_eq!(flagged(&d), ["print"]);
        assert_eq!(
            flagged(&lint_module("SCR122", "print >>f, 'x'\n")),
            ["print"]
        );
    }
}
