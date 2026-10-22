//! SEC001 sql-string-building.

use iglint_python::ast::{Expr, Operator, Stmt};
use iglint_python_semantic::{BindingKind, SemanticModel};
use iglint_text::{Ranged, TextRange};

use super::{MAX_DEPTH, argument, constant_string, is_fixed};
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SEC001",
    name: "sql-string-building",
    category: Category::Sec,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "SQL text built from values at run time",
    explanation: r#"## What it does

Reports SQL text passed to `system.db.runQuery`, `runUpdateQuery`,
`runScalarQuery`, `runSFUpdateQuery` and to the prepared-statement
functions `runPrepQuery`, `runPrepUpdate`, `runScalarPrepQuery` and
`runSFPrepUpdate` (and the hidden `runPrepStmt`, `runPrepStmtGetKey`,
`runUpdateQueryGetKey`) when the text is built with `%`, `+`,
`.format(...)` or `.join(...)` from values that are not literals.

The functions are recognised through imports and aliases (`db =
system.db`, `from system.db import runQuery`). The query is the first
argument or the `query` keyword. When it is a local name, every assignment
that reaches the call is followed (`q = "... '%s'" % name` and `q += ...`
before `system.db.runQuery(q)`); the finding points at the expression that
builds the text.

Text built only from literals is fine: concatenated literals, module
constants holding literals, placeholder lists such as `",".join(["?"] *
len(ids))`, and `%` formatting whose conversions are all numeric (`%d`,
`%i`, `%f`, ...), which cannot carry quotes.

The prepared-statement functions are included on purpose: their `?`
placeholders only protect the values passed as arguments, not values
pasted into the SQL text itself.

## Why is this bad?

Values pasted into SQL text can change the statement (SQL injection), and
quotes or special characters in ordinary data (a name such as `O'Brien`)
break it. Prepared statements pass the values separately, and the
database driver also handles types and date formats.

Table or column names chosen at run time cannot be passed as `?`
arguments; check them against a fixed list and suppress the finding with
`# iglint: ignore[SEC001]` on that line.

## Example

```python
query = "SELECT * FROM orders WHERE line = '%s'" % line
rows = system.db.runQuery(query)
```

Use instead:

```python
rows = system.db.runPrepQuery("SELECT * FROM orders WHERE line = ?", [line])
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&SqlStringBuilding)],
    options: None,
};

struct SqlStringBuilding;

/// Functions whose first argument (`query`) is SQL text, and whether they
/// take `?` arguments.
const QUERY_FUNCTIONS: &[(&str, bool)] = &[
    ("system.db.runPrepQuery", true),
    ("system.db.runPrepStmt", true),
    ("system.db.runPrepStmtGetKey", true),
    ("system.db.runPrepUpdate", true),
    ("system.db.runQuery", false),
    ("system.db.runSFPrepUpdate", true),
    ("system.db.runSFUpdateQuery", false),
    ("system.db.runScalarPrepQuery", true),
    ("system.db.runScalarQuery", false),
    ("system.db.runUpdateQuery", false),
    ("system.db.runUpdateQueryGetKey", false),
];

impl ScriptRule for SqlStringBuilding {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Call(call) = e else { return };
            let Some(callee) = model.callee(call) else {
                return;
            };
            let Some(&(_, prepared)) = QUERY_FUNCTIONS.iter().find(|(f, _)| *f == callee) else {
                return;
            };
            let Some(query) = argument(call, "query", Some(0)) else {
                return;
            };
            let mut ranges = Vec::new();
            building(model, query, 0, &mut ranges);
            for range in ranges {
                if found.iter().any(|(r, _)| *r == range) {
                    continue;
                }
                let advice = if prepared {
                    "pass the values as `?` arguments"
                } else {
                    "use a prepared query with `?` arguments"
                };
                found.push((
                    range,
                    format!("SQL text for `{callee}` is built from non-literal values; {advice}"),
                ));
            }
        });
        found.sort_by_key(|(r, _)| r.start());
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

/// Collects the expressions that build the SQL text `expr` from
/// non-literal values.
fn building(model: &SemanticModel<'_>, expr: &Expr, depth: u32, out: &mut Vec<TextRange>) {
    if depth > MAX_DEPTH || is_fixed(model, expr) {
        return;
    }
    match expr {
        Expr::BinOp(b) if b.op == Operator::Mod => {
            if !numeric_format(model, &b.left) {
                out.push(expr.range());
            }
        }
        Expr::BinOp(b) if b.op == Operator::Add => out.push(expr.range()),
        Expr::Call(c) => {
            if let Expr::Attribute(a) = &*c.func
                && matches!(&*a.attr.id, "format" | "join")
            {
                out.push(expr.range());
            }
        }
        Expr::IfExp(i) => {
            building(model, &i.body, depth + 1, out);
            building(model, &i.orelse, depth + 1, out);
        }
        Expr::Name(n) => {
            let Some(reference) = model.reference_at(n.range) else {
                return;
            };
            for id in &reference.bindings {
                binding_building(model, *id, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn binding_building(
    model: &SemanticModel<'_>,
    id: iglint_python_semantic::BindingId,
    depth: u32,
    out: &mut Vec<TextRange>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let binding = model.binding(id);
    match binding.kind {
        BindingKind::Assignment if !binding.unpacked => {
            if let Some(value) = binding.value {
                building(model, value, depth + 1, out);
            }
        }
        BindingKind::AugAssignment => {
            let Some(Stmt::AugAssign(s)) = binding.stmt else {
                return;
            };
            if !matches!(s.op, Operator::Add | Operator::Mod) {
                return;
            }
            if s.op == Operator::Add && !is_fixed(model, &s.value) {
                out.push(s.range);
            } else if s.op == Operator::Mod && !is_fixed(model, &s.value) {
                // `q %= value`: the format string is the previous value.
                out.push(s.range);
            }
            for earlier in &binding.shadowed {
                binding_building(model, *earlier, depth + 1, out);
            }
        }
        _ => {}
    }
}

/// Whether `format` is a constant `%` format string whose conversions are
/// all numeric (`%d`, `%.2f`, ...): such values cannot carry quotes.
fn numeric_format(model: &SemanticModel<'_>, format: &Expr) -> bool {
    let Some(text) = constant_string(model, format) else {
        return false;
    };
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if c != '%' {
            continue;
        }
        if chars.peek() == Some(&'(') {
            for k in chars.by_ref() {
                if k == ')' {
                    break;
                }
            }
        }
        while chars.peek().is_some_and(|c| {
            matches!(
                c,
                '-' | '+' | ' ' | '#' | '0'..='9' | '.' | '*' | 'h' | 'l' | 'L'
            )
        }) {
            chars.next();
        }
        match chars.next() {
            Some('%') => {}
            Some('d' | 'i' | 'o' | 'u' | 'x' | 'X' | 'e' | 'E' | 'f' | 'F' | 'g' | 'G') => {
                any = true;
            }
            _ => return false,
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    fn flagged_in(code: &str) -> Vec<String> {
        flagged(&lint_module("SEC001", code))
    }

    #[test]
    fn reports_formatting_and_concatenation() {
        let code = r#"def f(line, name, ids):
    system.db.runQuery("SELECT * FROM t WHERE line = '%s'" % line)
    system.db.runUpdateQuery("DELETE FROM t WHERE name = '" + name + "'")
    system.db.runScalarQuery("SELECT {} FROM t".format(name))
    system.db.runPrepQuery("SELECT * FROM t WHERE id IN (%s)" % ",".join(ids), [])
    system.db.runPrepUpdate(query="UPDATE t SET a = 1 WHERE b = '%s'" % name)
"#;
        assert_eq!(
            flagged_in(code),
            [
                r#""SELECT * FROM t WHERE line = '%s'" % line"#,
                r#""DELETE FROM t WHERE name = '" + name + "'""#,
                r#""SELECT {} FROM t".format(name)"#,
                r#""SELECT * FROM t WHERE id IN (%s)" % ",".join(ids)"#,
                r#""UPDATE t SET a = 1 WHERE b = '%s'" % name"#,
            ]
        );
    }

    #[test]
    fn follows_assignments_and_aliases() {
        let code = r#"db = system.db
def f(line, extra):
    q = "SELECT * FROM t WHERE line = '%s'" % line
    db.runQuery(q)
    p = "SELECT * FROM t WHERE 1 = 1"
    if extra:
        p += " AND name = '" + extra + "'"
    db.runPrepQuery(p, [])
"#;
        let d = lint_module("SEC001", code);
        assert_eq!(
            lines(&d),
            [
                (
                    3,
                    r#""SELECT * FROM t WHERE line = '%s'" % line"#.to_owned()
                ),
                (7, r#"p += " AND name = '" + extra + "'""#.to_owned()),
            ]
        );
        assert!(d[0].message.contains("`system.db.runQuery`"));
        assert!(d[1].message.contains("`?` arguments"));
    }

    #[test]
    fn from_import_of_system_function() {
        let code = "from system.db import runUpdateQuery as upd\ndef f(x):\n    upd('DELETE FROM t WHERE a = ' + x)\n";
        assert_eq!(flagged_in(code), ["'DELETE FROM t WHERE a = ' + x"]);
    }

    #[test]
    fn literal_text_is_fine() {
        let code = r#"TABLE = "orders"
BASE = "SELECT * FROM " + TABLE
def f(ids, n, q):
    system.db.runQuery("SELECT * " + "FROM t")
    system.db.runQuery(BASE + " WHERE 1 = 1")
    system.db.runPrepQuery("SELECT * FROM t WHERE id IN (%s)" % ",".join(["?"] * len(ids)), ids)
    system.db.runPrepQuery("SELECT * FROM t WHERE id IN (%s)" % ", ".join("?" for i in ids), ids)
    system.db.runQuery("SELECT TOP %d * FROM t" % n)
    system.db.runQuery("SELECT * FROM t WHERE id = %i AND v > %.2f" % (n, n))
    system.db.runQuery(q)
    system.db.runPrepQuery("SELECT * FROM t WHERE a = ?", [q])
    system.db.runQuery("SELECT * FROM t WHERE id = %s" % len(ids))
    idList = ",".join([str(int(i)) for i in ids])
    system.db.runQuery("DELETE FROM t WHERE id IN ({0})".format(idList))
    pos = int(n)
    while pos < 10:
        system.db.runQuery("DELETE FROM t WHERE a = {0}".format(pos))
        pos += 2
    other.runQuery("SELECT %s" % q)
"#;
        assert!(flagged_in(code).is_empty(), "{:?}", flagged_in(code));
    }

    #[test]
    fn numeric_conversions_only_with_literal_format() {
        let code = "def f(fmt, n):\n    system.db.runQuery(fmt % n)\n    system.db.runQuery('SELECT %d, %s' % (n, n))\n";
        assert_eq!(flagged_in(code), ["fmt % n", "'SELECT %d, %s' % (n, n)"]);
        let code = "def f(ids, q):\n    s = ','.join([str(i) for i in ids])\n    system.db.runQuery('SELECT %s' % s)\n    p = 1\n    p += q\n    system.db.runQuery('SELECT %s' % p)\n";
        assert_eq!(flagged_in(code), ["'SELECT %s' % s", "'SELECT %s' % p"]);
    }

    #[test]
    fn shadowed_system_is_not_the_api() {
        let code = "def f(system, x):\n    system.db.runQuery('SELECT ' + x)\n";
        assert!(flagged_in(code).is_empty());
    }

    #[test]
    fn everywhere_and_json_ranges() {
        assert_eq!(
            flagged_everywhere(
                "SEC001",
                "\tq = \"SELECT * FROM t WHERE a = '%s'\" % self.custom.a\n\tsystem.db.runQuery(q)"
            ),
            ["\"SELECT * FROM t WHERE a = '%s'\" % self.custom.a"]
        );
        let f = gateway_message(
            "def handleMessage(payload):\n\tsystem.db.runUpdateQuery(\"DELETE FROM t WHERE a = '%s'\" % payload['a'])\n",
        );
        assert_eq!(
            flagged(&f.lint(&["SEC001"])),
            ["\"DELETE FROM t WHERE a = '%s'\" % payload['a']"]
        );
    }
}
