//! Errors Jython's compiler raises for code its parser accepts.
//!
//! Every check here was verified against IA's Jython 2.7 with full
//! `compile()` (see `oracle/jython_compile_oracle.py` and the
//! `compile_oracle` test): messages are Jython's, and nothing is reported
//! that Jython compiles. Some checks mirror quirks: a nested function's
//! references to builtins count as free variables for `import *` and
//! `exec`, and `def f(a, *a)` compiles. (`continue` in a `finally` clause
//! is rejected by Jython's parser, see `iglint_python`.)

use iglint_python::Anchor;
use iglint_python::ast::*;
use iglint_text::{Ranged, TextRange};
use rustc_hash::FxHashSet;

use crate::model::{ScopeId, ScopeKind, SemanticModel};

/// One compile-time error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError {
    /// The code to underline.
    pub range: TextRange,
    /// Jython's message.
    pub message: String,
    /// The token Jython reports the error at (`None` when Jython reports no
    /// position: internal compiler failures).
    pub anchor: Option<Anchor>,
    pub kind: CompileErrorKind,
}

/// The family of a [`CompileError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompileErrorKind {
    ReturnOutsideFunction,
    YieldOutsideFunction,
    BreakOutsideLoop,
    ContinueOutsideLoop,
    DuplicateArgument,
    ReturnWithValueInGenerator,
    DefaultExceptNotLast,
    ParameterDeclaredGlobal,
    ImportStarInFunction,
    UnqualifiedExec,
    DeleteCellVariable,
    AssignToDebug,
    FutureNotFirst,
    UnknownFuture,
    /// Jython's compiler crashes on the target (`-a = 1`, `for f() in x`);
    /// the code cannot be compiled.
    InvalidTarget,
    /// A `yield` lexically inside a list comprehension, or in the first
    /// iterable of a set or dict comprehension, makes Jython's code
    /// generator emit bytecode its own verifier rejects
    /// (`java.lang.VerifyError`); the script cannot be compiled. Verified
    /// case by case against Jython 2.7 (`tests/oracle/compile.cases`,
    /// the `yield-comp-*` cases).
    YieldInComprehension,
    /// The first two lines reach [`crate::MARK_LIMIT`] characters
    /// (`java.io.IOException: Mark invalid`).
    MarkInvalid,
    /// A module, class or function body is larger than a Java method can
    /// hold.
    MethodTooLarge,
    /// A module has more constants than a Java class can hold.
    ClassTooLarge,
}

impl CompileErrorKind {
    /// Whether Jython reports this as an internal compiler failure (no
    /// source position): the code generator or the bytecode verifier
    /// crashes rather than raising a positioned `SyntaxError`.
    pub fn is_internal_crash(self) -> bool {
        matches!(
            self,
            CompileErrorKind::InvalidTarget | CompileErrorKind::YieldInComprehension
        )
    }
}

/// The compile-time errors of a module, in source order.
pub fn compile_errors(model: &SemanticModel<'_>) -> Vec<CompileError> {
    let mut c = Checker {
        model,
        out: Vec::new(),
        free: propagated_free(model),
    };
    c.future(&model.module.body);
    // The module's own statements are all compiled, even after a `raise`
    // (verified on Jython); suites inside them are not. Code after a
    // statement that does not complete is unreachable all the same.
    let mut unreachable = false;
    for stmt in &model.module.body {
        c.stmt(
            stmt,
            Ctx {
                scope: ScopeId::MODULE,
                in_loop: false,
                dead: false,
                unreachable,
                yield_crashes: false,
            },
        );
        unreachable |= !completion(stmt).falls;
    }
    for (id, scope) in model.scopes() {
        if scope.kind.is_function_like() {
            c.generator_returns(id);
            c.exec_and_star(id);
        }
    }
    c.out.sort_by_key(|e| (e.range.start(), e.range.end()));
    c.out
}

/// Jython's free variables of each scope, including those of nested scopes
/// that the scope does not bind.
fn propagated_free<'a>(model: &SemanticModel<'a>) -> Vec<FxHashSet<&'a str>> {
    let n = model.scopes.len();
    let mut free: Vec<FxHashSet<&'a str>> = vec![FxHashSet::default(); n];
    // Children have larger ids than their parents.
    for i in (0..n).rev() {
        let scope = &model.scopes[i];
        let mut set: FxHashSet<&'a str> = scope.free_names.iter().map(|(n, _)| *n).collect();
        for child in &scope.children {
            for name in &free[child.index()] {
                let bound_here = scope.kind != ScopeKind::Class && scope.is_local(name);
                if !bound_here && !scope.globals.contains_key(name) {
                    set.insert(name);
                }
            }
        }
        free[i] = set;
    }
    free
}

#[derive(Clone, Copy)]
struct Ctx {
    scope: ScopeId,
    in_loop: bool,
    /// Jython generates no code for the statement (see [`ends_suite`]):
    /// errors its code generator raises do not occur, errors of its
    /// symbol table pass still do.
    dead: bool,
    /// The statement's code is generated but cannot be reached in the
    /// code object's bytecode (see [`completion`]). The bytecode
    /// verifier crashes Jython only on reachable code, so invalid targets
    /// there compile; every other error still occurs.
    unreachable: bool,
    /// A `yield` evaluated in this position is compiled into the current
    /// function's code object by way of a list comprehension (inlined in
    /// Python 2) or as the first iterable of a set or dict comprehension
    /// (evaluated in the enclosing scope). Jython's verifier then rejects
    /// the method ([`CompileErrorKind::YieldInComprehension`]). The flag
    /// is set while walking those positions and cleared at every new code
    /// object (a `def`, `lambda` body or the inner scope of a
    /// comprehension or generator expression). Verified against Jython.
    yield_crashes: bool,
}

struct Checker<'m, 'a> {
    model: &'m SemanticModel<'a>,
    out: Vec<CompileError>,
    free: Vec<FxHashSet<&'a str>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetContext {
    Assign,
    AugAssign,
    Delete,
    For,
    Except,
    With,
    ListComp,
    Generator,
}

impl<'a> Checker<'_, 'a> {
    fn error(
        &mut self,
        kind: CompileErrorKind,
        range: TextRange,
        anchor: Option<Anchor>,
        message: impl Into<String>,
    ) {
        self.out.push(CompileError {
            range,
            message: message.into(),
            anchor,
            kind,
        });
    }

    fn is_function(&self, scope: ScopeId) -> bool {
        self.model.scope(scope).kind.is_function_like()
    }

    /// A suite: Jython generates no code after a statement that ends it,
    /// and none of the code after a statement that does not complete is
    /// reachable.
    fn body(&mut self, body: &'a [Stmt], mut ctx: Ctx) {
        for stmt in body {
            self.stmt(stmt, ctx);
            ctx.dead |= ends_suite(stmt);
            ctx.unreachable |= !completion(stmt).falls;
        }
    }

    /// An error of Jython's code generator, which does not run for dead
    /// code.
    fn codegen_error(
        &mut self,
        ctx: Ctx,
        kind: CompileErrorKind,
        range: TextRange,
        anchor: Option<Anchor>,
        message: impl Into<String>,
    ) {
        if !ctx.dead {
            self.error(kind, range, anchor, message);
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt, ctx: Ctx) {
        match stmt {
            Stmt::FunctionDef(f) => {
                self.exprs(&f.decorator_list, ctx);
                self.exprs(&f.args.defaults, ctx);
                let Some(scope) = self.model.function_scope(f) else {
                    return;
                };
                self.arguments(&f.args);
                self.global_params(scope, &f.body);
                let inner = Ctx {
                    scope,
                    in_loop: false,
                    dead: ctx.dead,
                    // A new code object.
                    unreachable: false,
                    yield_crashes: false,
                };
                self.body(&f.body, inner);
            }
            Stmt::ClassDef(c) => {
                self.exprs(&c.decorator_list, ctx);
                self.exprs(&c.bases, ctx);
                let Some(scope) = self.model.class_scope(c) else {
                    return;
                };
                let inner = Ctx {
                    scope,
                    in_loop: false,
                    dead: ctx.dead,
                    // A new code object.
                    unreachable: false,
                    yield_crashes: false,
                };
                self.body(&c.body, inner);
            }
            Stmt::Return(s) => {
                if !self.is_function(ctx.scope) {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::ReturnOutsideFunction,
                        s.range,
                        Some(s.anchor),
                        "'return' outside function",
                    );
                }
                self.opt_expr(s.value.as_deref(), ctx);
            }
            Stmt::Delete(s) => {
                for t in &s.targets {
                    self.target(t, TargetContext::Delete, ctx);
                    self.delete_cells(t, ctx);
                }
            }
            Stmt::Assign(s) => {
                for t in &s.targets {
                    self.target(t, TargetContext::Assign, ctx);
                }
                self.expr(&s.value, ctx);
            }
            Stmt::AugAssign(s) => {
                self.target(&s.target, TargetContext::AugAssign, ctx);
                self.expr(&s.value, ctx);
            }
            Stmt::Print(s) => {
                self.opt_expr(s.dest.as_deref(), ctx);
                self.exprs(&s.values, ctx);
            }
            Stmt::For(s) => {
                self.target(&s.target, TargetContext::For, ctx);
                self.expr(&s.iter, ctx);
                self.body(
                    &s.body,
                    Ctx {
                        in_loop: true,
                        ..ctx
                    },
                );
                self.body(&s.orelse, ctx);
            }
            Stmt::While(s) => {
                self.expr(&s.test, ctx);
                self.body(
                    &s.body,
                    Ctx {
                        in_loop: true,
                        ..ctx
                    },
                );
                self.body(&s.orelse, ctx);
            }
            Stmt::If(s) => {
                self.expr(&s.test, ctx);
                self.body(&s.body, ctx);
                self.body(&s.orelse, ctx);
            }
            Stmt::With(s) => {
                self.expr(&s.context_expr, ctx);
                if let Some(v) = &s.optional_vars {
                    self.target(v, TargetContext::With, ctx);
                }
                self.body(&s.body, ctx);
            }
            Stmt::Raise(s) => {
                self.opt_expr(s.type_.as_deref(), ctx);
                self.opt_expr(s.inst.as_deref(), ctx);
                self.opt_expr(s.tback.as_deref(), ctx);
            }
            Stmt::TryExcept(s) => {
                self.body(&s.body, ctx);
                let last = s.handlers.len().saturating_sub(1);
                for (i, h) in s.handlers.iter().enumerate() {
                    if h.type_.is_none() && i != last {
                        self.codegen_error(
                            ctx,
                            CompileErrorKind::DefaultExceptNotLast,
                            h.range,
                            Some(h.anchor),
                            "default 'except:' must be last",
                        );
                    }
                    self.opt_expr(h.type_.as_deref(), ctx);
                    if let Some(n) = &h.name {
                        self.target(n, TargetContext::Except, ctx);
                    }
                    self.body(&h.body, ctx);
                }
                self.body(&s.orelse, ctx);
            }
            Stmt::TryFinally(s) => {
                self.body(&s.body, ctx);
                self.body(&s.finalbody, ctx);
            }
            Stmt::Assert(s) => {
                self.expr(&s.test, ctx);
                self.opt_expr(s.msg.as_deref(), ctx);
            }
            Stmt::Exec(s) => {
                self.expr(&s.body, ctx);
                self.opt_expr(s.globals.as_deref(), ctx);
                self.opt_expr(s.locals.as_deref(), ctx);
            }
            Stmt::Expr(s) => self.expr(&s.value, ctx),
            Stmt::Break(s) => {
                if !ctx.in_loop {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::BreakOutsideLoop,
                        s.range,
                        Some(s.anchor),
                        "'break' outside loop",
                    );
                }
            }
            Stmt::Continue(s) => {
                if !ctx.in_loop {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::ContinueOutsideLoop,
                        s.range,
                        Some(s.anchor),
                        "'continue' not properly in loop",
                    );
                }
            }
            Stmt::Import(_) | Stmt::ImportFrom(_) | Stmt::Global(_) | Stmt::Pass(_) => {}
        }
    }

    fn exprs(&mut self, exprs: &'a [Expr], ctx: Ctx) {
        for e in exprs {
            self.expr(e, ctx);
        }
    }

    fn opt_expr(&mut self, e: Option<&'a Expr>, ctx: Ctx) {
        if let Some(e) = e {
            self.expr(e, ctx);
        }
    }

    fn expr(&mut self, expr: &'a Expr, ctx: Ctx) {
        match expr {
            Expr::Yield(y) => {
                if !self.is_function(ctx.scope) {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::YieldOutsideFunction,
                        y.range,
                        Some(y.anchor),
                        "'yield' outside function",
                    );
                } else if ctx.yield_crashes && !ctx.unreachable {
                    // Compiled into a method whose bytecode Jython's own
                    // verifier rejects (`java.lang.VerifyError`): a `yield`
                    // inside a list comprehension or in the first iterable
                    // of a set or dict comprehension.
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::YieldInComprehension,
                        y.range,
                        Some(y.anchor),
                        "a 'yield' inside a list comprehension, or in the first iterable of a \
                         set or dict comprehension, crashes Jython's compiler (internal error); \
                         the script cannot be compiled",
                    );
                }
                self.opt_expr(y.value.as_deref(), ctx);
            }
            Expr::Lambda(l) => {
                self.exprs(&l.args.defaults, ctx);
                self.arguments(&l.args);
                if let Some(scope) = self.model.lambda_scope(l) {
                    self.expr(
                        &l.body,
                        Ctx {
                            scope,
                            in_loop: false,
                            dead: ctx.dead,
                            unreachable: false,
                            yield_crashes: false,
                        },
                    );
                }
            }
            Expr::ListComp(e) => {
                // A list comprehension has no scope of its own in Python 2:
                // every part is compiled into the current function, so a
                // `yield` anywhere in it crashes the verifier.
                let lc = Ctx {
                    yield_crashes: true,
                    ..ctx
                };
                for g in &e.generators {
                    self.target(&g.target, TargetContext::ListComp, lc);
                    self.expr(&g.iter, lc);
                    self.exprs(&g.ifs, lc);
                }
                self.expr(&e.elt, lc);
            }
            Expr::GeneratorExp(_) | Expr::SetComp(_) | Expr::DictComp(_) => {
                let (generators, elts): (&'a [Comprehension], Vec<&'a Expr>) = match expr {
                    Expr::GeneratorExp(g) => (&g.generators, vec![&g.elt]),
                    Expr::SetComp(g) => (&g.generators, vec![&g.elt]),
                    Expr::DictComp(g) => (&g.generators, vec![&g.key, &g.value]),
                    _ => unreachable!(),
                };
                let Some(scope) = self.model.generator_scope(expr) else {
                    return;
                };
                let inner = Ctx {
                    scope,
                    in_loop: false,
                    dead: ctx.dead,
                    // A new code object.
                    unreachable: false,
                    yield_crashes: false,
                };
                // The first iterable is evaluated in the enclosing scope.
                // For a set or dict comprehension a `yield` there crashes
                // the verifier; for a generator expression it does not, so
                // it only inherits whatever the enclosing position already
                // carries. Every other part is a new code object.
                let first_iter = if matches!(expr, Expr::SetComp(_) | Expr::DictComp(_)) {
                    Ctx {
                        yield_crashes: true,
                        ..ctx
                    }
                } else {
                    ctx
                };
                for (i, g) in generators.iter().enumerate() {
                    self.target(&g.target, TargetContext::Generator, inner);
                    self.expr(&g.iter, if i == 0 { first_iter } else { inner });
                    self.exprs(&g.ifs, inner);
                }
                for e in elts {
                    self.expr(e, inner);
                }
            }
            _ => walk_children(expr, &mut |child| self.expr(child, ctx)),
        }
    }

    // ---- targets ----

    fn target(&mut self, target: &'a Expr, context: TargetContext, ctx: Ctx) {
        match target {
            Expr::Name(n) => {
                if &*n.id == "__debug__" {
                    self.error(
                        CompileErrorKind::AssignToDebug,
                        n.range,
                        Some(n.anchor),
                        "can not assign to __debug__",
                    );
                }
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.target(e, context, ctx);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.target(e, context, ctx);
                }
            }
            Expr::Attribute(a) => self.expr(&a.value, ctx),
            Expr::Subscript(_) => walk_children(target, &mut |child| self.expr(child, ctx)),
            other => {
                if crashes_compiler(other, context) && !ctx.unreachable {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::InvalidTarget,
                        other.range(),
                        None,
                        format!(
                            "Jython's compiler fails on this {} target (internal error); the script cannot be compiled",
                            target_description(other)
                        ),
                    );
                }
                self.expr(other, ctx);
            }
        }
    }

    // ---- functions ----

    fn arguments(&mut self, args: &'a Arguments) {
        let mut seen: FxHashSet<&str> = FxHashSet::default();
        let mut reported = false;
        for arg in &args.args {
            self.argument_names(arg, &mut seen, &mut reported);
        }
    }

    fn argument_names(
        &mut self,
        arg: &'a Expr,
        seen: &mut FxHashSet<&'a str>,
        reported: &mut bool,
    ) {
        match arg {
            Expr::Name(n) => {
                if !seen.insert(&n.id) && !*reported {
                    *reported = true;
                    self.error(
                        CompileErrorKind::DuplicateArgument,
                        n.range,
                        Some(n.anchor),
                        format!("duplicate argument name found: {}", n.id),
                    );
                }
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.argument_names(e, seen, reported);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.argument_names(e, seen, reported);
                }
            }
            _ => {}
        }
    }

    /// `global x` where `x` is a parameter.
    fn global_params(&mut self, scope: ScopeId, body: &'a [Stmt]) {
        let s = self.model.scope(scope);
        if s.globals.is_empty() {
            return;
        }
        let Params(params) = parameter_names(self.model, scope);
        if params.is_empty() {
            return;
        }
        let mut globals = Vec::new();
        collect_globals(body, &mut globals);
        for g in globals {
            if let Some(name) = g.names.iter().find(|n| params.contains(&&*n.id)) {
                self.error(
                    CompileErrorKind::ParameterDeclaredGlobal,
                    g.range,
                    Some(g.anchor),
                    format!("name '{}' is local and global", name.id),
                );
                return;
            }
        }
    }

    /// `del x` where `x` is read by a nested scope.
    fn delete_cells(&mut self, target: &'a Expr, ctx: Ctx) {
        match target {
            Expr::Name(n) => {
                let scope = self.model.scope(ctx.scope);
                if !scope.kind.is_function_like() || !scope.is_local(&n.id) {
                    return;
                }
                let captured = scope
                    .children
                    .iter()
                    .any(|c| self.free[c.index()].contains(&*n.id));
                if captured {
                    self.codegen_error(
                        ctx,
                        CompileErrorKind::DeleteCellVariable,
                        n.range,
                        Some(n.anchor),
                        format!(
                            "can not delete variable '{}' referenced in nested scope",
                            n.id
                        ),
                    );
                }
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.delete_cells(e, ctx);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.delete_cells(e, ctx);
                }
            }
            _ => {}
        }
    }

    /// `return value` in a generator. Jython reports whichever of the
    /// first `yield` and the first `return value` comes second.
    fn generator_returns(&mut self, scope: ScopeId) {
        let s = self.model.scope(scope);
        if !s.is_generator {
            return;
        }
        let crate::model::ScopeNode::Function(f) = s.node else {
            return;
        };
        let mut events = Vec::new();
        collect_yields_and_returns(&f.body, &mut events);
        let mut seen_yield = false;
        let mut seen_return = false;
        for (is_yield, range, anchor) in events {
            if (is_yield && seen_return) || (!is_yield && seen_yield) {
                self.error(
                    CompileErrorKind::ReturnWithValueInGenerator,
                    range,
                    Some(anchor),
                    "'return' with argument inside generator",
                );
                return;
            }
            seen_yield |= is_yield;
            seen_return |= !is_yield;
        }
    }

    /// Unqualified `exec` and `import *` in functions with free variables.
    fn exec_and_star(&mut self, scope: ScopeId) {
        let s = self.model.scope(scope);
        let crate::model::ScopeNode::Function(f) = s.node else {
            return;
        };
        let child_free = s.children.iter().any(|c| !self.free[c.index()].is_empty());
        let nested = {
            let mut at = s.parent;
            let mut nested = false;
            while let Some(p) = at {
                if self.model.scope(p).kind.is_function_like() {
                    nested = true;
                    break;
                }
                at = self.model.scope(p).parent;
            }
            nested
        };
        if s.unqualified_exec.is_some() {
            if child_free {
                self.error(
                    CompileErrorKind::UnqualifiedExec,
                    f.name.range,
                    Some(f.anchor),
                    format!(
                        "unqualified exec is not allowed in function '{}' because it contains a function with free variables",
                        f.name.id
                    ),
                );
                return;
            }
            if nested && !s.free_names.is_empty() {
                self.error(
                    CompileErrorKind::UnqualifiedExec,
                    f.name.range,
                    Some(f.anchor),
                    format!(
                        "unqualified exec is not allowed in function '{}' because it contains free variables",
                        f.name.id
                    ),
                );
                return;
            }
        }
        if s.star_import.is_some() && (child_free || nested) {
            let Some(stmt) = first_star_import(&f.body) else {
                return;
            };
            let reason = if child_free {
                "contains a nested function with free variables"
            } else {
                "is a nested function"
            };
            let Stmt::ImportFrom(i) = stmt else { return };
            self.error(
                CompileErrorKind::ImportStarInFunction,
                i.range,
                Some(i.anchor),
                format!(
                    "import * is not allowed in function '{}' because it {reason}",
                    f.name.id
                ),
            );
        }
    }

    // ---- __future__ ----

    fn future(&mut self, body: &'a [Stmt]) {
        // Allowed at the top of the module: a docstring, then future imports.
        let mut allowed = true;
        for (i, stmt) in body.iter().enumerate() {
            match stmt {
                Stmt::ImportFrom(s) if is_future(s) => {
                    if allowed {
                        self.future_names(s);
                    } else {
                        self.future_misplaced(s);
                    }
                }
                Stmt::Expr(e) if i == 0 && matches!(&*e.value, Expr::Str(_)) => {}
                _ => allowed = false,
            }
        }
        // Future imports anywhere else.
        let mut nested = Vec::new();
        for stmt in body {
            collect_nested_futures(stmt, &mut nested);
        }
        for s in nested {
            self.future_misplaced(s);
        }
    }

    fn future_misplaced(&mut self, s: &StmtImportFrom) {
        self.error(
            CompileErrorKind::FutureNotFirst,
            s.range,
            Some(s.anchor),
            "from __future__ imports must occur at the beginning of the file",
        );
    }

    fn future_names(&mut self, s: &StmtImportFrom) {
        for alias in &s.names {
            let name = &*alias.name.id;
            if FUTURE_FEATURES.contains(&name) {
                continue;
            }
            let message = if name == "braces" {
                "not a chance".to_owned()
            } else {
                format!("future feature {name} is not defined")
            };
            self.error(
                CompileErrorKind::UnknownFuture,
                s.range,
                Some(s.anchor),
                message,
            );
            return;
        }
    }
}

/// Features Jython 2.7's `__future__` knows.
const FUTURE_FEATURES: &[&str] = &[
    "nested_scopes",
    "generators",
    "division",
    "absolute_import",
    "with_statement",
    "print_function",
    "unicode_literals",
];

fn is_future(s: &StmtImportFrom) -> bool {
    s.level == 0 && s.module.as_ref().is_some_and(|m| &*m.id == "__future__")
}

/// Future imports that are not top-level statements of the module.
fn collect_nested_futures<'a>(stmt: &'a Stmt, out: &mut Vec<&'a StmtImportFrom>) {
    for_each_child_body(stmt, &mut |body| {
        for s in body {
            if let Stmt::ImportFrom(i) = s
                && is_future(i)
            {
                out.push(i);
            }
            collect_nested_futures(s, out);
        }
    });
}

/// Calls `f` with every statement list directly inside `stmt` (including
/// function and class bodies).
pub(crate) fn for_each_child_body<'a>(stmt: &'a Stmt, f: &mut dyn FnMut(&'a [Stmt])) {
    match stmt {
        Stmt::FunctionDef(s) => f(&s.body),
        Stmt::ClassDef(s) => f(&s.body),
        Stmt::For(s) => {
            f(&s.body);
            f(&s.orelse);
        }
        Stmt::While(s) => {
            f(&s.body);
            f(&s.orelse);
        }
        Stmt::If(s) => {
            f(&s.body);
            f(&s.orelse);
        }
        Stmt::With(s) => f(&s.body),
        Stmt::TryExcept(s) => {
            f(&s.body);
            for h in &s.handlers {
                f(&h.body);
            }
            f(&s.orelse);
        }
        Stmt::TryFinally(s) => {
            f(&s.body);
            f(&s.finalbody);
        }
        _ => {}
    }
}

/// `global` statements of a function body (not of nested functions or
/// classes).
fn collect_globals<'a>(body: &'a [Stmt], out: &mut Vec<&'a StmtGlobal>) {
    for stmt in body {
        match stmt {
            Stmt::Global(g) => out.push(g),
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            other => for_each_child_body(other, &mut |b| collect_globals(b, out)),
        }
    }
}

/// Whether Jython stops generating code for the rest of a suite after
/// `stmt` (its `CodeCompiler.suite` returns at the first statement whose
/// code "exits"): `return`, `raise`, `continue`, and an `if` with an
/// `else` (or `elif` chain ending in one) whose every branch ends that
/// way. Not `break`, loops, `try`, `with` or `assert False`, and not the
/// module's own statement list; function and class bodies and the suites
/// of compound statements are cut. Verified with
/// `oracle/run-oracle.sh --compile` (see `tests/limits_oracle.rs`).
pub(crate) fn ends_suite(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(_) | Stmt::Raise(_) | Stmt::Continue(_) => true,
        Stmt::If(s) => {
            !s.orelse.is_empty() && s.body.iter().any(ends_suite) && s.orelse.iter().any(ends_suite)
        }
        _ => false,
    }
}

/// How control leaves a statement, in the bytecode Jython generates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Completion {
    /// Execution may continue with the next statement.
    pub falls: bool,
    /// A `break` may leave the innermost enclosing loop.
    pub breaks: bool,
}

/// How control may leave `stmt`, with no constant folding (`while 1:`
/// may end): `return`, `raise`, `continue` and `break` do not complete;
/// `if` completes when a branch (or the missing `else`) does; a loop when
/// it has no `else`, its `else` completes or its body breaks; `with`
/// always (`__exit__` may swallow the exception); `try`/`except` when the
/// body and `else` or a handler completes (an exception no handler takes
/// propagates); `try`/`finally` when both its parts complete. Verified
/// with `oracle/run-oracle.sh --compile` (the `reach-*` cases of
/// `tests/oracle/compile.cases`).
pub(crate) fn completion(stmt: &Stmt) -> Completion {
    const FALLS: Completion = Completion {
        falls: true,
        breaks: false,
    };
    fn suite(body: &[Stmt]) -> Completion {
        let mut c = FALLS;
        for stmt in body {
            if !c.falls {
                break;
            }
            let s = completion(stmt);
            c = Completion {
                falls: s.falls,
                breaks: c.breaks || s.breaks,
            };
        }
        c
    }
    match stmt {
        Stmt::Return(_) | Stmt::Raise(_) | Stmt::Continue(_) => Completion {
            falls: false,
            breaks: false,
        },
        Stmt::Break(_) => Completion {
            falls: false,
            breaks: true,
        },
        Stmt::If(s) => {
            let (b, o) = (suite(&s.body), suite(&s.orelse));
            Completion {
                falls: b.falls || o.falls,
                breaks: b.breaks || o.breaks,
            }
        }
        Stmt::For(_) | Stmt::While(_) => {
            let (body, orelse) = match stmt {
                Stmt::For(s) => (&s.body, &s.orelse),
                Stmt::While(s) => (&s.body, &s.orelse),
                _ => unreachable!(),
            };
            let (b, o) = (suite(body), suite(orelse));
            Completion {
                falls: o.falls || b.breaks,
                breaks: o.breaks,
            }
        }
        Stmt::With(s) => Completion {
            falls: true,
            breaks: suite(&s.body).breaks,
        },
        Stmt::TryExcept(s) => {
            let body = suite(&s.body);
            let orelse = suite(&s.orelse);
            let handlers: Vec<Completion> = s.handlers.iter().map(|h| suite(&h.body)).collect();
            Completion {
                falls: (body.falls && orelse.falls) || handlers.iter().any(|h| h.falls),
                breaks: body.breaks
                    || (body.falls && orelse.breaks)
                    || handlers.iter().any(|h| h.breaks),
            }
        }
        Stmt::TryFinally(s) => {
            let (body, finally) = (suite(&s.body), suite(&s.finalbody));
            if finally.falls {
                Completion {
                    falls: body.falls,
                    breaks: body.breaks || finally.breaks,
                }
            } else {
                Completion {
                    falls: false,
                    breaks: finally.breaks,
                }
            }
        }
        _ => FALLS,
    }
}

/// The first `from x import *` of a function body that Jython generates
/// code for.
fn first_star_import(body: &[Stmt]) -> Option<&Stmt> {
    for stmt in body {
        match stmt {
            Stmt::ImportFrom(i) if i.names.iter().any(|a| &*a.name.id == "*") => return Some(stmt),
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            other => {
                let mut found = None;
                for_each_child_body(other, &mut |b| {
                    if found.is_none() {
                        found = first_star_import(b);
                    }
                });
                if found.is_some() {
                    return found;
                }
            }
        }
        if ends_suite(stmt) {
            break;
        }
    }
    None
}

/// `(is_yield, range, anchor)` of every `yield` and `return value` of a
/// function body in source order, not looking into nested scopes (list
/// comprehensions belong to the function).
fn collect_yields_and_returns(body: &[Stmt], out: &mut Vec<(bool, TextRange, Anchor)>) {
    fn expr(e: &Expr, out: &mut Vec<(bool, TextRange, Anchor)>) {
        match e {
            Expr::Yield(y) => {
                if let Some(v) = &y.value {
                    expr(v, out);
                }
                out.push((true, y.range, y.anchor));
            }
            Expr::Lambda(l) => {
                for d in &l.args.defaults {
                    expr(d, out);
                }
            }
            Expr::GeneratorExp(g) => {
                if let Some(first) = g.generators.first() {
                    expr(&first.iter, out);
                }
            }
            Expr::SetComp(g) => {
                if let Some(first) = g.generators.first() {
                    expr(&first.iter, out);
                }
            }
            Expr::DictComp(g) => {
                if let Some(first) = g.generators.first() {
                    expr(&first.iter, out);
                }
            }
            other => walk_children(other, &mut |c| expr(c, out)),
        }
    }
    for stmt in body {
        match stmt {
            Stmt::FunctionDef(f) => {
                for d in f.decorator_list.iter().chain(&f.args.defaults) {
                    expr(d, out);
                }
            }
            Stmt::ClassDef(c) => {
                for d in c.decorator_list.iter().chain(&c.bases) {
                    expr(d, out);
                }
            }
            Stmt::Return(r) => {
                if let Some(v) = &r.value {
                    // Jython records the `return value` event before it
                    // descends into the value, so a `yield` inside the
                    // value counts as coming after the return (verified on
                    // Jython: `return (yield 1)` and `return [(yield i)
                    // for i in x]` report at the yield).
                    out.push((false, r.range, r.anchor));
                    expr(v, out);
                }
            }
            other => {
                for_each_stmt_expr(other, &mut |e| expr(e, out));
                for_each_child_body(other, &mut |b| collect_yields_and_returns(b, out));
            }
        }
    }
}

/// Calls `f` with the expressions directly in `stmt` (not in nested
/// statements), in evaluation order.
pub(crate) fn for_each_stmt_expr<'a>(stmt: &'a Stmt, f: &mut dyn FnMut(&'a Expr)) {
    match stmt {
        Stmt::Delete(s) => s.targets.iter().for_each(&mut *f),
        Stmt::Assign(s) => {
            f(&s.value);
            s.targets.iter().for_each(f);
        }
        Stmt::AugAssign(s) => {
            f(&s.target);
            f(&s.value);
        }
        Stmt::Print(s) => {
            if let Some(d) = &s.dest {
                f(d);
            }
            s.values.iter().for_each(f);
        }
        Stmt::For(s) => {
            f(&s.iter);
            f(&s.target);
        }
        Stmt::While(s) => f(&s.test),
        Stmt::If(s) => f(&s.test),
        Stmt::With(s) => {
            f(&s.context_expr);
            if let Some(v) = &s.optional_vars {
                f(v);
            }
        }
        Stmt::Raise(s) => {
            for e in [&s.type_, &s.inst, &s.tback].into_iter().flatten() {
                f(e);
            }
        }
        Stmt::TryExcept(s) => {
            for h in &s.handlers {
                for e in [&h.type_, &h.name].into_iter().flatten() {
                    f(e);
                }
            }
        }
        Stmt::Assert(s) => {
            f(&s.test);
            if let Some(m) = &s.msg {
                f(m);
            }
        }
        Stmt::Exec(s) => {
            f(&s.body);
            for e in [&s.globals, &s.locals].into_iter().flatten() {
                f(e);
            }
        }
        Stmt::Expr(s) => f(&s.value),
        Stmt::Return(s) => {
            if let Some(v) = &s.value {
                f(v);
            }
        }
        _ => {}
    }
}

/// Calls `f` with the direct child expressions of `expr`.
pub(crate) fn walk_children<'a>(expr: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    fn slice<'a>(s: &'a Slice, f: &mut dyn FnMut(&'a Expr)) {
        match s {
            Slice::Ellipsis(_) => {}
            Slice::Slice(s) => {
                for e in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    f(e);
                }
            }
            Slice::ExtSlice(s) => {
                for d in &s.dims {
                    slice(d, f);
                }
            }
            Slice::Index(s) => f(&s.value),
        }
    }
    match expr {
        Expr::BoolOp(e) => e.values.iter().for_each(f),
        Expr::BinOp(e) => {
            f(&e.left);
            f(&e.right);
        }
        Expr::UnaryOp(e) => f(&e.operand),
        Expr::Lambda(e) => {
            e.args.defaults.iter().for_each(&mut *f);
            f(&e.body);
        }
        Expr::IfExp(e) => {
            f(&e.test);
            f(&e.body);
            f(&e.orelse);
        }
        Expr::Dict(e) => {
            for (k, v) in e.keys.iter().zip(&e.values) {
                f(k);
                f(v);
            }
        }
        Expr::Set(e) => e.elts.iter().for_each(f),
        Expr::ListComp(e) => {
            f(&e.elt);
            for g in &e.generators {
                f(&g.target);
                f(&g.iter);
                g.ifs.iter().for_each(&mut *f);
            }
        }
        Expr::SetComp(e) => {
            f(&e.elt);
            for g in &e.generators {
                f(&g.target);
                f(&g.iter);
                g.ifs.iter().for_each(&mut *f);
            }
        }
        Expr::DictComp(e) => {
            f(&e.key);
            f(&e.value);
            for g in &e.generators {
                f(&g.target);
                f(&g.iter);
                g.ifs.iter().for_each(&mut *f);
            }
        }
        Expr::GeneratorExp(e) => {
            f(&e.elt);
            for g in &e.generators {
                f(&g.target);
                f(&g.iter);
                g.ifs.iter().for_each(&mut *f);
            }
        }
        Expr::Yield(e) => {
            if let Some(v) = &e.value {
                f(v);
            }
        }
        Expr::Compare(e) => {
            f(&e.left);
            e.comparators.iter().for_each(f);
        }
        Expr::Call(e) => {
            f(&e.func);
            e.args.iter().for_each(&mut *f);
            for k in &e.keywords {
                f(&k.value);
            }
            for a in [&e.starargs, &e.kwargs].into_iter().flatten() {
                f(a);
            }
        }
        Expr::Repr(e) => f(&e.value),
        Expr::Attribute(e) => f(&e.value),
        Expr::Subscript(e) => {
            f(&e.value);
            slice(&e.slice, f);
        }
        Expr::List(e) => e.elts.iter().for_each(f),
        Expr::Tuple(e) => e.elts.iter().for_each(f),
        Expr::Num(_) | Expr::Str(_) | Expr::Name(_) => {}
    }
}

struct Params<'a>(Vec<&'a str>);

/// Parameter names of a function scope (including tuple parameters,
/// `*args` and `**kwargs`).
fn parameter_names<'a>(model: &SemanticModel<'a>, scope: ScopeId) -> Params<'a> {
    let mut names = Vec::new();
    let args = match model.scope(scope).node {
        crate::model::ScopeNode::Function(f) => &f.args,
        crate::model::ScopeNode::Lambda(l) => &l.args,
        _ => return Params(names),
    };
    fn walk<'a>(e: &'a Expr, out: &mut Vec<&'a str>) {
        match e {
            Expr::Name(n) => out.push(&n.id),
            Expr::Tuple(t) => t.elts.iter().for_each(|e| walk(e, out)),
            Expr::List(t) => t.elts.iter().for_each(|e| walk(e, out)),
            _ => {}
        }
    }
    for a in &args.args {
        walk(a, &mut names);
    }
    for n in [&args.vararg, &args.kwarg].into_iter().flatten() {
        names.push(&n.id);
    }
    Params(names)
}

/// Whether Jython's compiler fails on `target` (a node its parser accepts
/// in a target position) in `context`. Verified case by case against
/// Jython; combinations not listed compile.
fn crashes_compiler(target: &Expr, context: TargetContext) -> bool {
    use TargetContext as T;
    let kind = match target {
        Expr::Call(_) => "call",
        Expr::BinOp(_) => "binop",
        Expr::UnaryOp(_) => "unary",
        Expr::Compare(_) => "compare",
        Expr::Num(_) => "num",
        Expr::Str(_) => "str",
        Expr::BoolOp(_) => "boolop",
        Expr::IfExp(_) => "ifexp",
        Expr::Lambda(_) => "lambda",
        Expr::Dict(_) => "dict",
        Expr::Set(_) => "set",
        Expr::Repr(_) => "repr",
        Expr::ListComp(_) => "listcomp",
        Expr::GeneratorExp(_) => "genexp",
        _ => return false,
    };
    let crashing: &[&str] = match context {
        T::Assign | T::AugAssign => &["unary", "compare"],
        T::Delete => &["binop", "unary", "boolop", "ifexp", "listcomp"],
        T::For => &[
            "call", "binop", "unary", "num", "str", "boolop", "ifexp", "lambda", "dict", "set",
            "repr", "listcomp", "genexp",
        ],
        T::Except => &[
            "call", "binop", "unary", "compare", "num", "str", "boolop", "ifexp", "lambda", "dict",
            "set", "repr", "listcomp", "genexp",
        ],
        T::With => &["unary", "compare", "dict", "set"],
        T::ListComp => &[
            "call", "binop", "num", "str", "boolop", "ifexp", "lambda", "dict", "set", "repr",
            "listcomp", "genexp",
        ],
        T::Generator => &["binop", "unary", "boolop", "ifexp", "listcomp"],
    };
    crashing.contains(&kind)
}

fn target_description(target: &Expr) -> &'static str {
    match target {
        Expr::Call(_) => "function call",
        Expr::BinOp(_) => "operator",
        Expr::UnaryOp(_) => "unary operator",
        Expr::Compare(_) => "comparison",
        Expr::Num(_) => "number",
        Expr::Str(_) => "string",
        Expr::BoolOp(_) => "boolean operator",
        Expr::IfExp(_) => "conditional expression",
        Expr::Lambda(_) => "lambda",
        Expr::Dict(_) => "dict",
        Expr::Set(_) => "set",
        Expr::Repr(_) => "repr",
        Expr::ListComp(_) => "list comprehension",
        Expr::GeneratorExp(_) => "generator expression",
        _ => "expression",
    }
}
