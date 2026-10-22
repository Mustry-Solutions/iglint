//! SCR116 raise-string.

use iglint_python::ast::{Expr, Operator, Stmt};
use iglint_text::Ranged;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR116",
    name: "raise-string",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`raise` with a string",
    explanation: r#"## What it does

Reports `raise "message"`, including strings built with `%` or `+` from a
string literal.

## Why is this bad?

String exceptions were removed in Python 2.6. Jython raises `TypeError:
exceptions must be old-style classes or derived from BaseException, not
str` instead of the intended error, and the message is lost.

## Example

```python
if not rows:
    raise "no data for line %s" % line
```

Use instead:

```python
if not rows:
    raise ValueError("no data for line %s" % line)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&RaiseString)],
    options: None,
};

struct RaiseString;

fn is_string(e: &Expr) -> bool {
    match e {
        Expr::Str(_) => true,
        Expr::BinOp(b) => matches!(b.op, Operator::Mod | Operator::Add) && is_string(&b.left),
        _ => false,
    }
}

impl ScriptRule for RaiseString {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            if let Stmt::Raise(r) = s
                && let Some(t) = &r.type_
                && is_string(t)
            {
                found.push(t.range());
            }
        });
        for range in found {
            report(
                cx,
                unit,
                range,
                "Raising a string fails with `TypeError`; raise an exception instance such as `ValueError(...)`",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_string_raises() {
        let d = lint_module(
            "SCR116",
            "raise 'a'\nraise 'b %s' % x\nraise ValueError, 'ok'\nraise ValueError('ok')\n",
        );
        assert_eq!(flagged(&d), ["'a'", "'b %s' % x"]);
    }
}
