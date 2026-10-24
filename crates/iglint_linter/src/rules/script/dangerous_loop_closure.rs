//! SCR120 dangerous-loop-closure.

use iglint_python::ast::{Comprehension, Expr, Stmt};
use iglint_python_semantic::{Resolution, ScopeId, ScopeKind, SemanticModel};
use iglint_text::{Ranged, TextRange};
use rustc_hash::FxHashSet;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic, walk_exprs, walk_stmts};

pub static META: RuleMeta = RuleMeta {
    code: "SCR120",
    name: "dangerous-loop-closure",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: false,
    summary: "Function defined in a loop uses the loop's variable",
    explanation: r#"## What it does

Reports names read by a `lambda` or nested `def` created inside a `for`
loop when the name is a variable the loop rebinds (its target, a name
assigned in its body or the target of an `except` clause in it), and
names read by a `lambda` created in a comprehension when the name is one
of its variables (a list comprehension binds them in the enclosing
scope, the other kinds in their own; either way they are rebound on
every iteration).

Not reported: a function that cannot outlive the iteration that creates
it, so that it reads that iteration's value: a `lambda` called where it
is written (`(lambda: u)()`), a `lambda` passed directly to the builtin
`sorted`, `min`, `max`, `map`, `filter` or `reduce` (which call it only
while they run: `[sorted(rows, key=lambda r: r[c]) for c in cols]`), and
a `def` (or a name bound to a `lambda`) whose every use is a call later
in the same block of the loop body, in the same scope. A use before the
definition, any other use (stored, passed, returned), a decorator, a
call from a nested function or a closure it returns still count. So does
a generator: calling a generator function (a `def` or `lambda` with
`yield`) runs none of its body, and a generator expression in such a
function runs when it is consumed, unless it is passed directly to a
builtin that consumes it (`sum`, `any`, `all`, `list`, `sorted`, `min`,
`max`, `''.join`, ...).

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

The same in a comprehension (Jython: `[2, 2, 2]`):

```python
handlers = [lambda: k for k in range(3)]
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

/// Names a loop rebinds in its own scope: the target and names assigned
/// in the body, `except` targets included.
fn loop_names<'a>(target: &'a Expr, body: &'a [Stmt]) -> FxHashSet<&'a str> {
    let mut names = FxHashSet::default();
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
                        // `except ValueError, error:` rebinds `error` on
                        // every iteration that catches.
                        if let Some(name) = &h.name {
                            targets(name, out);
                        }
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

/// Builtins that call a function argument only during the call and keep
/// neither it nor what it reads (Jython 2.7: `map`, `filter` and
/// `reduce` return lists or values; `sorted`, `min` and `max` call `key`
/// and `cmp` while they run).
const CALLING_BUILTINS: &[&str] = &["filter", "map", "max", "min", "reduce", "sorted"];

/// Builtins that consume an iterable argument during the call (a
/// generator expression passed to them is run to its end, or abandoned,
/// before the call returns). `enumerate`, `iter` and `reversed` keep it.
const CONSUMING_BUILTINS: &[&str] = &[
    "all",
    "any",
    "dict",
    "filter",
    "frozenset",
    "list",
    "map",
    "max",
    "min",
    "reduce",
    "set",
    "sorted",
    "sum",
    "tuple",
    "zip",
];

/// Adds the ranges of the lambdas and generator expressions that the call
/// `e` (when it is one) passes directly to a builtin that runs them before
/// it returns (review round 25, N1: `[sorted(rows, key=lambda r: r[c]) for
/// c in cols]` sorts by each `c`): lambdas to [`CALLING_BUILTINS`],
/// generator expressions to [`CONSUMING_BUILTINS`] and to `join` on a
/// string literal. The callee must be the builtin itself: a name that
/// resolves to no binding.
fn run_during_call(model: &SemanticModel<'_>, e: &Expr, out: &mut FxHashSet<TextRange>) {
    let Expr::Call(c) = e else { return };
    let (calling, consuming) = match &*c.func {
        Expr::Name(n) => {
            let builtin = model
                .reference_at(n.range)
                .is_some_and(|r| r.resolution == Resolution::Builtin);
            (
                builtin && CALLING_BUILTINS.contains(&&*n.id),
                builtin && CONSUMING_BUILTINS.contains(&&*n.id),
            )
        }
        Expr::Attribute(a) => (
            false,
            a.attr.as_str() == "join" && matches!(&*a.value, Expr::Str(_)),
        ),
        _ => (false, false),
    };
    for a in c.args.iter().chain(c.keywords.iter().map(|k| &k.value)) {
        match a {
            Expr::Lambda(l) if calling => {
                out.insert(l.range);
            }
            Expr::GeneratorExp(_) if consuming => {
                out.insert(a.range());
            }
            _ => {}
        }
    }
}

/// Which of `lambdas` and `generators` (with their scopes) run later than
/// the iteration that creates them: a lambda unless it is called where it
/// is written, passed to a builtin that runs it during the call
/// (`during`), or `contained` ([`contained_closures`]), and not a
/// generator (a `yield` makes even a called lambda return a generator); a
/// generator expression in the body of such an exempt function unless a
/// builtin consumes it where it is written.
fn deferred(
    model: &SemanticModel<'_>,
    lambdas: &[&iglint_python::ast::ExprLambda],
    generators: &[&Expr],
    called: &FxHashSet<TextRange>,
    during: &FxHashSet<TextRange>,
    contained: &FxHashSet<TextRange>,
    exempt: &mut Vec<TextRange>,
) -> Vec<ScopeId> {
    let mut out = Vec::new();
    for l in lambdas {
        let Some(id) = model.lambda_scope(l) else {
            continue;
        };
        let runs_now = !model.scope(id).is_generator
            && (called.contains(&l.range)
                || during.contains(&l.range)
                || contained.contains(&l.range));
        if runs_now {
            exempt.push(l.range);
        } else {
            out.push(id);
        }
    }
    for e in generators {
        if !during.contains(&e.range())
            && exempt.iter().any(|r| r.contains_range(e.range()))
            && let Some(id) = model.generator_scope(e)
        {
            out.push(id);
        }
    }
    out
}

/// Scopes of the lambdas and functions created in `body` (at any depth),
/// except those that cannot outlive the iteration that creates them: a
/// lambda called where it is written (`(lambda: u)()`) or passed to a
/// builtin that calls it only while it runs ([`run_during_call`]), and a
/// `def` (or a name bound to a lambda) whose every use is a call later in
/// the same block, in the scope that holds the loop
/// ([`called_in_iteration`]). Calling a generator function does not run
/// its body, so such a function is never exempt, and the generator
/// expressions in the body of an exempt function run when they are
/// consumed: they count, unless a builtin consumes them where they are
/// written (review round 25, Codex: `def h(): yield t` called as
/// `out.append(h())`, or `lambda: (t for _ in [0])` called at once, gives
/// every generator the last value).
fn closures(model: &SemanticModel<'_>, loop_scope: ScopeId, body: &[Stmt]) -> Vec<ScopeId> {
    let mut called = FxHashSet::default();
    let mut during = FxHashSet::default();
    let mut lambdas = Vec::new();
    let mut generators = Vec::new();
    walk_exprs(body, &mut |e| {
        match e {
            Expr::Call(c) => {
                if let Expr::Lambda(l) = &*c.func {
                    called.insert(l.range);
                }
            }
            Expr::Lambda(l) => lambdas.push(l),
            Expr::GeneratorExp(_) => generators.push(e),
            _ => {}
        }
        run_during_call(model, e, &mut during);
    });
    let mut contained = FxHashSet::default();
    contained_closures(model, loop_scope, body, &mut contained);
    let mut exempt: Vec<TextRange> = Vec::new();
    let mut out = Vec::new();
    walk_stmts(body, &mut |st| {
        if let Stmt::FunctionDef(f) = st
            && let Some(id) = model.function_scope(f)
        {
            if contained.contains(&f.range) && !model.scope(id).is_generator {
                exempt.push(f.range);
            } else {
                out.push(id);
            }
        }
    });
    out.extend(deferred(
        model,
        &lambdas,
        &generators,
        &called,
        &during,
        &contained,
        &mut exempt,
    ));
    out.sort();
    out.dedup();
    out
}

/// The ranges of the `def` statements and lambdas of `block` (and of the
/// blocks of its compound statements, not inside functions or classes)
/// bound to a name that is only called later in the same block
/// ([`called_in_iteration`]).
fn contained_closures(
    model: &SemanticModel<'_>,
    loop_scope: ScopeId,
    block: &[Stmt],
    out: &mut FxHashSet<TextRange>,
) {
    let nested = |body: &[Stmt], out: &mut FxHashSet<TextRange>| {
        contained_closures(model, loop_scope, body, out);
    };
    for (i, s) in block.iter().enumerate() {
        let later = block.get(i + 1..).unwrap_or_default();
        match s {
            Stmt::FunctionDef(f) => {
                if f.decorator_list.is_empty()
                    && called_in_iteration(model, loop_scope, &f.name.id, later)
                {
                    out.insert(f.range);
                }
            }
            Stmt::Assign(a) => {
                if let ([Expr::Name(n)], Expr::Lambda(l)) = (&a.targets[..], &*a.value)
                    && called_in_iteration(model, loop_scope, &n.id, later)
                {
                    out.insert(l.range);
                }
            }
            Stmt::For(f) => {
                nested(&f.body, out);
                nested(&f.orelse, out);
            }
            Stmt::While(w) => {
                nested(&w.body, out);
                nested(&w.orelse, out);
            }
            Stmt::If(x) => {
                nested(&x.body, out);
                nested(&x.orelse, out);
            }
            Stmt::With(w) => nested(&w.body, out),
            Stmt::TryExcept(t) => {
                nested(&t.body, out);
                for h in &t.handlers {
                    nested(&h.body, out);
                }
                nested(&t.orelse, out);
            }
            Stmt::TryFinally(t) => {
                nested(&t.body, out);
                nested(&t.finalbody, out);
            }
            _ => {}
        }
    }
}

/// Whether the function bound to `name` in `loop_scope` (by the statement
/// just before `later`, in the same block) can only run in the iteration
/// that creates it: the scope binds `name` once (that statement), does not
/// declare it `global`, and every reference that may read the binding is
/// the callee of a call in `later`, in `loop_scope` itself (not in a
/// nested function or lambda, which may run later). A use before the
/// statement reads the previous iteration's function, and any other use
/// (stored, passed, returned, called from another scope) lets it outlive
/// the iteration.
fn called_in_iteration(
    model: &SemanticModel<'_>,
    loop_scope: ScopeId,
    name: &str,
    later: &[Stmt],
) -> bool {
    let scope = model.scope(loop_scope);
    if scope.bindings_of(name).len() != 1 || scope.globals.contains_key(name) {
        return false;
    }
    let mut calls = FxHashSet::default();
    walk_exprs(later, &mut |e| {
        if let Expr::Call(c) = e
            && let Expr::Name(n) = &*c.func
        {
            calls.insert(n.range);
        }
    });
    model.references().all(|(_, r)| {
        if r.name != name || r.is_implicit() {
            return true;
        }
        if r.scope == loop_scope {
            return calls.contains(&r.range);
        }
        // A reference in another scope may read this binding unless it
        // resolves to a binding of its own scope or of a scope other than
        // the loop's.
        match r.resolution {
            Resolution::Free(owner) => owner != loop_scope,
            Resolution::Global => loop_scope != ScopeId::MODULE,
            Resolution::Dynamic => false,
            _ => true,
        }
    })
}

/// The innermost scope whose code holds `range`.
fn holding_scope(model: &SemanticModel<'_>, range: TextRange) -> Option<ScopeId> {
    model
        .scopes()
        .filter(|(_, sc)| scope_range(sc).contains_range(range))
        .max_by_key(|(id, _)| depth(model, *id))
        .map(|(id, _)| id)
}

/// Reports the reads in the closures `inner` of `names` that resolve to
/// the namespace `scope` rebinds them in: an enclosing function, or the
/// module (a loop at module level, or a name the scope declares
/// `global`).
fn report_reads(
    model: &SemanticModel<'_>,
    scope: ScopeId,
    names: &FxHashSet<&str>,
    inner: &[ScopeId],
    found: &mut Vec<(TextRange, String)>,
) {
    if inner.is_empty() {
        return;
    }
    let globals = &model.scope(scope).globals;
    for (_, r) in model.references() {
        let owner = match r.resolution {
            Resolution::Free(owner) => owner,
            Resolution::Global => ScopeId::MODULE,
            _ => continue,
        };
        let rebinds = if globals.contains_key(r.name) {
            ScopeId::MODULE
        } else {
            scope
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
}

/// The lambdas of a comprehension that are created on every iteration (in
/// the element, the conditions and the iterables after the first, which
/// is evaluated once, before the loop), except those called where they
/// are written.
fn comprehension_closures(
    model: &SemanticModel<'_>,
    parts: &[&Expr],
    generators: &[Comprehension],
) -> Vec<ScopeId> {
    let mut exprs: Vec<&Expr> = parts.to_vec();
    for (i, g) in generators.iter().enumerate() {
        if i > 0 {
            exprs.push(&g.iter);
        }
        exprs.extend(g.ifs.iter());
    }
    let mut called = FxHashSet::default();
    let mut during = FxHashSet::default();
    let mut lambdas = Vec::new();
    let mut nested = Vec::new();
    for e in exprs {
        walk_expr_tree(e, &mut |e| {
            match e {
                Expr::Call(c) => {
                    if let Expr::Lambda(l) = &*c.func {
                        called.insert(l.range);
                    }
                }
                Expr::Lambda(l) => lambdas.push(l),
                Expr::GeneratorExp(_) => nested.push(e),
                _ => {}
            }
            run_during_call(model, e, &mut during);
        });
    }
    let mut out = deferred(
        model,
        &lambdas,
        &nested,
        &called,
        &during,
        &FxHashSet::default(),
        &mut Vec::new(),
    );
    out.sort();
    out.dedup();
    out
}

/// Calls `f` for `e` and every expression in it.
fn walk_expr_tree<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    use iglint_python::visitor::{Visitor, walk_expr};
    struct V<'f, 'a>(&'f mut dyn FnMut(&'a Expr));
    impl<'a> Visitor<'a> for V<'_, 'a> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            (self.0)(expr);
            walk_expr(self, expr);
        }
    }
    V(f).visit_expr(e);
}

impl ScriptRule for DangerousLoopClosure {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let body = &unit.parsed.module.body;
        let mut found: Vec<(TextRange, String)> = Vec::new();
        walk_stmts(body, &mut |s| {
            let Stmt::For(f) = s else { return };
            // A loop in a class body binds class attributes, which no
            // closure reads by name.
            let Some(loop_scope) = holding_scope(model, f.range) else {
                return;
            };
            if model.scope(loop_scope).kind == ScopeKind::Class {
                return;
            }
            let names = loop_names(&f.target, &f.body);
            let inner = closures(model, loop_scope, &f.body);
            report_reads(model, loop_scope, &names, &inner, &mut found);
        });
        // Comprehensions are loops too: a Python 2 list comprehension
        // binds its variables in the enclosing scope, the other kinds in
        // their own, and either way every lambda created in one reads the
        // last value when it runs (Jython prints `[2, 2, 2]` for each).
        walk_exprs(body, &mut |e| {
            let (parts, generators, scope): (Vec<&Expr>, _, _) = match e {
                Expr::ListComp(c) => (vec![&*c.elt], &c.generators, holding_scope(model, c.range)),
                Expr::GeneratorExp(c) => (vec![&*c.elt], &c.generators, model.generator_scope(e)),
                Expr::SetComp(c) => (vec![&*c.elt], &c.generators, model.generator_scope(e)),
                Expr::DictComp(c) => (
                    vec![&*c.key, &*c.value],
                    &c.generators,
                    model.generator_scope(e),
                ),
                _ => return,
            };
            let Some(scope) = scope else { return };
            if model.scope(scope).kind == ScopeKind::Class {
                return;
            }
            let mut names = FxHashSet::default();
            for g in generators {
                targets(&g.target, &mut names);
            }
            let inner = comprehension_closures(model, &parts, generators);
            report_reads(model, scope, &names, &inner, &mut found);
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

    /// Review round 24 (recall): comprehensions are loops (a Python 2 list
    /// comprehension binds its variable in the enclosing scope, the other
    /// kinds in their own), and an `except` target is rebound on every
    /// iteration. Jython prints `[2, 2, 2]` for every comprehension kind
    /// and `[1, 1]` for the handler.
    #[test]
    fn comprehensions_and_handlers_rebind() {
        for (code, names) in [
            ("hs = [lambda: k for k in range(3)]\n", &["k"][..]),
            (
                "def f():\n    return [lambda: k for k in range(3)]\n",
                &["k"][..],
            ),
            ("gs = list(lambda: j for j in range(3))\n", &["j"][..]),
            ("ss = dict((n, lambda: n) for n in range(3))\n", &["n"][..]),
            ("ds = {m: (lambda: m) for m in range(3)}\n", &["m"][..]),
            ("st = {(lambda: q) for q in range(3)}\n", &["q"][..]),
            (
                "hs = [lambda: (a, b) for a in range(2) for b in range(2)]\n",
                &["a", "b"][..],
            ),
            (
                "callbacks = []\nfor i in [0, 1]:\n    try:\n        raise ValueError(i)\n    except ValueError, error:\n        callbacks.append(lambda: error.args[0])\n",
                &["error"][..],
            ),
            (
                "def f(xs):\n    out = []\n    for x in xs:\n        try:\n            g(x)\n        except Exception as e:\n            out.append(lambda: e)\n    return out\n",
                &["e"][..],
            ),
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), names, "{code}");
        }
        for code in [
            "hs = [lambda k=k: k for k in range(3)]\n",
            // Called where it is written: it runs in its own iteration.
            "vs = [(lambda: k)() for k in range(3)]\n",
            // The first iterable is evaluated once, before the loop.
            "vs = [x for x in (lambda: k)()]\nk = 1\n",
            // The variable of the comprehension is not what the lambda reads.
            "hs = [lambda: other for k in range(3)]\n",
            "class C:\n    hs = [lambda: 1 for k in range(3)]\n",
        ] {
            assert!(library("m", code).lint(&["SCR120"]).is_empty(), "{code}");
        }
    }

    /// Review round 24 (precision): a closure that runs in the iteration
    /// that creates it reads the value of that iteration (Jython prints
    /// `[0, 1, 2]`): a lambda called where it is written, and a `def` or a
    /// name bound to a lambda that is only called later in the same
    /// block. A use before the definition, any other use, a decorator, a
    /// call from a nested function or a closure it returns still report.
    #[test]
    fn closures_called_in_their_iteration() {
        for code in [
            "def f(r):\n    for u in r:\n        print (lambda: u)()\n",
            "def f(r):\n    out = []\n    for t in r:\n        def h():\n            return t\n        out.append(h())\n    return out\n",
            "out = []\nfor t in range(3):\n    def h():\n        return t\n    out.append(h())\n",
            "def f(r):\n    for t in r:\n        g = lambda: t\n        if t:\n            print g()\n",
            "def f(r):\n    for t in r:\n        def h():\n            return t\n        for i in range(2):\n            h()\n",
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert!(d.is_empty(), "{code}: {:?}", flagged(&d));
        }
        for (code, names) in [
            // Stored: it outlives the iteration.
            (
                "def f(r):\n    out = []\n    for t in r:\n        def h():\n            return t\n        h()\n        out.append(h)\n    return out\n",
                &["t"][..],
            ),
            // Called before it is defined: the previous iteration's function.
            (
                "def f(r):\n    for t in r:\n        if t > 1:\n            h()\n        def h():\n            return t\n",
                &["t"][..],
            ),
            // Called in a nested lambda, which may run later.
            (
                "def f(r):\n    for t in r:\n        def h():\n            return t\n        later(lambda: h())\n",
                &["t"][..],
            ),
            // A decorator receives it.
            (
                "def f(r):\n    for t in r:\n        @register\n        def h():\n            return t\n        h()\n",
                &["t"][..],
            ),
            // At module level another function may call it later.
            (
                "for t in range(3):\n    def h():\n        return t\n    h()\ndef later():\n    return h()\n",
                &["t"][..],
            ),
            // It returns a closure that outlives the iteration.
            (
                "def f(r):\n    out = []\n    for t in r:\n        def h():\n            return lambda: t\n        out.append(h())\n    return out\n",
                &["t"][..],
            ),
            // A lambda passed on.
            (
                "def f(r):\n    for u in r:\n        later(lambda: u)\n",
                &["u"][..],
            ),
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), names, "{code}");
        }
    }

    /// Review round 25 (Codex): calling a generator function runs none of
    /// its body, and a generator expression runs when it is consumed, so
    /// a generator made in an iteration reads the loop's last value
    /// (Jython: `[[2], [2], [2]]` for each of these).
    #[test]
    fn generators_run_after_the_iteration() {
        for code in [
            "out = []\nfor t in range(3):\n    def h():\n        yield t\n    out.append(h())\nprint [list(g) for g in out]\n",
            "out = []\nfor t in range(3):\n    f = lambda: (t for _ in [0])\n    out.append(f())\n",
            "out = []\nfor t in range(3):\n    out.append((lambda: (t for _ in [0]))())\n",
            "def f(r):\n    out = []\n    for t in r:\n        def h():\n            return (t for _ in [0])\n        out.append(h())\n    return out\n",
            "out = [(lambda: (t for _ in [0]))() for t in range(3)]\n",
            "out = []\nfor t in range(3):\n    out.append((lambda: (yield t))())\n",
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), ["t"], "{code}");
        }
        // Consumed where it is written: each iteration's value
        // (Jython: `[3, 6]`).
        let code = "res = []\nfor t in (1, 2):\n    res.append((lambda: sum(x * t for x in [1, 2]))())\n    res.append((lambda: ','.join(str(x * t) for x in [1]))())\n";
        assert!(library("m", code).lint(&["SCR120"]).is_empty());
    }

    /// Review round 25 (Opus, N1): Jython 2.7's `sorted`, `min`, `max`,
    /// `map`, `filter` and `reduce` call a function argument only while
    /// they run and keep neither it nor its reads, so a lambda passed to
    /// one directly uses each iteration's value (Jython:
    /// `[[(1, 'b'), (2, 'a')], [(2, 'a'), (1, 'b')]]` for the first). Not
    /// when the name is not the builtin, or the function may keep it.
    #[test]
    fn lambdas_run_by_builtins_use_their_iteration() {
        let setup = "rows = [(1, 'b'), (2, 'a')]\ncols = (0, 1)\nkeys = (1, 2)\n";
        for code in [
            "x = [sorted(rows, key=lambda r: r[c]) for c in cols]\n",
            "x = [filter(lambda r: r[0] == k, rows) for k in keys]\n",
            "x = [map(lambda r: r[c], rows) for c in cols]\n",
            "x = [reduce(lambda a, b: a + b[0] * c, rows, 0) for c in cols]\n",
            "x = [min(rows, key=lambda r: r[c]) for c in cols]\n",
            "x = [sorted(rows, lambda a, b: cmp(a[c], b[c])) for c in cols]\n",
            "res = []\nfor c in cols:\n    res.append(max(rows, key=lambda r: r[c]))\n",
        ] {
            let code = format!("{setup}{code}");
            assert!(library("m", &code).lint(&["SCR120"]).is_empty(), "{code}");
        }
        for code in [
            "def keep(a, key):\n    return key\nres = [keep(rows, key=lambda r: r[c]) for c in cols]\n",
            "sorted = lambda a, key: key\nres = [sorted(rows, key=lambda r: r[c]) for c in cols]\n",
            "res = [(lambda: enumerate(r[c] for r in rows))() for c in cols]\n",
        ] {
            let code = format!("{setup}{code}");
            assert!(!library("m", &code).lint(&["SCR120"]).is_empty(), "{code}");
        }
    }

    #[test]
    fn default_arguments_capture() {
        let code = "def f(paths):\n    for p in paths:\n        system.util.invokeLater(lambda p=p: g(p))\n";
        assert!(library("m", code).lint(&["SCR120"]).is_empty());
    }
}
