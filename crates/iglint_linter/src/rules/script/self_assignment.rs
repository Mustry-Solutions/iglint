//! SCR109 self-assignment.

use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::{Resolution, ScopeKind};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR109",
    name: "self-assignment",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Variable assigned to itself",
    explanation: r#"## What it does

Reports `x = x` and the matching positions of `a, b = a, b`.

Not reported in class bodies (`x = x` there copies a module variable into
the class) and when the value is a builtin or an Ignition global
(`len = len` makes a module-level alias).

## Why is this bad?

The statement does nothing. Usually another name was meant on one side
(`self.value = value` written as `value = value`).

## Example

```python
def valueChanged(self, previousValue, currentValue, origin, missedEvents):
    currentValue = currentValue
```

Use instead:

```python
def valueChanged(self, previousValue, currentValue, origin, missedEvents):
    self.custom.current = currentValue
```

## pylint

Corresponds to `W0127 self-assigning-variable`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&SelfAssignment)],
    options: None,
};

struct SelfAssignment;

fn pairs<'a>(target: &'a Expr, value: &'a Expr, out: &mut Vec<(&'a Expr, &'a Expr)>) {
    match (target, value) {
        (Expr::Tuple(t), Expr::Tuple(v)) if t.elts.len() == v.elts.len() => {
            for (a, b) in t.elts.iter().zip(&v.elts) {
                pairs(a, b, out);
            }
        }
        (Expr::List(t), Expr::List(v)) if t.elts.len() == v.elts.len() => {
            for (a, b) in t.elts.iter().zip(&v.elts) {
                pairs(a, b, out);
            }
        }
        (Expr::Tuple(t), Expr::List(v)) if t.elts.len() == v.elts.len() => {
            for (a, b) in t.elts.iter().zip(&v.elts) {
                pairs(a, b, out);
            }
        }
        (Expr::List(t), Expr::Tuple(v)) if t.elts.len() == v.elts.len() => {
            for (a, b) in t.elts.iter().zip(&v.elts) {
                pairs(a, b, out);
            }
        }
        _ => out.push((target, value)),
    }
}

impl ScriptRule for SelfAssignment {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::Assign(a) = s else { return };
            let mut all = Vec::new();
            for t in &a.targets {
                pairs(t, &a.value, &mut all);
            }
            for (t, v) in all {
                let (Expr::Name(t), Expr::Name(v)) = (t, v) else {
                    continue;
                };
                if t.id != v.id {
                    continue;
                }
                let Some(r) = model.reference_at(v.range) else {
                    continue;
                };
                let implicit = !r.bindings.is_empty()
                    && r.bindings.iter().all(|b| model.binding(*b).is_implicit());
                if model.scope(r.scope).kind == ScopeKind::Class
                    || implicit
                    || matches!(r.resolution, Resolution::Builtin | Resolution::Injected)
                {
                    continue;
                }
                found.push((t.range, format!("`{}` is assigned to itself", t.id)));
            }
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_self_assignment() {
        let d = lint_module(
            "SCR109",
            "def f(a, b):\n    a = a\n    a, b = a, c\n    b, a = b, a\n",
        );
        assert_eq!(
            lines(&d),
            [
                (2, "a".to_owned()),
                (3, "a".to_owned()),
                (4, "b".to_owned()),
                (4, "a".to_owned())
            ]
        );
    }

    #[test]
    fn exemptions() {
        assert!(lint_module("SCR109", "x = 1\nclass C:\n    x = x\n").is_empty());
        assert!(lint_module("SCR109", "len = len\nsystem = system\n").is_empty());
        assert!(lint_module("SCR109", "def f(a):\n    a = a + 1\n    self.a = a\n").is_empty());
    }
}
