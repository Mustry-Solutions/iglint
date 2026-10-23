//! SCR103 unused-variable.

use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::{BindingKind, Resolution, ScopeKind};
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::options::OptionReader;
use crate::registry::{Category, FixAvailability, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::except_tuple::misread_handler_names;
use crate::rules::script::{
    StatementEdit, containing_body, delete_statement, report, semantic, source, statement_fix,
};

pub static META: RuleMeta = RuleMeta {
    code: "SCR103",
    name: "unused-variable",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "Local variable is assigned but never read",
    explanation: r#"## What it does

Reports local variables of functions (including Perspective scripts, which
are function bodies) whose name is assigned but never read, neither in the
function nor in a nested function or lambda. Assignments, `with ... as x`
and `except E, x` count; parameters, loop variables, comprehension
variables and imports (see `SCR102`) do not.

Not reported:

* names that are part of a tuple or list target (`a, b = f()`, where only
  one may be needed);
* names matching the dummy pattern: `_`, starting with `_`, `dummy`,
  `ignored_...`, `unused_...` (option `dummy-pattern`);
* functions that read their locals dynamically (`locals()`, `vars()`,
  `eval`, `exec`), for example `"%(name)s" % locals()`;
* the exception class in `except ValueError, TypeError:`, which `SCR115`
  reports.

## Why is this bad?

An unused variable is dead code at best; often it is a typo (the value was
meant to be used under that name) or a forgotten step (`result =
system.db.runPrepQuery(...)` whose result is never written anywhere).

## Example

```python
def runAction(self, event):
    rows = system.db.runNamedQuery("Orders/Open")
    count = len(rows)
    self.props.text = "%d open orders" % len(rows)
```

Use instead:

```python
def runAction(self, event):
    rows = system.db.runNamedQuery("Orders/Open")
    self.props.text = "%d open orders" % len(rows)
```

## Options

* `dummy-pattern` (regex, default `^(_.*|dummy.*|ignored_.*|unused_.*)$`):
  names that may be unused.

## Fix

For `x = value` with a single target: removes the statement when `value`
calls nothing and binds nothing (literals, names, attribute and subscript
reads, operators, comparisons), otherwise removes only `x = ` and keeps
the expression, so that its calls still run and the variables of a list
comprehension stay bound (Python 2 leaks them: `x = [i for i in y]`
followed by `return i`). Backticks call `repr`. What is kept is never a
pointless statement for `SCR114`. Unsafe: the value may still be wanted
under another name, and a removed read or operator can have effects in
Python (`value["pages"]` raises `KeyError` when the key is missing; a
property, `__getattr__`, a Java getter, `__eq__`, `__contains__` or the
`__str__` that `%` formatting calls runs code).

## pylint

Corresponds to `W0612 unused-variable`. pylint also reports unused names
of tuple targets (`a, b = f()`); iglint does not.
"#,
    options: &[OptionMeta {
        name: "dummy-pattern",
        ty: OptionType::String,
        default: r#""^(_.*|dummy.*|ignored_.*|unused_.*)$""#,
        description: "Names matching this regex may be unused.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnusedVariable)],
    options: Some(parse_options),
};

struct Options {
    dummy: regex::Regex,
}

fn parse_options(reader: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let pattern = reader.string("dummy-pattern")?;
    let dummy = regex::Regex::new(&pattern).map_err(|e| format!("dummy-pattern: {e}"))?;
    Ok(Box::new(Options { dummy }))
}

struct UnusedVariable;

/// Where, message, and the edit of the fix.
type Finding = (TextRange, String, Option<StatementEdit>);

impl ScriptRule for UnusedVariable {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let text = source(unit);
        // Names read anywhere (even before an assignment, or in a nested
        // function), per scope that owns them.
        let mut read: rustc_hash::FxHashSet<(iglint_python_semantic::ScopeId, &str)> =
            rustc_hash::FxHashSet::default();
        for (_, r) in model.references() {
            match r.resolution {
                Resolution::Local => {
                    read.insert((r.scope, r.name));
                }
                Resolution::Free(owner) => {
                    read.insert((owner, r.name));
                }
                _ => {}
            }
        }
        let misread = misread_handler_names(unit, model);
        let mut found: Vec<Finding> = Vec::new();
        for (scope_id, scope) in model.scopes() {
            if scope.kind != ScopeKind::Function || scope.reads_locals || scope.is_dynamic() {
                continue;
            }
            for name in scope.local_names() {
                let ids = scope.bindings_of(name);
                if ids.is_empty() || cx.options::<Options>().dummy.is_match(name) {
                    continue;
                }
                if read.contains(&(scope_id, *name)) {
                    continue;
                }
                let bindings: Vec<_> = ids.iter().map(|id| model.binding(*id)).collect();
                if bindings.iter().any(|b| {
                    b.is_used()
                        || misread.contains(&b.range)
                        || b.unpacked
                        || !matches!(
                            b.kind,
                            BindingKind::Assignment
                                | BindingKind::AugAssignment
                                | BindingKind::WithVar
                                | BindingKind::ExceptVar
                        )
                }) {
                    continue;
                }
                let first = bindings[0];
                let edit = if bindings.len() == 1 {
                    first.stmt.and_then(|s| removal(model, text, s))
                } else {
                    None
                };
                found.push((
                    first.range,
                    format!("Local variable `{name}` is assigned to but never used"),
                    edit,
                ));
            }
        }
        found.sort_by_key(|f| f.0.start());
        for (range, message, edit) in found {
            let fix = edit.and_then(|e| {
                statement_fix(cx, unit, "Remove the assignment", Applicability::Unsafe, e)
            });
            let _ = report(cx, unit, range, message).fix_opt(fix);
        }
    }
}

/// Removes `x = value`: the statement when the value calls nothing, else
/// `x = ` (the remaining expression statement still makes its calls, and
/// `SCR114` does not report it).
fn removal(
    model: &iglint_python_semantic::SemanticModel<'_>,
    text: &str,
    stmt: &Stmt,
) -> Option<StatementEdit> {
    let Stmt::Assign(a) = stmt else { return None };
    let [Expr::Name(target)] = a.targets.as_slice() else {
        return None;
    };
    if !super::pointless_statement::has_effect(&a.value) {
        let body = containing_body(&model.module.body, stmt)?;
        return delete_statement(text, body, stmt);
    }
    let prefix = TextRange::new(target.range.start(), a.value.range().start());
    // Only `x = ` (possibly with parentheses around the value).
    let between = &text[target.range.end().to_usize()..a.value.range().start().to_usize()];
    if !between.trim().trim_end_matches('(').trim().eq("=") {
        return None;
    }
    let paren = between.trim_end().ends_with('(');
    if paren {
        return None;
    }
    Some(StatementEdit::replace(prefix, String::new()))
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_unused_locals() {
        let d = lint_module("SCR103", "def f():\n    x = 1\n    y = 2\n    return y\n");
        assert_eq!(flagged(&d), ["x"]);
        assert_eq!(
            messages(&d),
            ["Local variable `x` is assigned to but never used"]
        );
    }

    #[test]
    fn exemptions() {
        // Module level, tuple targets, dummy names, loop variables, params.
        assert!(lint_module("SCR103", "x = 1\n").is_empty());
        assert!(lint_module("SCR103", "def f():\n    a, b = g()\n    return a\n").is_empty());
        assert!(lint_module("SCR103", "def f():\n    _x = 1\n    unused_y = 2\n").is_empty());
        assert!(
            lint_module(
                "SCR103",
                "def f(p):\n    for i in range(3):\n        pass\n"
            )
            .is_empty()
        );
        assert!(
            lint_module(
                "SCR103",
                "def f():\n    x = 1\n    return '%(x)s' % locals()\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR103", "def f():\n    x = 0\n    x += 1\n").is_empty());
        // Read by a nested function, or read before the assignment (that is
        // SCR118's finding).
        assert!(lint_module("SCR103", "def f():\n    x = 1\n    return lambda: x\n").is_empty());
        assert!(lint_module("SCR103", "def f():\n    print x\n    x = 1\n").is_empty());
    }

    #[test]
    fn with_and_except_variables() {
        let d = lint_module(
            "SCR103",
            "def f():\n    try:\n        g()\n    except Exception, e:\n        pass\n    with open('a') as fh:\n        pass\n",
        );
        assert_eq!(flagged(&d), ["e", "fh"]);
    }

    #[test]
    fn perspective_scripts_are_functions() {
        let d = view(
            ViewScript::Action,
            "\tresult \u{3d} system.db.runNamedQuery('q')",
        )
        .lint(&["SCR103"]);
        assert_eq!(decoded_flagged(&d), ["result"]);
    }

    #[test]
    fn options() {
        let f = library("m", "def f():\n    tmp = 1\n");
        assert!(
            f.lint_with(&[("SCR103", "dummy-pattern = '^tmp$'")])
                .is_empty()
        );
    }

    #[test]
    fn unsafe_fix() {
        let f = library("m", "def f():\n    x = 1\n    y = g()\n    return 0\n");
        let fixed = f.fixed(&[("SCR103", "")], true, &library_path(&f, "m"));
        assert_eq!(fixed, "def f():\n    g()\n    return 0\n");
        let f = view(ViewScript::Action, "\tx \u{3d} 'a'");
        let fixed = f.fixed(&[("SCR103", "")], true, &view_path(&f));
        assert!(fixed.contains(r#""script": "\tpass""#), "{fixed}");
    }

    #[test]
    fn removing_every_assignment_of_a_suite_leaves_pass() {
        let f = library(
            "m",
            "def f(c):\n    if c:\n        x = 1\n        y = 2\n    else:\n        z = 3\n    a = 1\n    b = 2\n",
        );
        let fixed = f.fixed(&[("SCR103", "")], true, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "def f(c):\n    if c:\n        pass\n    else:\n        pass\n"
        );
        let f = view(ViewScript::Action, "\tx \u{3d} 'a'\n\ty \u{3d} 'b'");
        let fixed = f.fixed(&[("SCR103", "")], true, &view_path(&f));
        assert!(fixed.contains(r#""script": "\tpass""#), "{fixed}");
    }

    /// List comprehension variables stay bound after the comprehension
    /// (Python 2), and backticks call `repr`: the fix keeps the value.
    /// Jython: `f([1, 2])` returns 2 before and after the fix, and raises
    /// `NameError` if the statement is removed.
    #[test]
    fn fix_keeps_values_that_bind_or_call() {
        let f = library(
            "m",
            "def f(y):\n    x = [i for i in y]\n    return i\ndef g(o):\n    r = `o`\n    s = ([j for j in o], 1)\n    return 0\n",
        );
        let fixed = f.fixed(&[("SCR103", "")], true, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "def f(y):\n    [i for i in y]\n    return i\ndef g(o):\n    `o`\n    ([j for j in o], 1)\n    return 0\n"
        );
        assert!(library("m", &fixed).lint(&["SCR114"]).is_empty());
    }

    /// The fix never leaves a pointless statement behind (`SCR114`).
    #[test]
    fn fix_leaves_no_pointless_statement() {
        let f = library(
            "m",
            "def f(value, obj):\n    pages = value['pages']\n    n = obj.count + 1\n    r = obj.load()['x']\n    return 0\n",
        );
        let fixed = f.fixed(&[("SCR103", "")], true, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "def f(value, obj):\n    obj.load()['x']\n    return 0\n"
        );
        let after = library("m", &fixed);
        assert!(after.lint(&["SCR114"]).is_empty());
    }
}
