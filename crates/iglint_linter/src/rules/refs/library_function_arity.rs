//! REF003 library-function-arity.

use iglint_project::library::{CallMismatch, LibraryLookup, SymbolKind};

use super::library_uses::library_uses;
use super::unknown_library_member::resolve_for_findings;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};

pub static META: RuleMeta = RuleMeta {
    code: "REF003",
    name: "library-function-arity",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Call to a library function with arguments it cannot accept",
    explanation: r#"## What it does

Checks calls of project library functions (`core.util.fmt(...)`, through
aliases and `from core.util import fmt`, and a module's calls of its own
top-level functions) against the function's signature: too few or too
many positional arguments, unknown keyword arguments and arguments passed
twice.

Only plain functions are checked: a name defined more than once in its
module (conditional definitions, redefinitions, `global` assignments), a
decorated function, a class or any other value is not. Neither is a
function its module may replace at run time (the module runs `exec`,
`eval` or `execfile`, writes `globals()` or `sys.modules[__name__]`, sets
the function's `func_code` or `func_defaults`, or a later
`from x import *` may overwrite it), nor one that a script of the project
assigns, deletes or replaces (`core.util.fmt = f`,
`setattr(core.util, 'fmt', f)`, `core.util.__dict__[k] = v`). Calls that unpack
`*args` or `**kwargs` are only checked for errors the unpacking cannot
hide. Calls inside a `try` that catches `TypeError` or `Exception` are
checked too: a wrong call is a bug, not a handled condition.

## Why is this bad?

The call raises `TypeError` at run time. A wrong argument count usually
means the function's signature changed and this caller was not updated.

## Example

```python
# core/util: def fmt(value, digits=2)
core.util.fmt(value, 2, 'kW')
```

Use instead:

```python
core.util.fmt(value, 2)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&LibraryFunctionArity)],
    options: None,
};

struct LibraryFunctionArity;

fn arguments(n: usize) -> String {
    crate::rules::helpers::plural(n, "argument", "arguments")
}

impl ScriptRule for LibraryFunctionArity {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(uses) = library_uses(cx, unit) else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        for u in uses {
            let Some(call) = u.call else {
                continue;
            };
            let LibraryLookup::Symbol {
                symbol, rest: 0, ..
            } = resolve_for_findings(index, project, &u.dotted)
            else {
                continue;
            };
            let SymbolKind::Function(signature) = &symbol.kind else {
                continue;
            };
            let keywords: Vec<&str> = call.keywords.iter().map(|k| k.arg.as_str()).collect();
            let Err(mismatch) = signature.check_call(
                call.args.len(),
                &keywords,
                call.starargs.is_some(),
                call.kwargs.is_some(),
            ) else {
                continue;
            };
            let name = &u.dotted;
            let message = match mismatch {
                CallMismatch::TooFewArguments { required, given } => format!(
                    "`{name}` requires {}, the call passes {given}",
                    arguments(required)
                ),
                CallMismatch::TooManyArguments { max, given } => format!(
                    "`{name}` accepts at most {} positionally, the call passes {given}",
                    arguments(max)
                ),
                CallMismatch::UnknownKeyword(k) => {
                    format!("`{name}` has no parameter `{k}`")
                }
                CallMismatch::DuplicateArgument(k) => {
                    format!("The call passes `{k}` of `{name}` twice")
                }
            };
            let range = iglint_text::Ranged::range(&*call.func);
            crate::rules::script::report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{
        ViewScript, decoded_flagged, library, messages, view, with_library,
    };

    const UTIL: &str = "def fmt(value, digits=2):\n    return value\ndef anything(*a, **k):\n    pass\ndef kw(a, **k):\n    pass\nif X:\n    def twice(a): pass\nelse:\n    def twice(a, b): pass\n@deco\ndef wrapped(a):\n    pass\nclass K(object):\n    def __init__(self, a): pass\n";

    #[test]
    fn wrong_calls() {
        let f = with_library(
            library(
                "app/main",
                "from core.util import fmt as f\ndef go(args, opts):\n    core.util.fmt()\n    core.util.fmt(1)\n    core.util.fmt(1, 2, 3)\n    f(1, digit=2)\n    f(1, value=2)\n    core.util.fmt(*args)\n    core.util.fmt(1, 2, *args)\n    core.util.anything(1, 2, x=3)\n    core.util.kw(1, x=2)\n    core.util.twice(1, 2, 3)\n    core.util.wrapped()\n    core.util.K()\n    core.util.fmt(value=1, **opts)\n",
            ),
            "core/util",
            UTIL,
        );
        let d = f.lint(&["REF003"]);
        assert_eq!(
            messages(&d),
            vec![
                "`core.util.fmt` requires 1 argument, the call passes 0",
                "`core.util.fmt` accepts at most 2 arguments positionally, the call passes 3",
                "`core.util.fmt` has no parameter `digit`",
                "The call passes `value` of `core.util.fmt` twice",
            ]
        );
    }

    #[test]
    fn own_module_and_view_scripts() {
        let f = library(
            "core/util",
            "def fmt(value):\n    return value\ndef go():\n    fmt()\n    fmt(1)\n",
        );
        assert_eq!(
            messages(&f.lint(&["REF003"])),
            vec!["`core.util.fmt` requires 1 argument, the call passes 0"]
        );
        let f = with_library(
            view(ViewScript::Transform, "\treturn core.util.fmt(value, 1, 2)"),
            "core/util",
            UTIL,
        );
        assert_eq!(decoded_flagged(&f.lint(&["REF003"])), vec!["core.util.fmt"]);
    }

    /// Calls of a function that may be another function at run time are
    /// not checked (verified on Jython: each rebinding changes what
    /// `fmt()` accepts).
    #[test]
    fn rebound_functions_are_not_checked() {
        for (util, other) in [
            ("exec 'def fmt(): pass'\n", ""),
            ("globals()['fmt'] = lambda: 1\n", ""),
            ("fmt.func_defaults = (1,)\n", ""),
            ("", "core.util.fmt = lambda: 1\n"),
            ("", "setattr(core.util, 'fmt', lambda: 1)\n"),
            ("", "m = core.util\nm.fmt = lambda: 1\n"),
        ] {
            let f = library(
                "core/util",
                &format!("def fmt(value):\n    return value\n{util}"),
            );
            let f = with_library(f, "app/other", other);
            let f = with_library(f, "app/main", "def go():\n    core.util.fmt()\n");
            assert!(f.lint(&["REF003"]).is_empty(), "{util}{other}");
        }
        // A store to another name of the module changes nothing.
        let f = library(
            "core/util",
            "def fmt(value):\n    return value\nCACHE = {}\n",
        );
        let f = with_library(f, "app/other", "core.util.CACHE = {}\n");
        let f = with_library(f, "app/main", "def go():\n    core.util.fmt()\n");
        assert_eq!(f.lint(&["REF003"]).len(), 1);
    }
}
