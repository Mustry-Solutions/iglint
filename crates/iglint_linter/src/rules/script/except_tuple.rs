//! SCR115 except-tuple-without-parentheses.

use iglint_python::ast::{ExceptHandler, Expr, ExprName, Stmt};
use iglint_python_semantic::{BindingKind, SemanticModel};
use iglint_text::{Ranged, TextRange};
use rustc_hash::FxHashSet;

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
    fix_safety: crate::registry::FixSafety::Unsafe,
    summary: "`except A, B:` binds `B` instead of catching it",
    explanation: r#"## What it does

Reports `except ValueError, TypeError:` where the name after the comma is
an exception class: a builtin exception, a class the script defines, or a
capitalized name it imports with `from m import Name`. The name is looked
up like Python does (the handler's function, enclosing functions, the
module, the builtins), ignoring the handler itself. A module
(`except ValueError, sys:` after `import sys`), any other variable, and a
name the handler body reads are variables, as written.

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

`SCR103 unused-variable` and `SCR117 redefined-builtin` do not report the
name of such a handler: this rule explains the mistake.

## Fix

Adds the parentheses. Unsafe: the fixed handler catches a second
exception class and no longer rebinds the name, so the program behaves
differently (that is the point, but iglint cannot prove the author meant
the class). No fix when a comment sits between the two names.
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

/// The handlers `except A, B:` of a script where `B` names an exception
/// class: the type expression and the name. `SCR103` and `SCR117` stay
/// silent about these names; this rule explains them.
///
/// `B` names a class when, ignoring the handler itself, it resolves (in
/// the handler's scope, then the enclosing functions, the module and the
/// builtins) only to `class B` statements, to `from m import B` with a
/// capitalized name, or to a builtin exception. A name that is a module
/// (`import sys`), any other variable, or that the handler body reads
/// (`except ValueError, err: return str(err)`) is a variable.
pub(crate) fn misread_handlers<'s>(
    unit: &ScriptUnit<'s, '_>,
    model: &SemanticModel<'_>,
) -> Vec<(&'s Expr, &'s ExprName)> {
    let mut found = Vec::new();
    walk_stmts(&unit.parsed.module.body, &mut |s| {
        let Stmt::TryExcept(t) = s else { return };
        for h in &t.handlers {
            let (Some(ty), Some(name)) = (&h.type_, &h.name) else {
                continue;
            };
            let Expr::Name(n) = &**name else { continue };
            if names_exception_class(model, h, n) {
                found.push((&**ty, n));
            }
        }
    });
    found
}

fn names_exception_class(model: &SemanticModel<'_>, h: &ExceptHandler, n: &ExprName) -> bool {
    let Some(handler_binding) = model
        .bindings()
        .find(|(_, b)| b.kind == BindingKind::ExceptVar && b.range == n.range)
        .map(|(_, b)| b)
    else {
        return false;
    };
    // The body reads the caught exception under that name.
    let body = match (h.body.first(), h.body.last()) {
        (Some(first), Some(last)) => first.range().cover(last.range()),
        _ => return false,
    };
    if handler_binding
        .references
        .iter()
        .any(|r| body.contains_range(model.reference(*r).range))
    {
        return false;
    }
    let mut scope = Some(handler_binding.scope);
    while let Some(id) = scope {
        if model.is_dynamic_chain(id) {
            return false;
        }
        let others: Vec<_> = model
            .scope(id)
            .bindings_of(&n.id)
            .iter()
            .map(|b| model.binding(*b))
            .filter(|b| b.kind != BindingKind::ExceptVar)
            .collect();
        if !others.is_empty() {
            return others.iter().all(|b| match b.kind {
                BindingKind::ClassDef => true,
                BindingKind::FromImport => b.alias.is_some_and(|a| {
                    a.name
                        .as_str()
                        .starts_with(|c: char| c.is_ascii_uppercase())
                }),
                _ => false,
            });
        }
        // Other handlers binding the same name: still a variable there.
        if id != handler_binding.scope && !model.scope(id).bindings_of(&n.id).is_empty() {
            return false;
        }
        scope = model.enclosing_lookup_scope(id);
    }
    iglint_stubs::jython_builtins()
        .iter()
        .any(|b| b.name == &*n.id && b.kind == iglint_stubs::BuiltinKind::Exception)
}

/// The name ranges of [`misread_handlers`].
pub(crate) fn misread_handler_names(
    unit: &ScriptUnit<'_, '_>,
    model: &SemanticModel<'_>,
) -> FxHashSet<TextRange> {
    misread_handlers(unit, model)
        .into_iter()
        .map(|(_, n)| n.range)
        .collect()
}

impl ScriptRule for ExceptTuple {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let text = source(unit);
        let found: Vec<(TextRange, TextRange, String)> = misread_handlers(unit, model)
            .into_iter()
            .map(|(ty, n)| (ty.range().cover(n.range), n.range, n.id.to_string()))
            .collect();
        for (whole, name_range, name) in found {
            // A comment between the two names cannot move into parentheses
            // safely.
            let fix = (!text[whole.to_std()].contains('#'))
                .then(|| {
                    let replacement = format!("({})", &text[whole.to_std()]);
                    fix(
                        cx,
                        unit,
                        "Add parentheses",
                        Applicability::Unsafe,
                        vec![(whole, replacement)],
                    )
                })
                .flatten();
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
        let fixed = f.fixed(&[("SCR115", "")], true, &library_path(&f, "m"));
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
        let fixed = f.fixed(&[("SCR115", "")], true, &view_path(&f));
        assert!(
            fixed.contains(r"\texcept (KeyError, IndexError):"),
            "{fixed}"
        );
    }

    #[test]
    fn comment_between_names_reports_without_fix() {
        let d = lint_module(
            "SCR115",
            "try:\n    f()\nexcept (ValueError # first\n        ), TypeError:\n    pass\n",
        );
        assert_eq!(flagged(&d), ["TypeError"]);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn unused_variable_and_redefined_builtin_stay_silent() {
        let source = "def f():\n    try:\n        g()\n    except ValueError, TypeError:\n        pass\n    try:\n        g()\n    except ValueError, e:\n        pass\n";
        let d = lint_module("SCR115", source);
        assert_eq!(flagged(&d), ["TypeError"]);
        // `e` is still an unused variable; `TypeError` is left to SCR115.
        assert_eq!(flagged(&lint_module("SCR103", source)), ["e"]);
        assert!(lint_module("SCR117", source).is_empty());
        let module = "try:\n    g()\nexcept ValueError, IOError:\n    pass\nlist = 1\n";
        assert_eq!(flagged(&lint_module("SCR117", module)), ["list"]);
    }

    /// Jython prints `bad` for this program; with parentheses it would
    /// print `<module 'sys'>`. `sys` is a variable here.
    #[test]
    fn modules_and_read_names_are_variables() {
        let code = "import sys\n\ndef f():\n    try:\n        raise ValueError('bad')\n    except ValueError, sys:\n        return str(sys)\n\nprint f()\n";
        assert!(lint_module("SCR115", code).is_empty());
        // Read in the handler body: a variable even with a builtin's name.
        assert!(
            lint_module(
                "SCR115",
                "try:\n    f()\nexcept ValueError, TypeError:\n    print TypeError\n"
            )
            .is_empty()
        );
        // A lowercase imported name, a rebound builtin, a dynamic scope.
        for code in [
            "from m import err\ntry:\n    f()\nexcept ValueError, err:\n    pass\n",
            "TypeError = 1\ntry:\n    f()\nexcept ValueError, TypeError:\n    pass\n",
            "from m import *\ntry:\n    f()\nexcept ValueError, TypeError:\n    pass\n",
            "def g(TypeError):\n    try:\n        f()\n    except ValueError, TypeError:\n        pass\n",
        ] {
            assert!(lint_module("SCR115", code).is_empty(), "{code}");
        }
        // Classes defined in the module, and capitalized from-imports.
        let d = lint_module(
            "SCR115",
            "class Oops(Exception):\n    pass\ndef g():\n    try:\n        f()\n    except ValueError, Oops:\n        pass\n",
        );
        assert_eq!(flagged(&d), ["Oops"]);
    }

    #[test]
    fn fix_is_unsafe() {
        let d = lint_module(
            "SCR115",
            "try:\n    f()\nexcept ValueError, TypeError:\n    pass\n",
        );
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::Applicability::Unsafe)
        );
    }
}
