//! SCR111 assert-on-tuple.

use iglint_python::ast::{Expr, Stmt};
use iglint_text::Ranged;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR111",
    name: "assert-on-tuple",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`assert` on a non-empty tuple is always true",
    explanation: r#"## What it does

Reports `assert (condition, "message")`: an assertion whose test is a
non-empty tuple.

## Why is this bad?

A non-empty tuple is always true, so the assertion never fails. The
parentheses turned the condition and the message into one tuple.

## Example

```python
assert (len(rows) > 0, "no rows returned")
```

Use instead:

```python
assert len(rows) > 0, "no rows returned"
```

## pylint

Corresponds to `W0199 assert-on-tuple`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&AssertOnTuple)],
    options: None,
};

struct AssertOnTuple;

impl ScriptRule for AssertOnTuple {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            if let Stmt::Assert(a) = s
                && let Expr::Tuple(t) = &*a.test
                && !t.elts.is_empty()
            {
                found.push(a.test.range());
            }
        });
        for range in found {
            report(
                cx,
                unit,
                range,
                "`assert` on a non-empty tuple is always true",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_tuples() {
        let d = lint_module("SCR111", "assert (x, 'm')\nassert x, 'm'\nassert ()\n");
        assert_eq!(flagged(&d), ["(x, 'm')"]);
    }
}
