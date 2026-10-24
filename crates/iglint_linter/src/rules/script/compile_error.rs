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
  (`-a = 1`, `for f() in x:`, `except E, a + b:`);
* a `yield` lexically inside a list comprehension, or in the first
  iterable of a set or dict comprehension (`[(yield) for i in x]`,
  `{a for a in (yield)}`): Jython's code generator emits bytecode its own
  verifier rejects (`java.lang.VerifyError`). A `yield` in the element,
  filter or later iterables of a set, dict or generator comprehension, and
  in the first iterable of a generator expression, compiles.
* a `yield` in the exception type of an `except` clause (`except
  (yield):`, `except E((yield)):`; Jython's code generator fails with
  `NegativeArraySizeException`), or in an attribute or subscript target
  inside an unpacking target (`b, a[(yield)] = 1, 2`, also in `for` and
  `with ... as` targets; `IllegalArgumentException`). A single target
  (`a[(yield)] = 1`), an augmented assignment and `del` compile, and so
  does a `yield` inside a `lambda` there.

It also reports scripts that are too large for Jython:

* the first two lines together are 100,000 characters or longer (UTF-16
  code units, line ends included; in Perspective and event scripts the
  first line is the generated `def` header). Jython reads them ahead to
  look for an encoding declaration and fails with
  `java.io.IOException: Mark invalid`. Longer lines further down are fine;
* a module, class or function body whose code certainly exceeds one Java
  method (`Module or method too large`: one body is one Java method of at
  most 65,535 bytes of bytecode). iglint adds, for every statement Jython
  generates code for, the least bytecode Jython's code generator emits for
  it and for each of its expressions (the line number update, every name,
  constant, attribute, subscript, call, operator, comparison, display,
  comprehension and branch), measured on Ignition 8.3.3's Jython and
  checked against the size of every body of thousands of generated
  programs, and reports a body only when that lower bound exceeds the
  limit. How many statements fit depends on what they hold: Jython fails
  from 9,380 lines of `pass`, 3,283 of `x = 1`, 2,741 of `xN = "vN"` at
  module level, 1,880 of `f(N, "a", b)` and 680 of `d["kN"] = {"a": N,
  "b": [1, 2, 3]}` in a function, and iglint reports these from 9,382,
  3,290, about 3,200, about 2,100 and about 740 lines;
* a module with too many functions, classes, lambdas and generator
  expressions for Jython's class initializer (`Module or method too
  large`: the class's `<init>` builds one code object per body, with its
  variable names, and is one Java method too). iglint counts, per body,
  the instructions Jython's code generator emits for its code object at
  their shortest encoding (29 to 32 bytes, and 5 or 6 per parameter or
  assigned local variable of a function), checked against the `<init>`
  of hundreds of generated modules near the limit with 0 false reports.
  Jython fails from about 765 functions of eight parameters, and iglint
  reports these from 900;
* a module with more than 32,767 distinct string literals (`Class too
  large`: one module is one Java class, with at most 65,535 constants;
  Jython fails from 32,594);
* a function or class whose name makes the name of its Java method too
  long (`java.lang.IllegalArgumentException: UTF8 string too large`):
  Jython names the method of a body `<name>$<number>`, with a private
  name mangled in a class, and a class file holds names of at most 65,535
  bytes, so a function or class of the module fails from 65,534
  characters. Long names elsewhere (variables, attributes, keywords,
  imports, docstrings) and long strings compile: Jython loads them in
  chunks.

Jython generates no code for the rest of a block after `return`, `raise`,
`continue` or an `if`/`else` whose every branch ends that way (but not
after `break`, a loop, `try` or `with`, and not for the module's own
statements). That dead code counts for neither size limit, and the errors
above that Jython's code generator raises (`return`, `yield`, `break` and
`continue` in the wrong place, `import *`, a bare `except:` that is not
last, `del` of a variable a nested function uses, invalid targets) are not
reported in it.

Invalid targets (`-a = 1`, `for f() in x`) crash Jython only where the
generated bytecode can be reached, so they are also not reported after
any statement that does not complete: `break`, a `try` whose body and
handlers (or whose `finally`) do not complete, a loop whose `else` does
not complete and that has no `break`. A new function, class or lambda
body is reachable again. That code is still generated: it counts for the
size limits, and the other errors are reported in it.

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
        // One finding per construct: an error inside the range of an
        // earlier error with the same message (a recursive construct
        // reported at each level) adds nothing.
        let mut reported: Vec<(iglint_text::TextRange, String)> = Vec::new();
        for error in compile_errors(model) {
            if reported
                .iter()
                .any(|(r, m)| *m == error.message && r.contains_range(error.range))
            {
                continue;
            }
            report(
                cx,
                unit,
                error.range,
                format!("SyntaxError: {}", error.message),
            );
            reported.push((error.range, error.message));
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

    /// Recursive constructs give one finding, not one per level
    /// (reviewer note R6-D). Chains near the depth limits are covered by
    /// the stress tests of the binary.
    #[test]
    fn nested_constructs_do_not_flood() {
        for code in [
            format!("({}a) = 1\n", "-".repeat(200)),
            format!("({}a) = 1\n", "not ".repeat(200)),
            format!("{}a{} = 1\n", "-(".repeat(100), ")".repeat(100)),
            format!("for ({}a) in x:\n    pass\n", "~".repeat(200)),
        ] {
            assert_eq!(
                lint_module("SCR002", &code).len(),
                1,
                "{}",
                code.get(..20).unwrap()
            );
        }
    }

    #[test]
    fn yield_in_comprehension_crashes_jython() {
        // Verified against Jython 2.7 (compile.cases `yield-comp-*`).
        for code in [
            "def f():\n    x = [(yield i) for i in range(3)]\n",
            "def f():\n    x = [i for i in (yield)]\n",
            "def f():\n    x = [i for i in range(3) if (yield i)]\n",
            "def f():\n    x = {i for i in (yield)}\n",
            "def f():\n    x = {i: 1 for i in (yield)}\n",
            "def f():\n    x = ([(yield) for j in r] for i in s)\n",
        ] {
            let d = lint_module("SCR002", code);
            assert_eq!(d.len(), 1, "{code}");
            assert!(d[0].message.contains("crashes Jython's compiler"), "{code}");
        }
        // Positions a set, dict or generator comprehension puts in its own
        // code object compile: element, filter and later iterables, and a
        // generator expression's first iterable.
        for code in [
            "def f():\n    x = ((yield i) for i in range(3))\n",
            "def f():\n    x = (i for i in (yield))\n",
            "def f():\n    x = {(yield i) for i in range(3)}\n",
            "def f():\n    x = {i: (yield i) for i in range(3)}\n",
            "def f():\n    x = {j for i in r for j in (yield)}\n",
            "def f():\n    x = [g() for i in r]\n    def g():\n        yield i\n",
        ] {
            assert!(lint_module("SCR002", code).is_empty(), "{code}");
        }
    }

    /// Review round 20 (Codex): a function name of 70,000 characters
    /// (`UTF8 string too large` on Jython).
    #[test]
    fn names_too_long_for_a_java_method() {
        let code = format!("\n\ndef {}():\n    return 1\n", "n".repeat(70_000));
        let d = lint_module("SCR002", &code);
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(d[0].message.contains("UTF8 string too large"), "{d:?}");
        let code = format!("\n\n{} = 1\n", "n".repeat(70_000));
        assert!(lint_module("SCR002", &code).is_empty());
    }

    /// Review round 20 (Opus): a `yield` in an exception type or in an
    /// attribute or subscript target of an unpacking (verified against
    /// Jython, `oracle/compile_cross.py`).
    #[test]
    fn yield_in_except_types_and_unpacking_targets_crashes_jython() {
        for code in [
            "def f():\n    try:\n        pass\n    except (yield):\n        pass\n",
            "def f():\n    try:\n        pass\n    except (E, (yield)), e:\n        pass\n",
            "def f():\n    b, a[(yield)] = 1, 2\n",
            "def f():\n    (b, (yield).x), c = (1, 2), 3\n",
            "def f():\n    for b, a[(yield):2] in s:\n        pass\n",
            "def f():\n    with c as [b, a[(yield)]]:\n        pass\n",
        ] {
            let d = lint_module("SCR002", code);
            assert_eq!(d.len(), 1, "{code}");
            assert!(d[0].message.contains("crashes Jython's compiler"), "{code}");
        }
        for code in [
            "def f():\n    try:\n        pass\n    except E, a[(yield)]:\n        pass\n",
            "def f():\n    try:\n        pass\n    except (lambda: (yield))():\n        pass\n",
            "def f():\n    a[(yield)] = 1\n",
            "def f():\n    a[(yield)] = b = 1\n",
            "def f():\n    a[(yield)] += 1\n",
            "def f():\n    del b, a[(yield)]\n",
            "def f():\n    for a[(yield)] in s:\n        pass\n",
            "def f():\n    b, a[0] = (yield), 2\n",
            "def f():\n    yield 1\n    return\n    b, a[(yield)] = 1, 2\n",
        ] {
            assert!(lint_module("SCR002", code).is_empty(), "{code}");
        }
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
        // Review round 18: too many code objects for the class
        // initializer. Jython fails from about 765 functions of eight
        // parameters.
        let functions = |n: usize| -> String {
            (0..n)
                .map(|i| format!("def g{i}(a, b, c, d, e, f1, f2, f3):\n    return a\n"))
                .collect()
        };
        assert!(lint_module("SCR002", &functions(760)).is_empty());
        let d = lint_module("SCR002", &functions(950));
        assert_eq!(d.len(), 1, "{d:?}");
        assert!(
            d[0].message
                .contains("too many functions, classes, lambdas and generator expressions"),
            "{}",
            d[0].message
        );
        // Review round 14: expressions count. Jython fails from 680
        // statements of this shape.
        let items = |n: usize| -> String {
            (0..n)
                .map(|i| format!("    d[\"k{i}\"] = {{\"a\": {i}, \"b\": [1, 2, 3]}}\n"))
                .collect()
        };
        assert!(lint_module("SCR002", &format!("def f():\n{}", items(679))).is_empty());
        let d = lint_module("SCR002", &format!("def f():\n{}", items(760)));
        assert_eq!(flagged(&d), ["f"]);
        assert!(d[0].message.contains("at least"), "{}", d[0].message);
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

    /// Verified on Jython: invalid targets only crash its compiler in
    /// reachable bytecode; the code after `try`/`finally` is generated
    /// (other errors there are reported) but may be unreachable.
    #[test]
    fn invalid_targets_in_unreachable_code_compile() {
        for unreachable in [
            "def f():\n    try:\n        return\n    finally:\n        pass\n    -a = 1\n",
            "def f():\n    for i in x:\n        break\n        -a = 1\n",
            "def f():\n    try:\n        return\n    except E:\n        return\n    -a = 1\n",
            "def f():\n    for i in x:\n        pass\n    else:\n        return\n    for g() in y:\n        pass\n",
            "raise X\n-a = 1\n",
        ] {
            assert!(
                lint_module("SCR002", unreachable).is_empty(),
                "{unreachable}"
            );
        }
        for reachable in [
            "def f():\n    try:\n        pass\n    finally:\n        pass\n    -a = 1\n",
            "def f():\n    with x:\n        return\n    -a = 1\n",
            "def f():\n    try:\n        return\n    except:\n        pass\n    -a = 1\n",
            "def f():\n    while 1:\n        break\n    -a = 1\n",
            "def f():\n    try:\n        return\n    finally:\n        pass\n    class K:\n        -a = 1\n",
        ] {
            assert_eq!(lint_module("SCR002", reachable).len(), 1, "{reachable}");
        }
        // Generated, so other errors are reported.
        let d = lint_module(
            "SCR002",
            "def f():\n    yield 1\n    try:\n        return\n    finally:\n        pass\n    return 2\n",
        );
        assert_eq!(flagged(&d), ["return 2"]);
    }

    #[test]
    fn broken_scripts_are_left_to_scr001() {
        assert!(lint_module("SCR002", "return 1\nif x\n").is_empty());
    }
}
