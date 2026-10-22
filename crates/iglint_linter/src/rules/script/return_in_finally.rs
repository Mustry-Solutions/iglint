//! SCR119 return-in-finally.

use iglint_python::ast::Stmt;
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR119",
    name: "return-in-finally",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`return` or `break` in a `finally` clause",
    explanation: r#"## What it does

Reports `return` in a `finally` clause, and `break` in a `finally` clause
that leaves a loop outside the `try`.

## Why is this bad?

When the `try` body raises, the `finally` clause runs and the `return` or
`break` discards the exception: the error vanishes without a trace, not
even in the gateway log.

## Example

```python
def readSetpoint(path):
    try:
        return system.tag.readBlocking([path])[0].value
    finally:
        return None
```

Use instead:

```python
def readSetpoint(path):
    try:
        return system.tag.readBlocking([path])[0].value
    except Exception:
        return None
```

## pylint

Corresponds to `W0150 lost-exception`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&ReturnInFinally)],
    options: None,
};

struct ReturnInFinally;

/// `return` and loop-leaving `break` statements of a `finally` body, not
/// looking into nested functions and classes.
fn collect(body: &[Stmt], in_loop: bool, out: &mut Vec<(TextRange, &'static str)>) {
    for stmt in body {
        match stmt {
            Stmt::Return(r) => out.push((r.range, "return")),
            Stmt::Break(b) if !in_loop => out.push((b.range, "break")),
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            Stmt::For(s) => {
                collect(&s.body, true, out);
                collect(&s.orelse, in_loop, out);
            }
            Stmt::While(s) => {
                collect(&s.body, true, out);
                collect(&s.orelse, in_loop, out);
            }
            Stmt::If(s) => {
                collect(&s.body, in_loop, out);
                collect(&s.orelse, in_loop, out);
            }
            Stmt::With(s) => collect(&s.body, in_loop, out),
            Stmt::TryExcept(s) => {
                collect(&s.body, in_loop, out);
                for h in &s.handlers {
                    collect(&h.body, in_loop, out);
                }
                collect(&s.orelse, in_loop, out);
            }
            Stmt::TryFinally(s) => {
                collect(&s.body, in_loop, out);
                collect(&s.finalbody, in_loop, out);
            }
            _ => {}
        }
    }
}

impl ScriptRule for ReturnInFinally {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            if let Stmt::TryFinally(t) = s {
                collect(&t.finalbody, false, &mut found);
            }
        });
        found.sort_by_key(|(r, _)| r.start());
        found.dedup();
        for (range, keyword) in found {
            report(
                cx,
                unit,
                range,
                format!("`{keyword}` in `finally` discards any exception raised in the `try`"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_return_and_break() {
        let code = "def f():\n    for x in y:\n        try:\n            g()\n        finally:\n            if x:\n                break\n            for z in x:\n                break\n    try:\n        g()\n    finally:\n        return 1\n";
        let d = lint_module("SCR119", code);
        assert_eq!(
            lines(&d),
            [(7, "break".to_owned()), (13, "return 1".to_owned())]
        );
    }

    #[test]
    fn nested_functions_are_fine() {
        assert!(
            lint_module(
                "SCR119",
                "try:\n    g()\nfinally:\n    def h():\n        return 1\n"
            )
            .is_empty()
        );
    }
}
