//! SCR115 except-tuple-without-parentheses.

use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::BindingKind;
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{fix, report, semantic, source, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR115",
    name: "except-tuple-without-parentheses",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    fix: FixAvailability::Always,
    summary: "`except A, B:` binds `B` instead of catching it",
    explanation: r#"## What it does

Reports `except ValueError, TypeError:` where the name after the comma is
an exception class: a builtin exception, or a name the module imports or
defines as a class.

## Why is this bad?

In Python 2, `except A, B:` means "catch `A` and store the exception in
the variable `B`". `TypeError` is not caught, and worse, the name
`TypeError` is rebound to the caught exception for the rest of the
function.

## Example

```python
try:
    value = int(text)
except ValueError, TypeError:
    value = 0
```

Use instead:

```python
try:
    value = int(text)
except (ValueError, TypeError):
    value = 0
```

## Fix

Adds the parentheses. Safe: the result is what the code says it means.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&ExceptTuple)],
    options: None,
};

struct ExceptTuple;

impl ScriptRule for ExceptTuple {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let text = source(unit);
        let mut found: Vec<(TextRange, TextRange, String)> = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::TryExcept(t) = s else { return };
            for h in &t.handlers {
                let (Some(ty), Some(name)) = (&h.type_, &h.name) else {
                    continue;
                };
                let Expr::Name(n) = &**name else { continue };
                let builtin_exception = iglint_stubs::jython_builtins()
                    .iter()
                    .any(|b| b.name == &*n.id && b.kind == iglint_stubs::BuiltinKind::Exception);
                let class_like = model.bindings().any(|(_, b)| {
                    b.name == &*n.id
                        && matches!(
                            b.kind,
                            BindingKind::Import | BindingKind::FromImport | BindingKind::ClassDef
                        )
                });
                if !(builtin_exception || class_like) {
                    continue;
                }
                let whole = ty.range().cover(n.range);
                // Parenthesized type already (`except (A), B:`) or text we
                // cannot rewrite safely.
                if text[whole.to_std()].contains('#') {
                    continue;
                }
                found.push((whole, n.range, n.id.to_string()));
            }
        });
        for (whole, name_range, name) in found {
            let replacement = format!("({})", &text[whole.to_std()]);
            let fix = fix(
                cx,
                unit,
                "Add parentheses",
                Applicability::Safe,
                vec![(whole, replacement)],
            );
            let message = format!(
                "`{name}` is bound to the caught exception, not caught; use parentheses to catch both"
            );
            let _ = report(cx, unit, name_range, message).fix_opt(fix);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_and_fixes() {
        let f = library(
            "m",
            "try:\n    f()\nexcept ValueError, TypeError:\n    pass\n",
        );
        let d = f.lint(&["SCR115"]);
        assert_eq!(flagged(&d), ["TypeError"]);
        let fixed = f.fixed(&[("SCR115", "")], false, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "try:\n    f()\nexcept (ValueError, TypeError):\n    pass\n"
        );
    }

    #[test]
    fn imported_classes() {
        let d = lint_module(
            "SCR115",
            "from java.lang import Throwable\ntry:\n    f()\nexcept Exception, Throwable:\n    pass\n",
        );
        assert_eq!(flagged(&d), ["Throwable"]);
    }

    #[test]
    fn exception_variables_are_fine() {
        assert!(
            lint_module(
                "SCR115",
                "try:\n    f()\nexcept ValueError, e:\n    print e\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn fix_in_view() {
        let f = view(
            ViewScript::Action,
            "\ttry:\n\t\tf()\n\texcept KeyError, IndexError:\n\t\tpass",
        );
        let fixed = f.fixed(&[("SCR115", "")], false, &view_path(&f));
        assert!(
            fixed.contains(r"\texcept (KeyError, IndexError):"),
            "{fixed}"
        );
    }
}
