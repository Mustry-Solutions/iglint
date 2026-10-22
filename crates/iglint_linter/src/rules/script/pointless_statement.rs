//! SCR114 pointless-statement.

use iglint_python::ast::{Expr, Stmt};
use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::report;

pub static META: RuleMeta = RuleMeta {
    code: "SCR114",
    name: "pointless-statement",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Expression statement with no effect",
    explanation: r#"## What it does

Reports expression statements that do nothing: a name, an attribute access
(`self.refreshBinding` without parentheses), a comparison (`x == 1` meant
as `x = 1`), arithmetic, a literal. Expressions that call something or
`yield` anywhere are not reported.

Inside a `try` body, a lone name, attribute or subscript is not reported:
`payload["key"]` there checks that the key exists.

Docstrings (the first statement of a module, class or function, including
the body of a Perspective script) and string statements right after an
assignment (attribute docstrings) are not reported. Other string
statements are reported unless `allow-strings` is set: they are often
used to comment out code, which is better done with `#`.

## Why is this bad?

The statement has no effect. Most often a call is missing its parentheses
(`system.perspective.closePopup`) or an assignment was written as a
comparison.

## Example

```python
def runAction(self, event):
    self.view.custom.dirty == False
    self.getSibling("Table").refreshBinding
```

Use instead:

```python
def runAction(self, event):
    self.view.custom.dirty = False
    self.getSibling("Table").refreshBinding("props.data")
```

## Options

* `allow-strings` (default `false`): do not report string statements.

## pylint

Corresponds to `W0104 pointless-statement`, `W0105
pointless-string-statement` and `W0106 expression-not-assigned`. pylint
also reports the existence checks in `try` bodies that iglint allows.
"#,
    options: &[OptionMeta {
        name: "allow-strings",
        ty: OptionType::Bool,
        default: "false",
        description: "Do not report string statements.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&PointlessStatement)],
    options: Some(parse_options),
};

struct Options {
    allow_strings: bool,
}

fn parse_options(reader: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        allow_strings: reader.bool("allow-strings")?,
    }))
}

struct PointlessStatement;

/// Whether evaluating the expression may have an effect.
fn has_effect(e: &Expr) -> bool {
    match e {
        Expr::Call(_) | Expr::Yield(_) => true,
        Expr::Lambda(_) => false,
        other => {
            let mut effect = false;
            iglint_python::visitor::walk_expr(&mut EffectFinder(&mut effect), other);
            effect
        }
    }
}

struct EffectFinder<'f>(&'f mut bool);

impl<'a> iglint_python::visitor::Visitor<'a> for EffectFinder<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(_) | Expr::Yield(_) => *self.0 = true,
            Expr::Lambda(_) => {}
            other => iglint_python::visitor::walk_expr(self, other),
        }
    }
}

fn check_body(
    body: &[Stmt],
    allow_strings: bool,
    in_try: bool,
    out: &mut Vec<(TextRange, &'static str)>,
) {
    for (i, stmt) in body.iter().enumerate() {
        match stmt {
            Stmt::Expr(e) => {
                let value = &*e.value;
                if has_effect(value) {
                    continue;
                }
                if let Expr::Str(_) = value {
                    let docstring = i == 0;
                    let attribute_doc =
                        i > 0 && matches!(body[i - 1], Stmt::Assign(_) | Stmt::AugAssign(_));
                    if docstring || attribute_doc || allow_strings {
                        continue;
                    }
                    out.push((e.range, "String statement has no effect; use a `#` comment"));
                    continue;
                }
                // `d[key]` or `obj.attr` inside `try` checks that it exists.
                if in_try
                    && matches!(
                        value,
                        Expr::Subscript(_) | Expr::Attribute(_) | Expr::Name(_)
                    )
                {
                    continue;
                }
                let message = match value {
                    Expr::Compare(_) => "Comparison has no effect; did you mean `=`?",
                    Expr::Attribute(_) => {
                        "Attribute access has no effect; is a call missing its parentheses?"
                    }
                    Expr::Name(_) => "Name has no effect; is a call missing its parentheses?",
                    _ => "Statement has no effect",
                };
                out.push((e.range, message));
            }
            Stmt::FunctionDef(f) => check_body(&f.body, allow_strings, false, out),
            Stmt::ClassDef(c) => check_body(&c.body, allow_strings, false, out),
            Stmt::For(s) => {
                check_nested(&s.body, allow_strings, in_try, out);
                check_nested(&s.orelse, allow_strings, in_try, out);
            }
            Stmt::While(s) => {
                check_nested(&s.body, allow_strings, in_try, out);
                check_nested(&s.orelse, allow_strings, in_try, out);
            }
            Stmt::If(s) => {
                check_nested(&s.body, allow_strings, in_try, out);
                check_nested(&s.orelse, allow_strings, in_try, out);
            }
            Stmt::With(s) => check_nested(&s.body, allow_strings, in_try, out),
            Stmt::TryExcept(s) => {
                check_nested(
                    &s.body,
                    allow_strings,
                    in_try || !s.handlers.is_empty(),
                    out,
                );
                for h in &s.handlers {
                    check_nested(&h.body, allow_strings, in_try, out);
                }
                check_nested(&s.orelse, allow_strings, in_try, out);
            }
            Stmt::TryFinally(s) => {
                check_nested(&s.body, allow_strings, in_try, out);
                check_nested(&s.finalbody, allow_strings, in_try, out);
            }
            _ => {}
        }
    }
}

/// A nested block: its first statement is not a docstring.
fn check_nested(
    body: &[Stmt],
    allow_strings: bool,
    in_try: bool,
    out: &mut Vec<(TextRange, &'static str)>,
) {
    if let Some(Stmt::Expr(e)) = body.first()
        && matches!(&*e.value, Expr::Str(_))
        && !allow_strings
    {
        out.push((e.range, "String statement has no effect; use a `#` comment"));
        check_body(&body[1..], allow_strings, in_try, out);
        return;
    }
    check_body(body, allow_strings, in_try, out);
}

impl ScriptRule for PointlessStatement {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let allow_strings = cx.options::<Options>().allow_strings;
        let mut found = Vec::new();
        check_body(&unit.parsed.module.body, allow_strings, false, &mut found);
        found.sort_by_key(|f| f.0.start());
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_pointless_statements() {
        let code = "\"\"\"doc\"\"\"\ndef f(self):\n    \"\"\"doc\"\"\"\n    self.refresh\n    x == 1\n    x\n    1 + 2\n    'comment'\n    f()\n    [g(y) for y in z]\n    yield 1\n";
        let d = lint_module("SCR114", code);
        assert_eq!(
            flagged(&d),
            ["self.refresh", "x == 1", "x", "1 + 2", "'comment'"]
        );
    }

    #[test]
    fn docstrings_in_view_scripts_and_options() {
        assert!(
            view(
                ViewScript::Action,
                "\t\"\"\"Opens the popup.\"\"\"\n\tsystem.perspective.openPopup('p', 'v')"
            )
            .lint(&["SCR114"])
            .is_empty()
        );
        let d = view(ViewScript::Action, "\tsystem.perspective.closePopup").lint(&["SCR114"]);
        assert_eq!(decoded_flagged(&d), ["system.perspective.closePopup"]);
        let f = library("m", "x = 1\n'attribute doc'\nif x:\n    'disabled code'\n");
        assert_eq!(flagged(&f.lint(&["SCR114"])), ["'disabled code'"]);
        assert!(
            f.lint_with(&[("SCR114", "allow-strings = true")])
                .is_empty()
        );
    }

    #[test]
    fn existence_checks_in_try() {
        let code = "def f(v):\n    try:\n        v[0]['k']\n        v.attr\n        return True\n    except (KeyError, AttributeError):\n        return False\n";
        assert!(lint_module("SCR114", code).is_empty());
    }
}
