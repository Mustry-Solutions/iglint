//! Which code Jython generates, and which of it the bytecode can reach.
//!
//! Jython's `CodeCompiler.suite` stops generating code for the rest of a
//! suite after a statement that exits it ([`ends_suite`]): that code is
//! *dead*, it has no bytecode and the functions and classes defined in it
//! have no code object. Code after a statement that does not complete
//! ([`completion`]) is generated but *unreachable*: the class writer
//! replaces it with `nop`/`athrow`, so none of its name accesses happen,
//! while functions defined in it still get their code objects, and the
//! bytecode verifier does not look at it.
//!
//! Two things make generated code reachable that the statement order does
//! not show, both measured with `oracle/scope/` (accesses that survive in
//! the bytecode) and `oracle/run-oracle.sh --compile` (verifier errors):
//!
//! * a generator function's code object starts with a jump to every
//!   `yield` (where `next()` resumes), so the code after a `yield` is
//!   reachable, and from there whatever follows it: the rest of its
//!   statement, the rest of the suite, the loop it is in (again from the
//!   top), and the handlers of the `try` statements around it;
//! * an `except` clause is reachable only when its `try` body generates an
//!   instruction: a body of `global` statements alone covers no code, so no
//!   exception can reach the handlers.

use iglint_python::ast::*;
use iglint_text::{Ranged, TextRange, TextSize};
use rustc_hash::FxHashSet;

use crate::compile::{completion, ends_suite, walk_children};
use crate::model::{ScopeId, ScopeNode, SemanticModel};

/// The parts of each code object that Jython does not generate or that its
/// bytecode cannot reach.
pub(crate) struct Emission {
    /// `(scope, range, dead)`: code of `scope` in `range` is not generated
    /// (`dead`) or not reachable.
    pub hidden: Vec<(ScopeId, TextRange, bool)>,
    /// Scopes that have no code object (defined in dead code).
    pub dead_scopes: FxHashSet<ScopeId>,
}

impl Emission {
    pub(crate) fn of(model: &SemanticModel<'_>) -> Emission {
        let mut e = Emission {
            hidden: Vec::new(),
            dead_scopes: FxHashSet::default(),
        };
        let mut walker = Walker {
            model,
            out: &mut e.hidden,
        };
        // The module's own statement list is never cut (verified on
        // Jython), but code after a statement that does not complete is
        // still unreachable.
        walker.suite(&model.module.body, ScopeId::MODULE, false, false, false);
        for (id, scope) in model.scopes() {
            let Some(parent) = scope.parent else { continue };
            let range = match scope.node {
                ScopeNode::Module(_) => continue,
                ScopeNode::Class(c) => c.range,
                ScopeNode::Function(f) => f.range,
                ScopeNode::Lambda(l) => l.range,
                ScopeNode::Generator(g) => g.range(),
            };
            let dead = e.dead_scopes.contains(&parent)
                || e.hidden
                    .iter()
                    .any(|(s, r, dead)| *dead && *s == parent && r.contains_range(range));
            if dead {
                e.dead_scopes.insert(id);
            }
        }
        e
    }

    /// Whether Jython generates code for `range` of `scope`'s code (it may
    /// still be unreachable).
    pub(crate) fn generated(&self, scope: ScopeId, range: TextRange) -> bool {
        !self.dead_scopes.contains(&scope)
            && !self
                .hidden
                .iter()
                .any(|(s, r, dead)| *dead && *s == scope && r.contains_range(range))
    }

    /// Whether the code of `scope` at `range` runs at all: it is generated
    /// and reachable in the bytecode.
    pub(crate) fn runs(&self, scope: ScopeId, range: TextRange) -> bool {
        !self.dead_scopes.contains(&scope)
            && !self
                .hidden
                .iter()
                .any(|(s, r, _)| *s == scope && r.contains_range(range))
    }
}

/// Whether evaluating `expr` in its code object may `yield` (a `yield` in
/// it that is not inside a nested code object: lambda bodies and the inner
/// parts of generator expressions and set and dict comprehensions).
fn expr_yields(expr: &Expr) -> bool {
    match expr {
        Expr::Yield(_) => true,
        Expr::Lambda(l) => l.args.defaults.iter().any(expr_yields),
        Expr::GeneratorExp(g) => g.generators.first().is_some_and(|c| expr_yields(&c.iter)),
        Expr::SetComp(g) => g.generators.first().is_some_and(|c| expr_yields(&c.iter)),
        Expr::DictComp(g) => g.generators.first().is_some_and(|c| expr_yields(&c.iter)),
        other => {
            let mut found = false;
            walk_children(other, &mut |c| found |= expr_yields(c));
            found
        }
    }
}

/// The end of the first `yield` (in source order) that evaluating `expr`
/// runs in its code object.
fn first_yield_end(expr: &Expr) -> Option<TextSize> {
    if !expr_yields(expr) {
        return None;
    }
    if let Expr::Yield(y) = expr {
        // Its value is evaluated before it.
        return Some(
            y.value
                .as_deref()
                .and_then(first_yield_end)
                .unwrap_or(y.range.end())
                .max(y.range.end()),
        );
    }
    let mut first: Option<TextSize> = None;
    walk_children(expr, &mut |c| {
        if first.is_none() {
            first = first_yield_end(c);
        }
    });
    first
}

/// The expressions of a simple statement, in evaluation order.
fn simple_exprs<'a>(stmt: &'a Stmt, f: &mut dyn FnMut(&'a Expr)) {
    let opt = |e: Option<&'a Expr>, f: &mut dyn FnMut(&'a Expr)| {
        if let Some(e) = e {
            f(e);
        }
    };
    match stmt {
        Stmt::Return(s) => opt(s.value.as_deref(), f),
        Stmt::Delete(s) => s.targets.iter().for_each(&mut *f),
        Stmt::Assign(s) => {
            f(&s.value);
            s.targets.iter().for_each(&mut *f);
        }
        Stmt::AugAssign(s) => {
            f(&s.target);
            f(&s.value);
        }
        Stmt::Print(s) => {
            opt(s.dest.as_deref(), f);
            s.values.iter().for_each(&mut *f);
        }
        Stmt::Raise(s) => {
            opt(s.type_.as_deref(), f);
            opt(s.inst.as_deref(), f);
            opt(s.tback.as_deref(), f);
        }
        Stmt::Assert(s) => {
            f(&s.test);
            opt(s.msg.as_deref(), f);
        }
        Stmt::Exec(s) => {
            f(&s.body);
            opt(s.globals.as_deref(), f);
            opt(s.locals.as_deref(), f);
        }
        Stmt::Expr(s) => f(&s.value),
        _ => {}
    }
}

fn opt_yields(expr: Option<&Expr>) -> bool {
    expr.is_some_and(expr_yields)
}

fn suite_yields(body: &[Stmt]) -> bool {
    body.iter().any(stmt_yields)
}

/// Whether running `stmt` may `yield` in its own code object.
fn stmt_yields(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::FunctionDef(f) => f
            .decorator_list
            .iter()
            .chain(&f.args.defaults)
            .any(expr_yields),
        Stmt::ClassDef(c) => c.decorator_list.iter().chain(&c.bases).any(expr_yields),
        Stmt::Return(s) => opt_yields(s.value.as_deref()),
        Stmt::Delete(s) => s.targets.iter().any(expr_yields),
        Stmt::Assign(s) => expr_yields(&s.value) || s.targets.iter().any(expr_yields),
        Stmt::AugAssign(s) => expr_yields(&s.target) || expr_yields(&s.value),
        Stmt::Print(s) => opt_yields(s.dest.as_deref()) || s.values.iter().any(expr_yields),
        Stmt::For(s) => {
            expr_yields(&s.target)
                || expr_yields(&s.iter)
                || suite_yields(&s.body)
                || suite_yields(&s.orelse)
        }
        Stmt::While(s) => expr_yields(&s.test) || suite_yields(&s.body) || suite_yields(&s.orelse),
        Stmt::If(s) => expr_yields(&s.test) || suite_yields(&s.body) || suite_yields(&s.orelse),
        Stmt::With(s) => {
            expr_yields(&s.context_expr)
                || opt_yields(s.optional_vars.as_deref())
                || suite_yields(&s.body)
        }
        Stmt::Raise(s) => {
            opt_yields(s.type_.as_deref())
                || opt_yields(s.inst.as_deref())
                || opt_yields(s.tback.as_deref())
        }
        Stmt::TryExcept(s) => {
            suite_yields(&s.body)
                || suite_yields(&s.orelse)
                || s.handlers.iter().any(|h| {
                    opt_yields(h.type_.as_deref())
                        || opt_yields(h.name.as_deref())
                        || suite_yields(&h.body)
                })
        }
        Stmt::TryFinally(s) => suite_yields(&s.body) || suite_yields(&s.finalbody),
        Stmt::Assert(s) => expr_yields(&s.test) || opt_yields(s.msg.as_deref()),
        Stmt::Exec(s) => {
            expr_yields(&s.body)
                || opt_yields(s.globals.as_deref())
                || opt_yields(s.locals.as_deref())
        }
        Stmt::Expr(s) => expr_yields(&s.value),
        Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Global(_)
        | Stmt::Pass(_)
        | Stmt::Break(_)
        | Stmt::Continue(_) => false,
    }
}

struct Walker<'m, 'a, 'o> {
    model: &'m SemanticModel<'a>,
    out: &'o mut Vec<(ScopeId, TextRange, bool)>,
}

impl<'a> Walker<'_, 'a, '_> {
    fn suite(
        &mut self,
        body: &'a [Stmt],
        scope: ScopeId,
        cut: bool,
        dead: bool,
        unreachable: bool,
    ) {
        let generator = self.model.scope(scope).is_generator;
        let (mut dead, mut unreachable) = (dead, unreachable);
        for stmt in body {
            self.stmt(stmt, scope, dead, unreachable);
            if cut {
                dead |= ends_suite(stmt);
            }
            if generator && !dead && stmt_yields(stmt) {
                // Resumed after its `yield`, the statement goes on as usual.
                unreachable = false;
            }
            unreachable |= !completion(stmt).falls;
        }
    }

    fn hide(&mut self, scope: ScopeId, range: TextRange) {
        self.out.push((scope, range, false));
    }

    /// Hides the expressions of a statement header that do not `yield`,
    /// and in one that does, what is evaluated before it resumes.
    fn hide_exprs(&mut self, scope: ScopeId, exprs: &[Option<&Expr>]) {
        for e in exprs.iter().flatten() {
            match first_yield_end(e) {
                None => self.hide(scope, e.range()),
                Some(end) => self.hide(scope, TextRange::new(e.range().start(), end)),
            }
        }
    }

    /// In an unreachable simple statement that yields: hides what runs
    /// before the first `yield` resumes (source order, but an assignment's
    /// value comes before its targets).
    fn hide_before_yield(&mut self, scope: ScopeId, stmt: &Stmt) {
        let (start, end) = match stmt {
            Stmt::Assign(s) if expr_yields(&s.value) => {
                (s.value.range().start(), first_yield_end(&s.value))
            }
            _ => {
                let mut end = None;
                simple_exprs(stmt, &mut |e| {
                    if end.is_none() {
                        end = first_yield_end(e);
                    }
                });
                (stmt.range().start(), end)
            }
        };
        if let Some(end) = end {
            self.hide(scope, TextRange::new(start, end));
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt, scope: ScopeId, dead: bool, unreachable: bool) {
        if dead {
            self.out.push((scope, stmt.range(), true));
            return;
        }
        let generator = self.model.scope(scope).is_generator;
        let revived = unreachable && generator && stmt_yields(stmt);
        if unreachable && !revived {
            // The whole statement, nested suites included; nested code
            // objects are judged on their own.
            self.hide(scope, stmt.range());
        }
        // In an unreachable statement that yields, the parts after a
        // `yield` run (see the module documentation).
        match stmt {
            Stmt::FunctionDef(f) => {
                if let Some(inner) = self.model.function_scope(f) {
                    self.suite(&f.body, inner, true, false, false);
                }
            }
            Stmt::ClassDef(c) => {
                if let Some(inner) = self.model.class_scope(c) {
                    self.suite(&c.body, inner, true, false, false);
                }
            }
            Stmt::For(s) => {
                // Resumed in the body, the loop goes round again: only the
                // iterable (evaluated once, first) stays unreachable.
                let looping = revived && (suite_yields(&s.body) || expr_yields(&s.target));
                if revived {
                    self.hide_exprs(scope, &[Some(&s.iter)]);
                }
                let inner = unreachable && !looping;
                self.suite(&s.body, scope, true, false, inner);
                self.suite(&s.orelse, scope, true, false, inner);
            }
            Stmt::While(s) => {
                let looping = revived && (suite_yields(&s.body) || expr_yields(&s.test));
                if revived && !looping {
                    self.hide_exprs(scope, &[Some(&s.test)]);
                }
                let inner = unreachable && !looping;
                self.suite(&s.body, scope, true, false, inner);
                self.suite(&s.orelse, scope, true, false, inner);
            }
            Stmt::If(s) => {
                if revived {
                    self.hide_exprs(scope, &[Some(&s.test)]);
                }
                self.suite(&s.body, scope, true, false, unreachable);
                self.suite(&s.orelse, scope, true, false, unreachable);
            }
            Stmt::With(s) => {
                if revived {
                    self.hide_exprs(scope, &[Some(&s.context_expr), s.optional_vars.as_deref()]);
                }
                self.suite(&s.body, scope, true, false, unreachable);
            }
            Stmt::TryExcept(s) => {
                self.suite(&s.body, scope, true, false, unreachable);
                // A body without instructions (only `global` statements)
                // covers no code, so no exception reaches the handlers.
                let silent = s.body.iter().all(|b| matches!(b, Stmt::Global(_)));
                let body_runs = !unreachable || suite_yields(&s.body);
                let handlers_unreachable = silent || !body_runs;
                for h in &s.handlers {
                    if handlers_unreachable && (!unreachable || revived) {
                        let header = [h.type_.as_deref(), h.name.as_deref()];
                        if suite_yields(&h.body) {
                            self.hide_exprs(scope, &header);
                        } else {
                            self.hide(scope, h.range);
                        }
                    }
                    self.suite(&h.body, scope, true, false, handlers_unreachable);
                }
                // `else` runs when the body completes.
                let skipped = !silent && !s.body.iter().all(|b| completion(b).falls);
                self.suite(&s.orelse, scope, true, false, !body_runs || skipped);
            }
            Stmt::TryFinally(s) => {
                self.suite(&s.body, scope, true, false, unreachable);
                let body_runs = !unreachable || suite_yields(&s.body);
                self.suite(&s.finalbody, scope, true, false, !body_runs);
            }
            _ => {
                if revived {
                    self.hide_before_yield(scope, stmt);
                }
            }
        }
    }
}
