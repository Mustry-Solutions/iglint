//! SCR120 dangerous-loop-closure.

use iglint_python::ast::{Expr, Stmt};
use iglint_python_semantic::{Resolution, ScopeId, SemanticModel};
use iglint_text::{Ranged, TextRange};
use rustc_hash::FxHashSet;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR120",
    name: "dangerous-loop-closure",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Function defined in a loop uses the loop's variable",
    explanation: r#"## What it does

Reports names read by a `lambda` or nested `def` created inside a `for`
loop when the name is a variable the loop rebinds (its target or a name
assigned in its body).

## Why is this bad?

The function reads the variable when it is called, not when it is
created. Functions called after the loop (callbacks given to
`system.util.invokeLater`, handlers stored in a dict) all see the last
value.

## Example

```python
for path in paths:
    system.util.invokeLater(lambda: system.tag.writeBlocking([path], [0]))
```

Use instead:

```python
for path in paths:
    system.util.invokeLater(lambda path=path: system.tag.writeBlocking([path], [0]))
```

## pylint

Corresponds to `W0640 cell-var-from-loop`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&DangerousLoopClosure)],
    options: None,
};

struct DangerousLoopClosure;

/// Names a loop rebinds in its own scope: the target and names assigned
/// in the body.
fn loop_names<'a>(target: &'a Expr, body: &'a [Stmt]) -> FxHashSet<&'a str> {
    let mut names = FxHashSet::default();
    fn targets<'a>(e: &'a Expr, out: &mut FxHashSet<&'a str>) {
        match e {
            Expr::Name(n) => {
                out.insert(&n.id);
            }
            Expr::Tuple(t) => t.elts.iter().for_each(|e| targets(e, out)),
            Expr::List(t) => t.elts.iter().for_each(|e| targets(e, out)),
            _ => {}
        }
    }
    targets(target, &mut names);
    fn body_names<'a>(body: &'a [Stmt], out: &mut FxHashSet<&'a str>) {
        for s in body {
            match s {
                Stmt::Assign(a) => a.targets.iter().for_each(|t| targets(t, out)),
                Stmt::AugAssign(a) => targets(&a.target, out),
                Stmt::For(f) => {
                    targets(&f.target, out);
                    body_names(&f.body, out);
                    body_names(&f.orelse, out);
                }
                Stmt::While(w) => {
                    body_names(&w.body, out);
                    body_names(&w.orelse, out);
                }
                Stmt::If(i) => {
                    body_names(&i.body, out);
                    body_names(&i.orelse, out);
                }
                Stmt::With(w) => {
                    if let Some(v) = &w.optional_vars {
                        targets(v, out);
                    }
                    body_names(&w.body, out);
                }
                Stmt::TryExcept(t) => {
                    body_names(&t.body, out);
                    for h in &t.handlers {
                        body_names(&h.body, out);
                    }
                    body_names(&t.orelse, out);
                }
                Stmt::TryFinally(t) => {
                    body_names(&t.body, out);
                    body_names(&t.finalbody, out);
                }
                _ => {}
            }
        }
    }
    body_names(body, &mut names);
    names
}

/// Scopes of the lambdas and functions created directly in `body` (not
/// inside other nested scopes).
fn closures(model: &SemanticModel<'_>, body: &[Stmt]) -> Vec<ScopeId> {
    let mut out = Vec::new();
    for s in body {
        walk_stmts(std::slice::from_ref(s), &mut |st| {
            if let Stmt::FunctionDef(f) = st
                && let Some(id) = model.function_scope(f)
            {
                out.push(id);
            }
        });
        crate::rules::script::walk_exprs(std::slice::from_ref(s), &mut |e| {
            if let Expr::Lambda(l) = e
                && let Some(id) = model.lambda_scope(l)
            {
                out.push(id);
            }
        });
    }
    out.sort();
    out.dedup();
    out
}

impl ScriptRule for DangerousLoopClosure {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_stmts(&unit.parsed.module.body, &mut |s| {
            let Stmt::For(f) = s else { return };
            let names = loop_names(&f.target, &f.body);
            let Some(loop_scope) = model
                .scopes()
                .filter(|(_, sc)| sc.kind != iglint_python_semantic::ScopeKind::Class)
                .find(|(id, _)| scope_contains(model, *id, f.range))
                .map(|(id, _)| id)
            else {
                return;
            };
            let inner = closures(model, &f.body);
            for (_, r) in model.references() {
                let Resolution::Free(owner) = r.resolution else {
                    continue;
                };
                if owner != loop_scope || !names.contains(r.name) {
                    continue;
                }
                if inner.iter().any(|c| is_within(model, r.scope, *c)) {
                    found.push((
                        r.range,
                        format!(
                            "`{}` is read when the function runs, after the loop has moved on",
                            r.name
                        ),
                    ));
                }
            }
        });
        found.sort_by_key(|f| f.0.start());
        found.dedup();
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

/// Whether `scope` is `ancestor` or nested in it.
fn is_within(model: &SemanticModel<'_>, mut scope: ScopeId, ancestor: ScopeId) -> bool {
    loop {
        if scope == ancestor {
            return true;
        }
        match model.scope(scope).parent {
            Some(p) => scope = p,
            None => return false,
        }
    }
}

/// The innermost scope whose code contains `range`.
fn scope_contains(model: &SemanticModel<'_>, id: ScopeId, range: TextRange) -> bool {
    let scope = model.scope(id);
    let own = match scope.node {
        iglint_python_semantic::ScopeNode::Module(m) => m.range,
        iglint_python_semantic::ScopeNode::Function(f) => f.range,
        iglint_python_semantic::ScopeNode::Lambda(l) => l.range,
        iglint_python_semantic::ScopeNode::Generator(g) => g.range(),
        iglint_python_semantic::ScopeNode::Class(c) => c.range,
    };
    own.contains_range(range)
        && !scope.children.iter().any(|c| {
            model.scope(*c).kind != iglint_python_semantic::ScopeKind::Class
                && scope_contains(model, *c, range)
        })
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_late_binding() {
        let code = "def f(paths):\n    for p in paths:\n        q = p + '/x'\n        system.util.invokeLater(lambda: g(p, q))\n";
        let d = library("m", code).lint(&["SCR120"]);
        assert_eq!(flagged(&d), ["p", "q"]);
    }

    #[test]
    fn default_arguments_capture() {
        let code = "def f(paths):\n    for p in paths:\n        system.util.invokeLater(lambda p=p: g(p))\n";
        assert!(library("m", code).lint(&["SCR120"]).is_empty());
    }
}
