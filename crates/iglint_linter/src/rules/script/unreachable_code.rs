//! SCR105 unreachable-code.

use iglint_python::ast::Stmt;
use iglint_text::Ranged;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR105",
    name: "unreachable-code",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Statement can never run",
    explanation: r#"## What it does

Reports the first statement of a block that execution can never reach:
after `return`, `raise`, `break` or `continue`, after an `if`/`else` or
`try`/`except` whose every branch ends that way, and after a `while True:`
loop without `break`. Only the first unreachable statement of a run is
reported. The bodies of `if False:` and `while 0:` are not reported (they
are a common way to disable code on purpose).

Calls that never return (`sys.exit()`) are not taken into account.

## Why is this bad?

The code looks like it does something but never runs. In Perspective
scripts it is often a statement written after an early `return` by
mistake, such as a property write that should happen before it.

## Example

```python
def transform(self, value, quality, timestamp):
    return value * 10
    self.custom.lastValue = value
```

Use instead:

```python
def transform(self, value, quality, timestamp):
    self.custom.lastValue = value
    return value * 10
```

## pylint

Corresponds to `W0101 unreachable`, which only looks at the statement
directly after `return`, `raise`, `break` and `continue`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnreachableCode)],
    options: None,
};

struct UnreachableCode;

fn reason(before: &Stmt) -> &'static str {
    match before {
        Stmt::Return(_) => "after `return`",
        Stmt::Raise(_) => "after `raise`",
        Stmt::Break(_) => "after `break`",
        Stmt::Continue(_) => "after `continue`",
        Stmt::While(_) => "after a loop that never ends",
        _ => "every path before it returns, raises or leaves the loop",
    }
}

impl ScriptRule for UnreachableCode {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        for (stmt, before) in model.unreachable_statements() {
            let message = format!("Unreachable code: {}", reason(before));
            report(cx, unit, stmt.range(), message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn after_terminators() {
        let d = lint_module(
            "SCR105",
            "def f(x):\n    return x\n    x += 1\n    print x\n",
        );
        assert_eq!(flagged(&d), ["x += 1"]);
        assert_eq!(messages(&d), ["Unreachable code: after `return`"]);
        let d = lint_module(
            "SCR105",
            "def f(x):\n    if x:\n        return 1\n    else:\n        raise ValueError()\n    return 2\n",
        );
        assert_eq!(flagged(&d), ["return 2"]);
        let d = lint_module("SCR105", "while True:\n    pass\nprint 1\n");
        assert_eq!(flagged(&d), ["print 1"]);
    }

    #[test]
    fn reachable_code_is_clean() {
        assert!(
            lint_module(
                "SCR105",
                "while True:\n    if g():\n        break\nprint 1\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR105", "if False:\n    print 1\n").is_empty());
        // Reviewer round 15: `True` is a name a module can rebind, so a
        // loop on it may end (`True = 0` makes it end at once on Jython).
        for code in [
            "True = 0\nwhile True:\n    pass\nprint 1\n",
            "def f():\n    global True\n    True = 0\nwhile True:\n    pass\nprint 1\n",
            "exec s\nwhile True:\n    pass\nprint 1\n",
        ] {
            assert!(lint_module("SCR105", code).is_empty(), "{code}");
        }
        // Without a binding, the loop never ends.
        assert_eq!(
            lint_module("SCR105", "x = 0\nwhile True:\n    pass\nprint 1\n").len(),
            1
        );
        assert!(lint_module("SCR105", "def f():\n    try:\n        return g()\n    except ValueError:\n        pass\n    return 0\n").is_empty());
        assert!(
            lint_module(
                "SCR105",
                "def f():\n    try:\n        return g()\n    finally:\n        cleanup()\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn transforms() {
        let d = view(
            ViewScript::Transform,
            "\treturn value\n\tself.custom.x \u{3d} value",
        )
        .lint(&["SCR105"]);
        assert_eq!(decoded_flagged(&d), ["self.custom.x = value"]);
    }
}
