//! SCR121 global-at-module-level.

use iglint_python::ast::Stmt;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::report;

pub static META: RuleMeta = RuleMeta {
    code: "SCR121",
    name: "global-at-module-level",
    category: Category::Script,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "`global` statement at module level",
    explanation: r#"## What it does

Reports `global` statements at the top level of a library module.

## Why is this bad?

At module level every name is already global; the statement does nothing.
It often signals a misunderstanding: Ignition library modules are loaded
once per project and their module-level variables are shared by every
caller, which `global` does not change.

Inside Perspective and event scripts `global` is not at module level (the
script is a function body) and is not reported.

## Example

```python
global cache
cache = {}
```

Use instead:

```python
cache = {}
```

## pylint

Corresponds to `W0604 global-at-module-level`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&GlobalAtModuleLevel)],
    options: None,
};

struct GlobalAtModuleLevel;

fn collect(body: &[Stmt], out: &mut Vec<iglint_text::TextRange>) {
    for s in body {
        match s {
            Stmt::Global(g) => out.push(g.range),
            Stmt::If(i) => {
                collect(&i.body, out);
                collect(&i.orelse, out);
            }
            Stmt::For(f) => {
                collect(&f.body, out);
                collect(&f.orelse, out);
            }
            Stmt::While(w) => {
                collect(&w.body, out);
                collect(&w.orelse, out);
            }
            Stmt::With(w) => collect(&w.body, out),
            Stmt::TryExcept(t) => {
                collect(&t.body, out);
                for h in &t.handlers {
                    collect(&h.body, out);
                }
                collect(&t.orelse, out);
            }
            Stmt::TryFinally(t) => {
                collect(&t.body, out);
                collect(&t.finalbody, out);
            }
            _ => {}
        }
    }
}

impl ScriptRule for GlobalAtModuleLevel {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found = Vec::new();
        collect(&unit.parsed.module.body, &mut found);
        for range in found {
            report(cx, unit, range, "`global` at module level has no effect");
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn module_level_only() {
        let d = lint_module(
            "SCR121",
            "global a\nif x:\n    global b\ndef f():\n    global c\n",
        );
        assert_eq!(flagged(&d), ["global a", "global b"]);
        assert!(
            view(ViewScript::Action, "\tglobal x\n\tx \u{3d} 1")
                .lint(&["SCR121"])
                .is_empty()
        );
    }
}
