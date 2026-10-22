//! SCR106 bare-except.

use iglint_python::ast::{ExceptHandler, Stmt};
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR106",
    name: "bare-except",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "`except:` without an exception type",
    explanation: r#"## What it does

Reports `except:` clauses without an exception type, unless the handler
re-raises (a bare `raise` directly in its body).

## Why is this bad?

A bare `except:` catches everything: `SystemExit`, `KeyboardInterrupt`,
`GeneratorExit` and, in Jython, Java exceptions too. It hides programming
errors such as a misspelled name (`NameError`) along with the failure it
was meant for.

Note that in Jython `except Exception:` does **not** catch Java exceptions
(`java.lang.NullPointerException`, SQL exceptions from `system.db.*`).
Name both when that is the intent:

```python
from java.lang import Throwable

try:
    system.db.runPrepUpdate(query, args)
except (Exception, Throwable), e:
    logger.error("update failed: %s" % e)
```

## Example

```python
try:
    value = float(payload["value"])
except:
    value = 0.0
```

Use instead:

```python
try:
    value = float(payload["value"])
except (KeyError, TypeError, ValueError):
    value = 0.0
```

## pylint

Corresponds to `W0702 bare-except` (pylint also reports handlers that
re-raise).
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&BareExcept)],
    options: None,
};

struct BareExcept;

fn reraises(handler: &ExceptHandler) -> bool {
    handler.body.iter().any(|s| {
        matches!(s, Stmt::Raise(r) if r.type_.is_none() && r.inst.is_none() && r.tback.is_none())
    })
}

impl ScriptRule for BareExcept {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |stmt| {
            if let Stmt::TryExcept(t) = stmt {
                for h in &t.handlers {
                    if h.type_.is_none() && !reraises(h) {
                        found.push(TextRange::at(h.range.start(), TextSize::of("except")));
                    }
                }
            }
        });
        for range in found {
            report(
                cx,
                unit,
                range,
                "Bare `except:` also catches `SystemExit` and Java exceptions; name the exception types",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_bare_except() {
        let d = lint_module("SCR106", "try:\n    f()\nexcept:\n    pass\n");
        assert_eq!(lines(&d), [(3, "except".to_owned())]);
    }

    #[test]
    fn typed_and_reraising_handlers_are_fine() {
        assert!(lint_module("SCR106", "try:\n    f()\nexcept Exception:\n    pass\n").is_empty());
        assert!(lint_module("SCR106", "try:\n    f()\nexcept:\n    log()\n    raise\n").is_empty());
    }

    #[test]
    fn view_scripts() {
        let d = view(ViewScript::Handler, "\ttry:\n\t\tx()\n\texcept:\n\t\tpass").lint(&["SCR106"]);
        assert_eq!(decoded_flagged(&d), ["except"]);
    }
}
