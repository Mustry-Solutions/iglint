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
            // The innermost scope whose code holds the loop. A loop in a
            // class body binds class attributes, which no closure reads
            // by name.
            let Some(loop_scope) = model
                .scopes()
                .filter(|(_, sc)| scope_range(sc).contains_range(f.range))
                .max_by_key(|(id, _)| depth(model, *id))
                .map(|(id, _)| id)
            else {
                return;
            };
            if model.scope(loop_scope).kind == iglint_python_semantic::ScopeKind::Class {
                return;
            }
            let inner = closures(model, &f.body);
            let globals = &model.scope(loop_scope).globals;
            for (_, r) in model.references() {
                // The namespace the closure reads the name from: an
                // enclosing function, or the module (a loop at module
                // level, or in a function that declares the name
                // `global`).
                let owner = match r.resolution {
                    Resolution::Free(owner) => owner,
                    Resolution::Global => ScopeId::MODULE,
                    _ => continue,
                };
                let rebinds = if globals.contains_key(r.name) {
                    ScopeId::MODULE
                } else {
                    loop_scope
                };
                if owner != rebinds || !names.contains(r.name) {
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

/// The source range of a scope's node.
fn scope_range(scope: &iglint_python_semantic::Scope<'_>) -> TextRange {
    match scope.node {
        iglint_python_semantic::ScopeNode::Module(m) => m.range,
        iglint_python_semantic::ScopeNode::Function(f) => f.range,
        iglint_python_semantic::ScopeNode::Lambda(l) => l.range,
        iglint_python_semantic::ScopeNode::Generator(g) => g.range(),
        iglint_python_semantic::ScopeNode::Class(c) => c.range,
    }
}

/// How many scopes enclose `scope`.
fn depth(model: &SemanticModel<'_>, mut scope: ScopeId) -> usize {
    let mut n = 0;
    while let Some(p) = model.scope(scope).parent {
        scope = p;
        n += 1;
    }
    n
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

    /// Review round 23 (Codex): a loop at module level rebinds a module
    /// global, which closures read when they run (Jython prints `[2, 2]`).
    #[test]
    fn reports_late_binding_of_module_globals() {
        for (code, names) in [
            (
                "callbacks = []\nfor value in [1, 2]:\n    callbacks.append(lambda: value)\nprint [callback() for callback in callbacks]\n",
                &["value"][..],
            ),
            (
                "handlers = {}\nfor key, n in [('a', 1), ('b', 2)]:\n    def h():\n        return n\n    handlers[key] = h\n",
                &["n"][..],
            ),
            (
                "def f(xs):\n    global v\n    for v in xs:\n        system.util.invokeLater(lambda: g(v))\n",
                &["v"][..],
            ),
            (
                "class C(object):\n    def m(self, xs):\n        for v in xs:\n            system.util.invokeLater(lambda: g(v))\n",
                &["v"][..],
            ),
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), names, "{code}");
        }
        for code in [
            "callbacks = []\nfor value in [1, 2]:\n    callbacks.append(lambda value=value: value)\n",
            "for value in [1, 2]:\n    def h(value=value):\n        return value\n",
            // The closure reads another name, or the loop is in a class
            // body (its names are class attributes, not what the lambda
            // reads), or a function's own local shadows the global.
            "for value in [1, 2]:\n    f = lambda: other\n",
            "class C:\n    for value in [1, 2]:\n        f = lambda: value\n",
            "def f(xs):\n    for v in xs:\n        g(lambda v=v: v)\nv = 1\n",
        ] {
            assert!(library("m", code).lint(&["SCR120"]).is_empty(), "{code}");
        }
    }

    #[test]
    fn default_arguments_capture() {
        let code = "def f(paths):\n    for p in paths:\n        system.util.invokeLater(lambda p=p: g(p))\n";
        assert!(library("m", code).lint(&["SCR120"]).is_empty());
    }
}
