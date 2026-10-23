//! SCR002 compile-error.

use iglint_python_semantic::{compile_errors, mark_limit_error, size_errors};

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

It also reports scripts that are too large for Jython:

* the first two lines together are 100,000 characters or longer (UTF-16
  code units, line ends included; in Perspective and event scripts the
  first line is the generated `def` header). Jython reads them ahead to
  look for an encoding declaration and fails with
  `java.io.IOException: Mark invalid`. Longer lines further down are fine;
* a module, class or function body with more than 10,922 lines that start
  a statement (`Module or method too large`: one body is one Java method of
  at most 64 KiB). Jython already fails at about 3,000 to 9,400 lines,
  depending on the statements; iglint only reports sizes that cannot
  compile whatever the statements are;
* a module with more than 32,767 distinct string literals (`Class too
  large`: one module is one Java class, with at most 65,535 constants).

Jython generates no code for the rest of a block after `return`, `raise`,
`continue` or an `if`/`else` whose every branch ends that way (but not
after `break`, a loop, `try` or `with`, and not for the module's own
statements). That dead code counts for neither size limit, and the errors
above that Jython's code generator raises (`return`, `yield`, `break` and
`continue` in the wrong place, `import *`, a bare `except:` that is not
last, `del` of a variable a nested function uses, invalid targets) are not
reported in it.

Every case was checked against Ignition's Jython with full compilation;
iglint reports nothing that Jython compiles.

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
        let source = &unit.script.source;
        // Jython fails while reading, before it parses.
        if let Some(error) = mark_limit_error(source) {
            report(cx, unit, error.range, error.message);
        }
        if unit.parsed.errors.is_empty() {
            for error in size_errors(&unit.parsed.module, source) {
                report(cx, unit, error.range, error.message);
            }
        }
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
    fn size_limits() {
        // The first two lines, measured as Jython reads them.
        let long = format!("#{}\nx=1\n", "a".repeat(99_994));
        assert!(lint_module("SCR002", &long).is_empty());
        let too_long = format!("#{}\nx=1\n", "a".repeat(99_995));
        let d = lint_module("SCR002", &too_long);
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("Mark invalid"), "{}", d[0].message);
        // Further down any length compiles.
        let later = format!("x = 1\ny = 2\nz = '{}'\n", "a".repeat(300_000));
        assert!(lint_module("SCR002", &later).is_empty());
        // In a view script the header is line 1.
        let body = format!("\t#{}\n\tpass", "b".repeat(99_969));
        assert!(view(ViewScript::Action, &body).lint(&["SCR002"]).is_empty());
        let body = format!("\t#{}\n\tpass", "b".repeat(99_970));
        assert_eq!(view(ViewScript::Action, &body).lint(&["SCR002"]).len(), 1);
        // A body too large for one Java method.
        let d = lint_module(
            "SCR002",
            &format!("def f():\n{}", "    pass\n".repeat(11_000)),
        );
        assert_eq!(flagged(&d), ["f"]);
        assert!(d[0].message.contains("too large"));
        assert!(lint_module("SCR002", &"x = 1\n".repeat(3_000)).is_empty());
        // Dead code generates nothing (Jython compiles these).
        let dead = format!("def f():\n    return\n{}", "    pass\n".repeat(12_000));
        assert!(lint_module("SCR002", &dead).is_empty());
        let body = format!("\treturn\n{}", "\tpass\n".repeat(12_000));
        assert!(view(ViewScript::Action, &body).lint(&["SCR002"]).is_empty());
    }

    /// Verified on Jython: its code generator never sees dead code.
    #[test]
    fn dead_code_compiles() {
        for dead in [
            "def f():\n    return\n    break\n",
            "def f():\n    raise E\n    continue\n",
            "def f():\n    if x:\n        return\n    else:\n        raise E\n    break\n",
            "class C:\n    raise E\n    return\n",
            "if x:\n    raise E\n    yield 1\n",
        ] {
            assert!(lint_module("SCR002", dead).is_empty(), "{dead}");
        }
        for live in [
            "def f():\n    if x:\n        return\n    break\n",
            "def f():\n    while x:\n        break\n    break\n",
            "raise E\nbreak\n",
            "def f():\n    return 1\n    yield 2\n",
        ] {
            assert_eq!(lint_module("SCR002", live).len(), 1, "{live}");
        }
    }

    #[test]
    fn broken_scripts_are_left_to_scr001() {
        assert!(lint_module("SCR002", "return 1\nif x\n").is_empty());
    }
}
