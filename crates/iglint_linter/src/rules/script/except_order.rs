//! SCR113 except-order.

use iglint_python::ast::{Expr, Stmt};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{file_line, report, semantic, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR113",
    name: "except-order",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Exception class caught after a broader one",
    explanation: r#"## What it does

Reports an `except` clause naming a builtin exception class that an
earlier clause of the same `try` already catches, because it names the
same class or a base class (`Exception` before `ValueError`, `LookupError`
before `KeyError`). Only builtin exception classes are compared (the
hierarchy was checked against Jython 2.7); Java exception classes are not.

## Why is this bad?

The later handler never runs: the earlier, broader handler takes every
exception it would have handled.

## Example

```python
try:
    value = int(payload["value"])
except Exception:
    value = None
except KeyError:
    value = 0
```

Use instead:

```python
try:
    value = int(payload["value"])
except KeyError:
    value = 0
except Exception:
    value = None
```

## pylint

Corresponds to `E0701 bad-except-order`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&ExceptOrder)],
    options: None,
};

struct ExceptOrder;

/// Jython 2.7's builtin exception hierarchy: class and base.
const BASES: &[(&str, &str)] = &[
    ("SystemExit", "BaseException"),
    ("KeyboardInterrupt", "BaseException"),
    ("GeneratorExit", "BaseException"),
    ("Exception", "BaseException"),
    ("StopIteration", "Exception"),
    ("StandardError", "Exception"),
    ("Warning", "Exception"),
    ("BufferError", "StandardError"),
    ("ArithmeticError", "StandardError"),
    ("FloatingPointError", "ArithmeticError"),
    ("OverflowError", "ArithmeticError"),
    ("ZeroDivisionError", "ArithmeticError"),
    ("AssertionError", "StandardError"),
    ("AttributeError", "StandardError"),
    ("EnvironmentError", "StandardError"),
    ("IOError", "EnvironmentError"),
    ("OSError", "EnvironmentError"),
    ("EOFError", "StandardError"),
    ("ImportError", "StandardError"),
    ("LookupError", "StandardError"),
    ("IndexError", "LookupError"),
    ("KeyError", "LookupError"),
    ("MemoryError", "StandardError"),
    ("NameError", "StandardError"),
    ("UnboundLocalError", "NameError"),
    ("ReferenceError", "StandardError"),
    ("RuntimeError", "StandardError"),
    ("NotImplementedError", "RuntimeError"),
    ("SyntaxError", "StandardError"),
    ("IndentationError", "SyntaxError"),
    ("TabError", "IndentationError"),
    ("SystemError", "StandardError"),
    ("TypeError", "StandardError"),
    ("ValueError", "StandardError"),
    ("UnicodeError", "ValueError"),
    ("UnicodeDecodeError", "UnicodeError"),
    ("UnicodeEncodeError", "UnicodeError"),
    ("UnicodeTranslateError", "UnicodeError"),
    ("DeprecationWarning", "Warning"),
    ("PendingDeprecationWarning", "Warning"),
    ("RuntimeWarning", "Warning"),
    ("SyntaxWarning", "Warning"),
    ("UserWarning", "Warning"),
    ("FutureWarning", "Warning"),
    ("ImportWarning", "Warning"),
    ("UnicodeWarning", "Warning"),
    ("BytesWarning", "Warning"),
];

/// Whether `base` is `class` or one of its bases.
fn is_subclass(class: &str, base: &str) -> bool {
    let mut at = class;
    loop {
        if at == base {
            return true;
        }
        match BASES.iter().find(|(c, _)| *c == at) {
            Some((_, parent)) => at = parent,
            None => return false,
        }
    }
}

impl ScriptRule for ExceptOrder {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        // Builtin exception classes named by a handler type.
        let classes = |t: &Expr| -> Vec<(String, TextRange)> {
            let mut out = Vec::new();
            let mut one = |e: &Expr| {
                if let Expr::Name(n) = e
                    && (n.id.as_ref() == "BaseException" || BASES.iter().any(|(c, _)| *c == &*n.id))
                    && super::is_builtin_reference(model, e, &n.id)
                {
                    out.push((n.id.to_string(), n.range));
                }
            };
            match t {
                Expr::Tuple(tuple) => tuple.elts.iter().for_each(&mut one),
                other => one(other),
            }
            out
        };
        let mut found = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::TryExcept(t) = s else { return };
            let mut earlier: Vec<(String, TextRange)> = Vec::new();
            for h in &t.handlers {
                let Some(ty) = &h.type_ else { continue };
                let here = classes(ty);
                for (class, range) in &here {
                    if let Some((base, base_range)) =
                        earlier.iter().find(|(b, _)| is_subclass(class, b))
                    {
                        found.push((*range, class.clone(), base.clone(), *base_range));
                    }
                }
                earlier.extend(here);
            }
        });
        for (range, class, base, base_range) in found {
            let message = match file_line(cx, unit, base_range.start()) {
                Some(line) if class == base => {
                    format!("`{class}` is already caught by the handler on line {line}")
                }
                Some(line) => format!(
                    "`{class}` is never caught here: `{base}` on line {line} catches it first"
                ),
                None => format!("`{class}` is never caught here: `{base}` catches it first"),
            };
            let label = unit.script.file_range(base_range);
            let file = unit.file;
            let _ = report(cx, unit, range, message)
                .label(file, label, "broader handler")
                .fingerprint_key(format!("{class} after {base}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_shadowed_handlers() {
        let code = "try:\n    f()\nexcept (LookupError, TypeError):\n    pass\nexcept KeyError:\n    pass\nexcept Exception:\n    pass\nexcept ValueError:\n    pass\n";
        let d = lint_module("SCR113", code);
        assert_eq!(flagged(&d), ["KeyError", "ValueError"]);
        assert_eq!(
            d[0].message,
            "`KeyError` is never caught here: `LookupError` on line 3 catches it first"
        );
    }

    #[test]
    fn correct_order_and_unknown_classes_are_fine() {
        assert!(
            lint_module(
                "SCR113",
                "try:\n    f()\nexcept KeyError:\n    pass\nexcept Exception:\n    pass\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR113", "from java.lang import Exception as JE\ntry:\n    f()\nexcept Throwable:\n    pass\nexcept ValueError:\n    pass\n").is_empty());
        // A shadowed builtin name is not the builtin.
        assert!(lint_module("SCR113", "Exception = MyError\ntry:\n    f()\nexcept Exception:\n    pass\nexcept ValueError:\n    pass\n").is_empty());
    }
}
