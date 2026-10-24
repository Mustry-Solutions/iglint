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

Generator expressions created in the loop count too: one reads the
loop's variables when it is consumed (in its body and its conditions;
its first iterable is evaluated where it is written).

Not reported: a function that cannot outlive the iteration that creates
it, so that it reads that iteration's value: a `lambda` called where it
is written (`(lambda: u)()`), a `lambda` passed directly to the builtin
`sorted`, `min`, `max`, `map`, `filter` or `reduce` as an argument the
builtin calls while it runs (the `cmp` and `key` of `sorted`, the `key`
of `min` and `max`, the first argument of `map`, `filter` and `reduce`:
`[sorted(rows, key=lambda r: r[c]) for c in cols]`; any other argument
is a value the builtin may return, `max(lambda: t, lambda: t)` returns a
lambda), and a `def` (or a name bound to a `lambda`) whose every use is
a call later in the same block of the loop body, in the same scope. A
use before the definition, any other use (stored, passed, returned), a
decorator, a call from a nested function or a closure it returns still
count. So does a generator: calling a generator function (a `def` or
`lambda` with `yield`) runs none of its body, and a generator expression
runs when it is consumed, unless it is consumed where it is written: as
the iterable of a builtin that consumes it (`sum`, `any`, `all`, `list`,
`tuple`, `set`, `dict`, `sorted`, a single argument of `min` or `max`,
the sequences of `map`, `filter`, `reduce` and `zip`, `''.join`), of a
`for` statement or of a list, set or dict comprehension. A generator
function whose every call is consumed that way (`total += sum(h())`)
is not reported either.

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

/// An argument of a call: by position or by keyword.
#[derive(Clone, Copy)]
enum Arg<'a> {
    At(usize),
    Keyword(&'a str),
}

/// Whether the builtin `name` calls the function it gets as `arg` only
/// while it runs, keeping neither it nor what it reads (Jython 2.7:
/// `sorted` calls `cmp` and `key`, `min` and `max` call `key`, `map`,
/// `filter` and `reduce` their first argument). Any other argument is a
/// value the call may return or keep (review round 26, Codex:
/// `max(lambda: t, lambda: t)` returns a lambda, `reduce(f, [], lambda:
/// t)` its initial value).
fn calls_argument(name: &str, arg: Arg<'_>) -> bool {
    matches!(
        (name, arg),
        ("sorted", Arg::At(1 | 2) | Arg::Keyword("cmp" | "key"))
            | ("min" | "max", Arg::Keyword("key"))
            | ("map" | "filter" | "reduce", Arg::At(0))
    )
}

/// Whether the builtin `name` consumes the iterable it gets as `arg`
/// before it returns (runs it to its end, or abandons it), with
/// `positional` positional arguments: the iterable of `all`, `any`,
/// `dict`, `frozenset`, `list`, `set`, `sorted`, `sum` and `tuple`, of
/// `min` and `max` given one positional argument (with more they compare,
/// and may return, the arguments themselves), the sequences of `filter`,
/// `reduce`, `map` and `zip`. `enumerate`, `iter` and `reversed` keep it;
/// `dict(k=g)` and `sum(x, g)` keep `g`.
fn consumes_argument(name: &str, arg: Arg<'_>, positional: usize) -> bool {
    match (name, arg) {
        (
            "all" | "any" | "dict" | "frozenset" | "list" | "set" | "sorted" | "sum" | "tuple",
            Arg::At(0),
        ) => true,
        ("min" | "max", Arg::At(0)) => positional == 1,
        ("filter" | "reduce", Arg::At(1)) => true,
        ("map", Arg::At(i)) => i >= 1,
        ("zip", Arg::At(_)) => true,
        _ => false,
    }
}

/// What a builtin runs before the call `e` (when it is one) returns
/// (review round 25, N1; round 26): adds to `invoked` the lambdas it calls
/// ([`calls_argument`]) and to `consumed` the arguments it consumes
/// ([`consumes_argument`], and the argument of `join` on a string
/// literal): generator expressions and calls of generator functions run
/// to their end there. The callee must be the builtin itself: a name that
/// resolves to no binding.
fn run_during_call(
    model: &SemanticModel<'_>,
    e: &Expr,
    invoked: &mut FxHashSet<TextRange>,
    consumed: &mut FxHashSet<TextRange>,
) {
    let Expr::Call(c) = e else { return };
    let name = match &*c.func {
        Expr::Name(n)
            if model
                .reference_at(n.range)
                .is_some_and(|r| r.resolution == Resolution::Builtin) =>
        {
            &*n.id
        }
        Expr::Attribute(a) if a.attr.as_str() == "join" && matches!(&*a.value, Expr::Str(_)) => {
            if let Some(a) = c.args.first() {
                consumed.insert(a.range());
            }
            return;
        }
        _ => return,
    };
    let args = c
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| (Arg::At(i), a))
        .chain(
            c.keywords
                .iter()
                .map(|k| (Arg::Keyword(k.arg.as_str()), &k.value)),
        );
    for (arg, value) in args {
        if let Expr::Lambda(l) = value
            && calls_argument(name, arg)
        {
            invoked.insert(l.range);
        }
        if consumes_argument(name, arg, c.args.len()) {
            consumed.insert(value.range());
        }
    }
}

/// Adds the iterables of the generator expressions of `generators` that
/// are consumed where they are written: they run when it does
/// (`sum(x for x in (v * i for v in y))`).
fn consumed_with(generators: &[&Expr], consumed: &mut FxHashSet<TextRange>) {
    loop {
        let mut changed = false;
        for e in generators {
            if let Expr::GeneratorExp(g) = e
                && consumed.contains(&e.range())
            {
                for c in &g.generators {
                    changed |= consumed.insert(c.iter.range());
                }
            }
        }
        if !changed {
            return;
        }
    }
}

/// Adds the iterables that a statement or a comprehension of `body`
/// consumes where it is written: the iterable of a `for` statement and
/// every iterable of a list, set or dict comprehension (a generator
/// expression is lazy, its own iterables are consumed when it is).
fn consumed_in_place(stmts: &[Stmt], exprs: &[&Expr], consumed: &mut FxHashSet<TextRange>) {
    walk_stmts(stmts, &mut |st| {
        if let Stmt::For(f) = st {
            consumed.insert(f.iter.range());
        }
    });
    let mut comprehension = |e: &Expr| {
        let generators = match e {
            Expr::ListComp(c) => &c.generators,
            Expr::SetComp(c) => &c.generators,
            Expr::DictComp(c) => &c.generators,
            _ => return,
        };
        for g in generators {
            consumed.insert(g.iter.range());
        }
    };
    walk_exprs(stmts, &mut comprehension);
    for e in exprs {
        walk_expr_tree(e, &mut comprehension);
    }
}

/// Which of `lambdas` and `generators` (with their scopes) run later than
/// the iteration that creates them: a lambda unless it is called where it
/// is written, passed to a builtin that calls it during the call
/// (`invoked`), or `contained` ([`contained_closures`]), and not a
/// generator (a `yield` makes even a called lambda return a generator); a
/// generator expression unless it is consumed where it is written
/// (`consumed`: by a builtin, a `for` statement or a comprehension).
fn deferred(
    model: &SemanticModel<'_>,
    lambdas: &[&iglint_python::ast::ExprLambda],
    generators: &[&Expr],
    called: &FxHashSet<TextRange>,
    invoked: &FxHashSet<TextRange>,
    consumed: &FxHashSet<TextRange>,
    contained: &FxHashSet<TextRange>,
) -> Vec<ScopeId> {
    let mut out = Vec::new();
    for l in lambdas {
        let Some(id) = model.lambda_scope(l) else {
            continue;
        };
        let runs_now = !model.scope(id).is_generator
            && (called.contains(&l.range)
                || invoked.contains(&l.range)
                || contained.contains(&l.range));
        if !runs_now {
            out.push(id);
        }
    }
    for e in generators {
        if !consumed.contains(&e.range())
            && let Some(id) = model.generator_scope(e)
        {
            out.push(id);
        }
    }
    out
}

/// Scopes of the lambdas, functions and generator expressions created in
/// `body` (at any depth), except those that cannot outlive the iteration
/// that creates them: a lambda called where it is written (`(lambda:
/// u)()`) or passed to a builtin that calls it only while it runs
/// ([`run_during_call`]), a `def` (or a name bound to a lambda) whose
/// every use is a call later in the same block, in the scope that holds
/// the loop ([`called_in_iteration`]), and a generator expression consumed
/// where it is written. Calling a generator function does not run its
/// body, so such a function counts (review round 25, Codex: `def h():
/// yield t` called as `out.append(h())` gives every generator the last
/// value) unless every call of it is consumed where it is written
/// (`total += sum(h())`, review round 26); a generator expression runs
/// when it is consumed, so one that is kept (`out.append(v * i for v in
/// y)`) counts too (review round 26).
fn closures(model: &SemanticModel<'_>, loop_scope: ScopeId, body: &[Stmt]) -> Vec<ScopeId> {
    let mut called = FxHashSet::default();
    let mut invoked = FxHashSet::default();
    let mut consumed = FxHashSet::default();
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
        run_during_call(model, e, &mut invoked, &mut consumed);
    });
    consumed_in_place(body, &[], &mut consumed);
    consumed_with(&generators, &mut consumed);
    let mut contained = Contained::default();
    contained_closures(model, loop_scope, body, &consumed, &mut contained);
    let mut out = Vec::new();
    walk_stmts(body, &mut |st| {
        if let Stmt::FunctionDef(f) = st
            && let Some(id) = model.function_scope(f)
        {
            let exempt = if model.scope(id).is_generator {
                contained.consumed.contains(&f.range)
            } else {
                contained.called.contains(&f.range)
            };
            if !exempt {
                out.push(id);
            }
        }
    });
    out.extend(deferred(
        model,
        &lambdas,
        &generators,
        &called,
        &invoked,
        &consumed,
        &contained.called,
    ));
    out.sort();
    out.dedup();
    out
}

/// What [`contained_closures`] finds.
#[derive(Default)]
struct Contained {
    /// Functions and lambdas whose every use is a call later in the same
    /// block.
    called: FxHashSet<TextRange>,
    /// Those of them whose every call is also consumed where it is written
    /// (for generator functions).
    consumed: FxHashSet<TextRange>,
}

/// The ranges of the `def` statements and lambdas of `block` (and of the
/// blocks of its compound statements, not inside functions or classes)
/// bound to a name that is only called later in the same block
/// ([`called_in_iteration`]), and of those whose calls are all in
/// `consumed`.
fn contained_closures(
    model: &SemanticModel<'_>,
    loop_scope: ScopeId,
    block: &[Stmt],
    consumed: &FxHashSet<TextRange>,
    out: &mut Contained,
) {
    let nested = |body: &[Stmt], out: &mut Contained| {
        contained_closures(model, loop_scope, body, consumed, out);
    };
    let add = |name: &str, range: TextRange, later: &[Stmt], out: &mut Contained| {
        if let Some(calls) = called_in_iteration(model, loop_scope, name, later) {
            out.called.insert(range);
            if calls.iter().all(|c| consumed.contains(c)) {
                out.consumed.insert(range);
            }
        }
    };
    for (i, s) in block.iter().enumerate() {
        let later = block.get(i + 1..).unwrap_or_default();
        match s {
            Stmt::FunctionDef(f) => {
                if f.decorator_list.is_empty() {
                    add(&f.name.id, f.range, later, out);
                }
            }
            Stmt::Assign(a) => {
                if let ([Expr::Name(n)], Expr::Lambda(l)) = (&a.targets[..], &*a.value) {
                    add(&n.id, l.range, later, out);
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
///
/// Returns the ranges of the calls when it is so.
fn called_in_iteration(
    model: &SemanticModel<'_>,
    loop_scope: ScopeId,
    name: &str,
    later: &[Stmt],
) -> Option<Vec<TextRange>> {
    let scope = model.scope(loop_scope);
    if scope.bindings_of(name).len() != 1 || scope.globals.contains_key(name) {
        return None;
    }
    // Callee ranges, with the range of their call.
    let mut calls = rustc_hash::FxHashMap::default();
    walk_exprs(later, &mut |e| {
        if let Expr::Call(c) = e
            && let Expr::Name(n) = &*c.func
        {
            calls.insert(n.range, c.range);
        }
    });
    let mut found = Vec::new();
    let only_calls = model.references().all(|(_, r)| {
        if r.name != name || r.is_implicit() {
            return true;
        }
        if r.scope == loop_scope {
            return match calls.get(&r.range) {
                Some(call) => {
                    found.push(*call);
                    true
                }
                None => false,
            };
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
    });
    only_calls.then_some(found)
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
    let mut invoked = FxHashSet::default();
    let mut consumed = FxHashSet::default();
    let mut lambdas = Vec::new();
    let mut nested = Vec::new();
    for e in &exprs {
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
            run_during_call(model, e, &mut invoked, &mut consumed);
        });
    }
    consumed_in_place(&[], &exprs, &mut consumed);
    consumed_with(&nested, &mut consumed);
    let mut out = deferred(
        model,
        &lambdas,
        &nested,
        &called,
        &invoked,
        &consumed,
        &FxHashSet::default(),
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

    /// Review round 26 (Codex): a builtin runs only the arguments it
    /// calls or consumes; the others are values it returns or keeps
    /// (Jython: `[2, 2, 2]` for `max(lambda: t, lambda: t)`, the initial
    /// value of `reduce`, a lambda in the iterable of `sorted` or `map`,
    /// `[[4], [4], [4]]` for `min` of two generators, `[[2], [2], [2]]`
    /// for `dict(a=...)`).
    #[test]
    fn builtins_run_only_the_arguments_they_call_or_consume() {
        for (body, names) in [
            ("out.append(max(lambda: t, lambda: t))", &["t", "t"][..]),
            ("out.append(min(lambda: t, key=len))", &["t"][..]),
            (
                "out.append(reduce(lambda x, y: x, [], lambda: t))",
                &["t"][..],
            ),
            ("out.append(sorted([lambda: t]))", &["t"][..]),
            ("out.append(map(None, [lambda: t]))", &["t"][..]),
            ("out.append(filter(None, [lambda: t]))", &["t"][..]),
            (
                "out.append(min((v * t for v in [1]), (v * t for v in [2])))",
                &["t", "t"][..],
            ),
            ("out.append(dict(a=(v * t for v in [1])))", &["t"][..]),
            ("out.append(sum([1], (v * t for v in [1])))", &["t"][..]),
            ("out.append(enumerate(v * t for v in [1]))", &["t"][..]),
        ] {
            let code = format!("out = []\nfor t in range(3):\n    {body}\n");
            let d = library("m", &code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), names, "{code}");
        }
        for body in [
            "out.append(sorted([3, 1], lambda x, y: cmp(x * t, y * t)))",
            "out.append(sorted([3, 1], None, lambda x: x * t))",
            "out.append(sorted([3, 1], cmp=lambda x, y: cmp(x * t, y)))",
            "out.append(max([1, 2], key=lambda x: x * t))",
            "out.append(map(lambda x: x * t, [1]))",
            "out.append(filter(lambda x: x == t, [1]))",
            "out.append(reduce(lambda x, y: x + y * t, [1], 0))",
            "out.append(min(v * t for v in [1]))",
            "out.append(sum(v * t for v in [1]))",
            "out.append(map(str, (v * t for v in [1])))",
            "out.append(zip([1], (v * t for v in [1])))",
            "out.append(dict((v, t) for v in [1]))",
            "out.append(', '.join(str(v * t) for v in [1]))",
        ] {
            let code = format!("out = []\nfor t in range(3):\n    {body}\n");
            let d = library("m", &code).lint(&["SCR120"]);
            assert!(d.is_empty(), "{code}: {:?}", flagged(&d));
        }
    }

    /// Review round 26 (Opus, N2): a generator expression kept in a loop
    /// reads the loop's variable when it is consumed, after the loop
    /// (Jython: `[[2], [2], [2]]`), unless it is consumed where it is
    /// written: by a builtin, a `for` statement or a comprehension, also
    /// through the iterable of a consumed generator expression. A generator
    /// function whose every call is consumed so reads each iteration's value
    /// (Jython: `3` for the sum).
    #[test]
    fn generators_kept_or_consumed_in_the_loop() {
        for (code, names) in [
            (
                "out = []\nfor i in range(3):\n    out.append(v * i for v in [1])\n",
                &["i"][..],
            ),
            (
                "def f(y, r):\n    keep = {}\n    for i in r:\n        keep[i] = (v * i for v in y)\n    return keep\n",
                &["i"][..],
            ),
            (
                "gs = [(v * k for v in [1]) for k in range(3)]\n",
                &["k"][..],
            ),
            (
                "out = []\nfor i in range(3):\n    def gen():\n        yield i\n    out.append(list(gen()))\n    out.append(gen())\n",
                &["i"][..],
            ),
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert_eq!(flagged(&d), names, "{code}");
        }
        for code in [
            "total = 0\nfor i in range(3):\n    def gen():\n        yield i\n    total += sum(gen())\n",
            "total = 0\nfor i in range(3):\n    def gen():\n        yield i\n    total += sum(gen())\n    print list(gen())\n",
            "for i in range(3):\n    total = sum(x for x in (v * i for v in [1]))\n",
            "for i in range(3):\n    for z in (v * i for v in [1]):\n        print z\n",
            "for i in range(3):\n    print [z for z in (v * i for v in [1])]\n",
            "for i in range(3):\n    print {z: 1 for z in (v * i for v in [1])}\n",
            // The first iterable is evaluated in the iteration.
            "out = []\nfor i in range(3):\n    out.append(v for v in [i])\n",
        ] {
            let d = library("m", code).lint(&["SCR120"]);
            assert!(d.is_empty(), "{code}: {:?}", flagged(&d));
        }
    }

    #[test]
    fn default_arguments_capture() {
        let code = "def f(paths):\n    for p in paths:\n        system.util.invokeLater(lambda p=p: g(p))\n";
        assert!(library("m", code).lint(&["SCR120"]).is_empty());
    }
}
