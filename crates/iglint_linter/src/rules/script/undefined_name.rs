//! SCR101 undefined-name.

use iglint_python_semantic::{ReferenceKind, Resolution};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, source};

pub static META: RuleMeta = RuleMeta {
    code: "SCR101",
    name: "undefined-name",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Name is not defined anywhere the script can see",
    explanation: r#"## What it does

Reports names that are bound nowhere the code can see them: not in the
function, not in an enclosing function, not at module level, not a Jython
builtin and not one of the names Ignition provides to the script.

Ignition provides, without import (verified on an 8.3 gateway):
`system`, `fpmi` (legacy alias of `system`), `sys`, `__builtins__`, the
generated function's own name, and every top-level package or module of
the project's script library (`core` for `core/util/code.py`), including
those of child projects when the script belongs to an inheritable project.
Library modules also have `__name__` (and `__package__` once an import has
run), but **not** `__file__`.

The scoping rules are Python 2.7's: list comprehension variables leak into
the enclosing scope, generator expressions do not; names bound in a class
body are not visible to its methods.

The rule stays silent where it cannot be sure: in scopes with `from x
import *` or unqualified `exec`; in a module that writes its own namespace
(`exec code in globals()`, also `in d, globals()`, `globals()['x'] = ...`,
`globals().update(...)`, `setattr(sys.modules[__name__], ...)`,
`sys.modules[__name__].x = ...`) or the builtins (`__builtins__['x'] = ...`,
`setattr(__builtin__, ...)`, `__builtin__.x = ...`); in a function that runs
`exec code in locals()` (or `in vars()`, `in globals(), locals()`), whose
names Jython looks up at run time; inside a `try`
whose handlers catch `NameError`, in projects whose parent project is not
part of the linted tree (its library is unknown), and for the 7.x prefixes
`project.`, `shared.` and `app.` (reported by `SCR206`).

Not checked at all: a `.py` file outside any project (no `project.json` in
its directory or a parent directory, such as a loose file linted on its
own) and the tag scripts of gateway resources. Any name they do not
bind could be a top-level package of the script library they run with,
which iglint does not know there, so no name is reported, not only names
that look like packages. Lint the file as part of its project to check it.

## Why is this bad?

The script raises `NameError` when it reaches the name. In a Perspective
event that means the action silently does nothing but log an error; in a
transform the binding gets an error quality.

## Example

```python
def runAction(self, event):
    rows = system.db.runNamedQuery("Lines/All")
    for row in rows:
        lineName = row["name"]
    self.view.custom.last = linename
```

Use instead:

```python
def runAction(self, event):
    rows = system.db.runNamedQuery("Lines/All")
    for row in rows:
        lineName = row["name"]
    self.view.custom.last = lineName
```

## pylint

Corresponds to `E0602 undefined-variable`. Differences, all because pylint
assumes CPython 3: pylint reports `system`, the project's library packages
and the header parameters of Perspective scripts (iglint knows them), and
reports list comprehension variables used after the comprehension (they
leak in Jython 2.7). pylint does not report `__file__` in library modules,
which Ignition does not define.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UndefinedName)],
    options: None,
};

struct UndefinedName;

/// Builtins that exist in Python 3 but not in Jython 2.7.
const PYTHON3_ONLY: &[&str] = &[
    "ascii",
    "breakpoint",
    "BlockingIOError",
    "BrokenPipeError",
    "ChildProcessError",
    "ConnectionAbortedError",
    "ConnectionError",
    "ConnectionRefusedError",
    "ConnectionResetError",
    "FileExistsError",
    "FileNotFoundError",
    "InterruptedError",
    "IsADirectoryError",
    "ModuleNotFoundError",
    "NotADirectoryError",
    "PermissionError",
    "ProcessLookupError",
    "RecursionError",
    "ResourceWarning",
    "StopAsyncIteration",
    "TimeoutError",
];

impl ScriptRule for UndefinedName {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let text = source(unit);
        for (_, r) in model.references() {
            if r.resolution != Resolution::Unresolved || r.name_error_guarded || r.is_implicit() {
                continue;
            }
            if matches!(r.name, "project" | "shared" | "app")
                && text
                    .get(r.range.end().to_usize()..)
                    .is_some_and(|rest| rest.starts_with('.'))
            {
                continue;
            }
            let mut message = match r.kind {
                ReferenceKind::Del => format!("Undefined name `{}` in `del`", r.source_name),
                _ => format!("Undefined name `{}`", r.source_name),
            };
            if r.name != r.source_name {
                // A private name in a class body is looked up mangled.
                message.push_str(&format!(" (looked up as `{}`)", r.name));
            }
            if PYTHON3_ONLY.contains(&r.name) {
                message.push_str(" (a Python 3 builtin; Jython 2.7 does not have it)");
            }
            report(cx, unit, r.range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_unbound_names() {
        let d = lint_module("SCR101", "def f():\n    return undefinedThing + len([])\n");
        assert_eq!(flagged(&d), ["undefinedThing"]);
        assert_eq!(messages(&d), ["Undefined name `undefinedThing`"]);
    }

    #[test]
    fn ignition_globals_are_defined() {
        let code = "def f():\n    system.tag.readBlocking(['a'])\n    fpmi.gui.x\n    sys.exit\n    __builtins__\n    return core.util.g()\n";
        let f = with_library(library("m", code), "core/util", "def g(): pass\n");
        assert!(f.lint(&["SCR101"]).is_empty());
        // `__file__` is not defined in library modules.
        assert_eq!(
            flagged(&lint_module("SCR101", "x = __file__\n")),
            ["__file__"]
        );
        // `__package__` exists once an import ran.
        assert!(lint_module("SCR101", "import os\nx = __package__\n").is_empty());
        // Without an import that binds it, `__package__` is undefined in a
        // library module of a known project (measured: `NameError`).
        let d = crate::rules::script::testing::library("m", "x = __package__\n").lint(&["SCR101"]);
        assert_eq!(flagged(&d), ["__package__"]);
        assert!(
            crate::rules::script::testing::library("m", "import os\nx = __package__\n")
                .lint(&["SCR101"])
                .is_empty()
        );
    }

    #[test]
    fn header_parameters_are_defined_per_location() {
        assert!(
            view(ViewScript::Action, "\tself.props.text = event.value")
                .lint(&["SCR101"])
                .is_empty()
        );
        assert!(
            view(ViewScript::Transform, "\treturn value, quality, timestamp")
                .lint(&["SCR101"])
                .is_empty()
        );
        assert!(
            view(
                ViewScript::Change,
                "\tx = previousValue, currentValue, origin, missedEvents"
            )
            .lint(&["SCR101"])
            .is_empty()
        );
        assert!(
            view(ViewScript::Handler, "\tself.x = payload")
                .lint(&["SCR101"])
                .is_empty()
        );
        assert!(
            view(ViewScript::Method, "\treturn a + b")
                .lint(&["SCR101"])
                .is_empty()
        );
        // `event` is not a parameter of a transform.
        let d = view(ViewScript::Transform, "\treturn event").lint(&["SCR101"]);
        assert_eq!(decoded_flagged(&d), ["event"]);
    }

    #[test]
    fn event_files_use_the_canonical_parameters() {
        // Ignition replaces the written header: `extra` does not exist.
        let d =
            gateway_message("def handleMessage(payload=None, extra=5):\n\treturn payload, extra\n")
                .lint(&["SCR101"]);
        assert_eq!(flagged(&d), ["extra"]);
        assert!(
            gateway_message("def handleMessage(payload):\n\thandleMessage\n")
                .lint(&["SCR101"])
                .is_empty()
        );
    }

    #[test]
    fn python27_scoping() {
        assert!(lint_module("SCR101", "def f(y):\n    [x for x in y]\n    return x\n").is_empty());
        assert_eq!(
            flagged(&lint_module(
                "SCR101",
                "def f(y):\n    list(x for x in y)\n    return x\n"
            )),
            ["x"]
        );
        assert_eq!(
            flagged(&lint_module(
                "SCR101",
                "class C:\n    k = 1\n    def m(self):\n        return k\n"
            )),
            ["k"]
        );
    }

    #[test]
    fn silent_where_names_may_exist() {
        assert!(lint_module("SCR101", "from os.path import *\nprint join\n").is_empty());
        assert!(
            lint_module(
                "SCR101",
                "try:\n    x = unicode_thing\nexcept NameError:\n    x = None\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR101", "x = project.util.f()\ny = shared.a\n").is_empty());
        assert!(lint_module("SCR101", "globals()['dyn'] = 1\nprint dyn\n").is_empty());
    }

    #[test]
    fn python3_builtins_get_a_hint() {
        let d = lint_module(
            "SCR101",
            "try:\n    pass\nexcept FileNotFoundError:\n    pass\n",
        );
        assert!(d[0].message.contains("Python 3"));
    }

    #[test]
    fn inherited_libraries() {
        // A child project sees its parent's library packages.
        let f = crate::rules::test_support::Fixture::new()
            .file(
                "projects/base/project.json",
                r#"{"title": "base", "enabled": true, "inheritable": true}"#,
            )
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "enabled": true, "inheritable": false, "parent": "base"}"#,
            )
            .resource(
                "projects/base/ignition/script-python/shared_lib",
                &[("code.py", "def g(): pass\n")],
            )
            .resource(
                "projects/p/ignition/script-python/m",
                &[("code.py", "shared_lib.g()\n")],
            );
        assert!(f.lint(&["SCR101"]).is_empty());
        // A parent outside the linted tree: its library is unknown.
        let f = crate::rules::test_support::Fixture::new()
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "enabled": true, "inheritable": false, "parent": "missing"}"#,
            )
            .resource(
                "projects/p/ignition/script-python/m",
                &[("code.py", "unknown_lib.g()\n")],
            );
        assert!(f.lint(&["SCR101"]).is_empty());
    }

    /// Review round 13: a class body looks private names up mangled.
    #[test]
    fn private_names_in_class_bodies_are_mangled() {
        let code = "from math import pi as _D__pi\n\nclass D(object):\n    x = __pi\n";
        assert!(lint_module("SCR101", code).is_empty());
        let d = lint_module("SCR101", "class D(object):\n    x = __pi\n");
        assert_eq!(flagged(&d), ["__pi"]);
        assert_eq!(
            messages(&d),
            ["Undefined name `__pi` (looked up as `_D__pi`)"]
        );
        // Methods do not mangle names; attributes are mangled everywhere
        // in the class, which is not a name lookup.
        let code =
            "_D__pi = 1\n\nclass D(object):\n    def f(self):\n        return __pi, self.__pi\n";
        assert_eq!(flagged(&lint_module("SCR101", code)), ["__pi"]);
        // The names the interpreter reads itself are never reported.
        assert!(lint_module("SCR101", "class C:\n    pass\nimport os\nassert os\n").is_empty());
    }
}
