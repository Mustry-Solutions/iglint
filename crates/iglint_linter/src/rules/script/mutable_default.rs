//! SCR107 mutable-default-argument.

use iglint_python::ast::{Expr, Stmt};
use iglint_text::Ranged;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{is_builtin_reference, report, semantic, walk_exprs, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR107",
    name: "mutable-default-argument",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Mutable value as a parameter default",
    explanation: r#"## What it does

Reports parameter defaults that are mutable objects created once: list,
dict and set literals and comprehensions, and calls to the builtins
`list()`, `dict()`, `set()` and `bytearray()`.

## Why is this bad?

The default is evaluated once, when the `def` runs. Every call that does
not pass the argument shares the same object, so changes made by one call
are seen by the next. Library modules live for the whole gateway uptime:
such a list keeps growing across every Perspective session.

## Example

```python
def addAlarm(alarm, seen=[]):
    seen.append(alarm)
    return len(seen)
```

Use instead:

```python
def addAlarm(alarm, seen=None):
    if seen is None:
        seen = []
    seen.append(alarm)
    return len(seen)
```

## pylint

Corresponds to `W0102 dangerous-default-value`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&MutableDefault)],
    options: None,
};

struct MutableDefault;

impl ScriptRule for MutableDefault {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut defaults: Vec<&Expr> = Vec::new();
        let body = &unit.parsed.module.body;
        walk_stmts(body, &mut |s| {
            if let Stmt::FunctionDef(f) = s {
                defaults.extend(f.args.defaults.iter());
            }
        });
        walk_exprs(body, &mut |e| {
            if let Expr::Lambda(l) = e {
                defaults.extend(l.args.defaults.iter());
            }
        });
        defaults.sort_by_key(|d| d.range().start());
        for d in defaults {
            let mutable = match d {
                Expr::List(_)
                | Expr::Dict(_)
                | Expr::Set(_)
                | Expr::ListComp(_)
                | Expr::DictComp(_)
                | Expr::SetComp(_) => true,
                Expr::Call(c) => ["list", "dict", "set", "bytearray"]
                    .iter()
                    .any(|n| is_builtin_reference(model, &c.func, n)),
                _ => false,
            };
            if mutable {
                report(
                    cx,
                    unit,
                    d.range(),
                    "Mutable default argument: the same object is shared by every call",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_mutable_defaults() {
        let d = lint_module(
            "SCR107",
            "def f(a=[], b={}, c=set(), d=(), e=None, g=lambda x={1}: x):\n    pass\n",
        );
        assert_eq!(flagged(&d), ["[]", "{}", "set()", "{1}"]);
    }

    #[test]
    fn shadowed_builtins_are_not_mutable() {
        assert!(
            lint_module(
                "SCR107",
                "def list(): return ()\ndef f(a=list()):\n    pass\n"
            )
            .is_empty()
        );
    }
}
