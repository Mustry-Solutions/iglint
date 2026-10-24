//! SCR118 used-before-assignment.

use iglint_python_semantic::{ReferenceKind, Resolution, ScopeId, ScopeKind};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR118",
    name: "used-before-assignment",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Variable is read where no assignment can have happened",
    explanation: r#"## What it does

Reports reads of a function's local variable (or a module-level name in
module code) that no assignment reaches on any path: it is read before its
first assignment, after `del`, or only assigned in a branch that cannot
run. The analysis follows `if`/`else`, loops (a later iteration sees
earlier assignments), `try`/`except`/`else`/`finally`, `with`, `break`,
`continue`, `return` and `raise`. A variable that is assigned on at least
one path is never reported: the rule only reports reads that fail every
time they run.

A name is local to a function when the function assigns it anywhere, even
after the read; then a module-level variable of the same name is not
visible in the function (Python raises `UnboundLocalError`).

Unreachable reads, scopes with `exec` or `from x import *`, and reads in a
`try` that catches `NameError` are not reported. Module-level reads fall
back to builtins and to names that functions bind with `global`.

## Why is this bad?

The read raises `UnboundLocalError` (functions) or `NameError` (module
code) every time it runs.

## Example

```python
COUNTER = 0

def onMessageReceived(self, payload):
    COUNTER += 1
    self.custom.count = COUNTER
```

Use instead:

```python
def onMessageReceived(self, payload):
    self.custom.count = (self.custom.count or 0) + 1
```

## pylint

Corresponds to `E0601 used-before-assignment` (definite cases only;
pylint's `possibly-used-before-assignment` is not reported). pylint misses
the augmented assignment case (`N += 1` in a function where `N` is a
module variable), which always raises `UnboundLocalError`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UsedBeforeAssignment)],
    options: None,
};

struct UsedBeforeAssignment;

impl ScriptRule for UsedBeforeAssignment {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let module_dynamic = model.scope(ScopeId::MODULE).is_dynamic();
        let mut found = Vec::new();
        for (_, r) in model.references() {
            if !r.unbound
                || !r.reachable
                || r.name_error_guarded
                || r.is_implicit()
                || r.resolution != Resolution::Local
            {
                continue;
            }
            let scope = model.scope(r.scope);
            let applies = match scope.kind {
                ScopeKind::Function | ScopeKind::Lambda | ScopeKind::Generator => true,
                ScopeKind::Module => !module_dynamic,
                ScopeKind::Class => false,
            };
            if !applies || scope.is_dynamic() || model.scope(ScopeId::MODULE).writes_globals {
                continue;
            }
            let assigned = !scope.bindings_of(r.name).is_empty();
            let global_exists = scope.kind != ScopeKind::Module
                && !model.scope(ScopeId::MODULE).bindings_of(r.name).is_empty();
            let mut message = match (r.kind, assigned) {
                (ReferenceKind::Del, _) => format!("`{}` is deleted before it is assigned", r.name),
                (_, true) => format!("`{}` is used before it is assigned", r.name),
                (_, false) => format!("`{}` is used but never assigned", r.name),
            };
            if global_exists {
                message.push_str(&format!(
                    "; it is local because the function assigns it, use `global {}` to mean the module variable",
                    r.name
                ));
            }
            found.push((r.range, message));
        }
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn definite_cases() {
        let d = lint_module("SCR118", "def f():\n    print x\n    x = 1\n");
        assert_eq!(flagged(&d), ["x"]);
        assert_eq!(messages(&d), ["`x` is used before it is assigned"]);
        let d = lint_module("SCR118", "N = 0\ndef f():\n    N += 1\n");
        assert_eq!(flagged(&d), ["N"]);
        assert!(d[0].message.contains("global N"));
        let d = lint_module("SCR118", "def f():\n    x = 1\n    del x\n    return x\n");
        assert_eq!(flagged(&d), ["x"]);
    }

    /// Review round 22 (Opus) N2: reads that only a `del` reaches. Each
    /// reported case raises `UnboundLocalError` on Ignition 8.3.3's Jython
    /// for every argument; the others return on some path.
    #[test]
    fn reads_after_del() {
        for code in [
            "def f2():\n\tv = 1\n\tdel v\n\treturn v\n",
            "def f():\n    for i in range(2):\n        v = 1\n        del v\n    return v\n",
            "def f(c):\n    v = 1\n    while c:\n        del v\n        v = 2\n        c = False\n    del v\n    return v\n",
            "def f(c):\n    v = 1\n    if c:\n        del v\n    else:\n        del v\n    return v\n",
        ] {
            assert_eq!(flagged(&lint_module("SCR118", code)), ["v"], "{code}");
        }
        for code in [
            "def f(c):\n    v = 1\n    if c:\n        del v\n    return v\n",
            "def f():\n    v = 1\n    del v\n    v = 2\n    return v\n",
            "def f():\n    v = 1\n    del v\n    try:\n        return v\n    except NameError:\n        return 0\n",
        ] {
            assert!(lint_module("SCR118", code).is_empty(), "{code}");
        }
    }

    #[test]
    fn possible_assignments_are_not_reported() {
        let cases = [
            "def f(c):\n    if c:\n        x = 1\n    return x\n",
            "def f(xs):\n    for i in xs:\n        if i:\n            print prev\n        prev = i\n",
            "def f():\n    while True:\n        try:\n            x = g()\n            break\n        except Exception:\n            continue\n    return x\n",
            "def f():\n    try:\n        x = g()\n    except ValueError:\n        return\n    return x\n",
            "def f():\n    try:\n        x = g()\n    finally:\n        print x\n",
            "def f():\n    with g() as h:\n        x = 1\n    return x\n",
            "def f(y):\n    [x for x in y]\n    return x\n",
            "def f():\n    exec 'x = 1'\n    return x\n",
            "def init():\n    global X\n    X = 1\ninit()\nprint X\n",
            "print len\nlen = 2\n",
            "class C:\n    x = y\n    y = 1\n",
        ];
        for code in cases {
            assert!(lint_module("SCR118", code).is_empty(), "{code}");
        }
    }

    #[test]
    fn module_level() {
        let d = lint_module("SCR118", "print y\ny = 5\n");
        assert_eq!(flagged(&d), ["y"]);
    }

    #[test]
    fn view_scripts() {
        let d = view(
            ViewScript::Change,
            "\tif currentValue.value:\n\t\tself.custom.a \u{3d} total\n\ttotal \u{3d} 1",
        )
        .lint(&["SCR118"]);
        assert_eq!(decoded_flagged(&d), ["total"]);
    }
}
