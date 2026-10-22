//! SCR002 compile-error.

use iglint_python_semantic::compile_errors;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR002",
    name: "compile-error",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Script parses but Jython refuses to compile it",
    explanation: r#"## What it does

Reports code that Jython 2.7's parser accepts but its compiler rejects,
with Jython's message:

* `return` or `yield` outside a function (also in a class body);
* `break` outside a loop, `continue` outside a loop (a loop's `else` clause
  is outside it);
* a duplicate parameter name (`def f(a, a)`, also inside tuple parameters);
* `return value` (even `return None`) in a generator;
* `import *` in a function that is nested or contains nested functions with
  free variables, unqualified `exec` in a function with free variables;
* a bare `except:` that is not the last handler;
* `global x` where `x` is a parameter;
* `del x` where `x` is used by a nested function;
* assigning to `__debug__`;
* `from __future__ import ...` that is not at the top of a module (in
  Perspective and event scripts it never is: the script is the body of a
  generated function), or names an unknown feature;
* assignment targets Jython's parser accepts but its compiler crashes on
  (`-a = 1`, `for f() in x:`, `except E, a + b:`).

Every case was checked against Ignition's Jython with full compilation;
iglint reports nothing that Jython compiles. A method too large for the
JVM (`Method code too large!`, very long modules) is not detected.

## Why is this bad?

The script never runs. In a library module every script that imports it
fails.

## Example

```python
def onMessageReceived(self, payload):
    for row in payload["rows"]:
        process(row)
    else:
        continue
```

Use instead:

```python
def onMessageReceived(self, payload):
    for row in payload["rows"]:
        process(row)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&CompileError)],
    options: None,
};

struct CompileError;

impl ScriptRule for CompileError {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        for error in compile_errors(model) {
            report(
                cx,
                unit,
                error.range,
                format!("SyntaxError: {}", error.message),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn module_level_errors() {
        let d = lint_module("SCR002", "return 1\nbreak\n");
        assert_eq!(flagged(&d), ["return 1", "break"]);
        assert_eq!(
            messages(&d),
            [
                "SyntaxError: 'return' outside function",
                "SyntaxError: 'break' outside loop"
            ]
        );
    }

    #[test]
    fn valid_code_is_clean() {
        let code = "def f(a, *a):\n    yield 1\n    return\nclass C:\n    def m(self):\n        from os import *\n        return join\n";
        assert!(lint_module("SCR002", code).is_empty());
    }

    #[test]
    fn view_scripts_are_function_bodies() {
        // `return` is fine in a Perspective script, a future import is not.
        assert!(
            view(ViewScript::Action, "\treturn")
                .lint(&["SCR002"])
                .is_empty()
        );
        let d = view(
            ViewScript::Action,
            "\tfrom __future__ import division\n\tx = 1 / 2",
        )
        .lint(&["SCR002"]);
        assert_eq!(decoded_flagged(&d), ["from __future__ import division"]);
        let d = view(
            ViewScript::Transform,
            "\tfor v in value:\n\t\tpass\n\telse:\n\t\tbreak\n\treturn value",
        )
        .lint(&["SCR002"]);
        assert_eq!(decoded_flagged(&d), ["break"]);
    }

    #[test]
    fn generator_return_and_duplicate_arguments() {
        let d = lint_module(
            "SCR002",
            "def g(a, (b, a)):\n    yield a\n    return None\n",
        );
        assert_eq!(flagged(&d), ["a", "return None"]);
    }

    #[test]
    fn invalid_targets() {
        let d = lint_module("SCR002", "for f() in x:\n    pass\n");
        assert_eq!(flagged(&d), ["f()"]);
        assert!(d[0].message.contains("internal error"));
    }

    #[test]
    fn broken_scripts_are_left_to_scr001() {
        assert!(lint_module("SCR002", "return 1\nif x\n").is_empty());
    }
}
