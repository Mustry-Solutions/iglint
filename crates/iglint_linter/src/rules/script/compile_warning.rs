//! SCR003 compile-warning.

use iglint_python_semantic::{compile_errors, compile_warnings};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR003",
    name: "compile-warning",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Jython compiles the script with a `SyntaxWarning`",
    explanation: r#"## What it does

Reports the code for which Jython 2.7's compiler issues a
`SyntaxWarning`, with Jython's message. Verified against Ignition's Jython
with full compilation, these are all of them:

* `name 'x' declared global after assignment`: a `global` statement names
  a name that the same function, class body or module binds earlier in
  the source (an assignment, augmented assignment, `del`, `import`,
  `def`, `class`, `for`, `with` or `except` target, a list comprehension
  variable), whatever the control flow;
* `name 'x' declared global after use`: otherwise, the same scope reads
  the name earlier (also as the object of an attribute or item
  assignment, and in decorators, default values, base classes and the
  first iterable of a generator expression). Uses in nested functions and
  classes, and uses after an earlier `global` of the name, do not count;
* `import * only allowed at module level`: `from m import *` in a
  function or a method, or in a class inside a function. In Perspective
  and event scripts every `import *` is in a function (the script is the
  body of a generated function).

Where Jython refuses to compile the code anyway (`SCR002`), only the error
is reported.

## Why is this bad?

The `global` statement applies to the whole function, also to the code
before it: the earlier assignment or read already used the global
variable, which is rarely what the order suggests. `import *` in a
function makes its names unknown to the compiler; Jython then looks every
name up by name at run time, and nested functions that use free variables
make it an error (`SCR002`). The warnings also end up in the gateway or
client log each time the script is compiled.

## Example

```python
def resetCounter():
    counter = 0
    global counter
```

Use instead:

```python
def resetCounter():
    global counter
    counter = 0
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&CompileWarning)],
    options: None,
};

struct CompileWarning;

impl ScriptRule for CompileWarning {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let errors = compile_errors(model);
        for warning in compile_warnings(model) {
            // Jython refuses the code at the same place: SCR002 reports it.
            if errors.iter().any(|e| {
                e.range.contains_range(warning.range) || warning.range.contains_range(e.range)
            }) {
                continue;
            }
            report(
                cx,
                unit,
                warning.range,
                format!("SyntaxWarning: {}", warning.message),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn global_after_assignment_or_use() {
        let d = lint_module(
            "SCR003",
            "def f():\n    x = 1\n    global x\ndef g():\n    print y\n    global y\n",
        );
        assert_eq!(flagged(&d), ["x", "y"]);
        assert_eq!(
            messages(&d),
            [
                "SyntaxWarning: name 'x' declared global after assignment",
                "SyntaxWarning: name 'y' declared global after use"
            ]
        );
        // Order matters, not control flow; nested scopes do not count.
        assert!(lint_module("SCR003", "def f():\n    global x\n    x = 1\n").is_empty());
        assert!(
            lint_module(
                "SCR003",
                "def f():\n    def g():\n        print x\n    global x\n"
            )
            .is_empty()
        );
        assert_eq!(
            lint_module(
                "SCR003",
                "def f():\n    if a:\n        x = 1\n    else:\n        global x\n"
            )
            .len(),
            1
        );
    }

    #[test]
    fn import_star_below_module_level() {
        let d = lint_module("SCR003", "def f():\n    from os import *\n    return sep\n");
        assert_eq!(flagged(&d), ["from os import *"]);
        assert!(lint_module("SCR003", "class C:\n    from os import *\n").is_empty());
        // Jython refuses it in a nested function: SCR002 alone.
        assert!(
            lint_module(
                "SCR003",
                "def f():\n    def g():\n        from os import *\n"
            )
            .is_empty()
        );
        // No code is generated after `return`.
        assert!(lint_module("SCR003", "def f():\n    return\n    from os import *\n").is_empty());
    }

    #[test]
    fn view_scripts_are_function_bodies() {
        let d = view(ViewScript::Action, "\tfrom os import *\n\tprint sep").lint(&["SCR003"]);
        assert_eq!(decoded_flagged(&d), ["from os import *"]);
        let d = view(ViewScript::Action, "\tcount = 1\n\tglobal count").lint(&["SCR003"]);
        assert_eq!(decoded_flagged(&d), ["count"]);
    }
}
