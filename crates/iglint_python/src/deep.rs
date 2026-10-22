//! Deep trees: a depth limit, depth measurement and a destructor that does
//! not recurse.
//!
//! Left-associative chains (`a + b + ...`, `a.b.c...`, `x[0][0]...`), prefix
//! operators and `elif` clauses are parsed with loops, so their length is
//! only limited by the input. Every consumer of the tree recurses, though,
//! so the parser rejects statements whose tree is deeper than [`MAX_DEPTH`]
//! (see the crate documentation), and [`Expr`] and [`Stmt`] are dropped
//! without recursion, so that even a rejected partial tree of any depth is
//! freed safely.

use crate::ast::*;

/// The deepest tree a statement may produce, counted in statement and
/// expression nodes from the module.
///
/// Ignition compiles scripts on gateway threads with the default 1 MiB Java
/// stack. There, Jython fails long before this depth on every construct
/// (measured: `elif` chains of about 2,000 clauses, nested calls of about
/// 300, `not` chains of about 3,900, subscript chains of about 7,000,
/// binary chains of about 6,200, unary chains of about 7,800, attribute
/// chains of about 13,000 because the method gets too large), so a deeper
/// tree is never valid Ignition code.
pub const MAX_DEPTH: usize = 16_384;

/// A cheap expression to leave behind when a child is moved out.
fn placeholder() -> Expr {
    Expr::Name(ExprName {
        range: Default::default(),
        anchor: Default::default(),
        id: Name::default(),
        ctx: ExprContext::Load,
    })
}

/// Whether the expression has no child expressions.
fn is_leaf(expr: &Expr) -> bool {
    matches!(expr, Expr::Num(_) | Expr::Str(_) | Expr::Name(_))
}

fn take_box(expr: &mut Expr, out: &mut Vec<Expr>) {
    if !is_leaf(expr) {
        out.push(std::mem::replace(expr, placeholder()));
    }
}

fn take_opt(expr: &mut Option<Box<Expr>>, out: &mut Vec<Expr>) {
    if let Some(expr) = expr {
        take_box(expr, out);
    }
}

fn take_vec(exprs: &mut Vec<Expr>, out: &mut Vec<Expr>) {
    if exprs.iter().any(|e| !is_leaf(e)) {
        out.append(exprs);
    }
}

fn take_generators(generators: &mut [Comprehension], out: &mut Vec<Expr>) {
    for generator in generators {
        take_box(&mut generator.target, out);
        take_box(&mut generator.iter, out);
        take_vec(&mut generator.ifs, out);
    }
}

fn take_slice(slice: &mut Slice, out: &mut Vec<Expr>) {
    match slice {
        Slice::Ellipsis(_) => {}
        Slice::Slice(s) => {
            take_opt(&mut s.lower, out);
            take_opt(&mut s.upper, out);
            take_opt(&mut s.step, out);
        }
        Slice::ExtSlice(s) => {
            for dim in &mut s.dims {
                take_slice(dim, out);
            }
        }
        Slice::Index(s) => take_box(&mut s.value, out),
    }
}

fn take_arguments(arguments: &mut Arguments, out: &mut Vec<Expr>) {
    take_vec(&mut arguments.args, out);
    take_vec(&mut arguments.defaults, out);
}

/// Moves the child expressions that have children of their own out of
/// `expr` into `out`.
fn take_children(expr: &mut Expr, out: &mut Vec<Expr>) {
    match expr {
        Expr::BoolOp(e) => take_vec(&mut e.values, out),
        Expr::BinOp(e) => {
            take_box(&mut e.left, out);
            take_box(&mut e.right, out);
        }
        Expr::UnaryOp(e) => take_box(&mut e.operand, out),
        Expr::Lambda(e) => {
            take_arguments(&mut e.args, out);
            take_box(&mut e.body, out);
        }
        Expr::IfExp(e) => {
            take_box(&mut e.test, out);
            take_box(&mut e.body, out);
            take_box(&mut e.orelse, out);
        }
        Expr::Dict(e) => {
            take_vec(&mut e.keys, out);
            take_vec(&mut e.values, out);
        }
        Expr::Set(e) => take_vec(&mut e.elts, out),
        Expr::ListComp(e) => {
            take_box(&mut e.elt, out);
            take_generators(&mut e.generators, out);
        }
        Expr::SetComp(e) => {
            take_box(&mut e.elt, out);
            take_generators(&mut e.generators, out);
        }
        Expr::DictComp(e) => {
            take_box(&mut e.key, out);
            take_box(&mut e.value, out);
            take_generators(&mut e.generators, out);
        }
        Expr::GeneratorExp(e) => {
            take_box(&mut e.elt, out);
            take_generators(&mut e.generators, out);
        }
        Expr::Yield(e) => take_opt(&mut e.value, out),
        Expr::Compare(e) => {
            take_box(&mut e.left, out);
            take_vec(&mut e.comparators, out);
        }
        Expr::Call(e) => {
            take_box(&mut e.func, out);
            take_vec(&mut e.args, out);
            for keyword in &mut e.keywords {
                take_box(&mut keyword.value, out);
            }
            take_opt(&mut e.starargs, out);
            take_opt(&mut e.kwargs, out);
        }
        Expr::Repr(e) => take_box(&mut e.value, out),
        Expr::Attribute(e) => take_box(&mut e.value, out),
        Expr::Subscript(e) => {
            take_box(&mut e.value, out);
            take_slice(&mut e.slice, out);
        }
        Expr::List(e) => take_vec(&mut e.elts, out),
        Expr::Tuple(e) => take_vec(&mut e.elts, out),
        Expr::Num(_) | Expr::Str(_) | Expr::Name(_) => {}
    }
}

impl Drop for Expr {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_children(self, &mut pending);
        while let Some(mut expr) = pending.pop() {
            take_children(&mut expr, &mut pending);
            // `expr` is dropped here with only leaf children left.
        }
    }
}

/// Whether the statement has no nested statements.
fn is_simple(stmt: &Stmt) -> bool {
    !matches!(
        stmt,
        Stmt::FunctionDef(_)
            | Stmt::ClassDef(_)
            | Stmt::For(_)
            | Stmt::While(_)
            | Stmt::If(_)
            | Stmt::With(_)
            | Stmt::TryExcept(_)
            | Stmt::TryFinally(_)
    )
}

fn take_body(body: &mut Vec<Stmt>, out: &mut Vec<Stmt>) {
    if body.iter().any(|s| !is_simple(s)) {
        out.append(body);
    }
}

/// Moves the nested compound statements out of `stmt` into `out`.
/// Expressions free themselves without recursion.
fn take_bodies(stmt: &mut Stmt, out: &mut Vec<Stmt>) {
    match stmt {
        Stmt::FunctionDef(s) => take_body(&mut s.body, out),
        Stmt::ClassDef(s) => take_body(&mut s.body, out),
        Stmt::For(s) => {
            take_body(&mut s.body, out);
            take_body(&mut s.orelse, out);
        }
        Stmt::While(s) => {
            take_body(&mut s.body, out);
            take_body(&mut s.orelse, out);
        }
        Stmt::If(s) => {
            take_body(&mut s.body, out);
            take_body(&mut s.orelse, out);
        }
        Stmt::With(s) => take_body(&mut s.body, out),
        Stmt::TryExcept(s) => {
            take_body(&mut s.body, out);
            for handler in &mut s.handlers {
                take_body(&mut handler.body, out);
            }
            take_body(&mut s.orelse, out);
        }
        Stmt::TryFinally(s) => {
            take_body(&mut s.body, out);
            take_body(&mut s.finalbody, out);
        }
        _ => {}
    }
}

impl Drop for Stmt {
    fn drop(&mut self) {
        let mut pending = Vec::new();
        take_bodies(self, &mut pending);
        while let Some(mut stmt) = pending.pop() {
            take_bodies(&mut stmt, &mut pending);
        }
    }
}

/// Calls `f` with every direct child expression of `expr`, with the number
/// of tree levels between them (1, or 2 through a slice, keyword,
/// comprehension or argument list).
fn for_each_child<'a>(expr: &'a Expr, mut f: impl FnMut(&'a Expr, usize)) {
    let slice = |slice: &'a Slice, f: &mut dyn FnMut(&'a Expr, usize)| {
        let mut slices = vec![(slice, 1)];
        while let Some((slice, levels)) = slices.pop() {
            match slice {
                Slice::Ellipsis(_) => {}
                Slice::Slice(s) => {
                    for e in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                        f(e, levels + 1);
                    }
                }
                Slice::ExtSlice(s) => slices.extend(s.dims.iter().map(|d| (d, levels + 1))),
                Slice::Index(s) => f(&s.value, levels + 1),
            }
        }
    };
    let generators = |generators: &'a [Comprehension], f: &mut dyn FnMut(&'a Expr, usize)| {
        for g in generators {
            f(&g.target, 2);
            f(&g.iter, 2);
            for e in &g.ifs {
                f(e, 2);
            }
        }
    };
    match expr {
        Expr::BoolOp(e) => e.values.iter().for_each(|v| f(v, 1)),
        Expr::BinOp(e) => {
            f(&e.left, 1);
            f(&e.right, 1);
        }
        Expr::UnaryOp(e) => f(&e.operand, 1),
        Expr::Lambda(e) => {
            for a in e.args.args.iter().chain(&e.args.defaults) {
                f(a, 2);
            }
            f(&e.body, 1);
        }
        Expr::IfExp(e) => {
            f(&e.test, 1);
            f(&e.body, 1);
            f(&e.orelse, 1);
        }
        Expr::Dict(e) => e.keys.iter().chain(&e.values).for_each(|v| f(v, 1)),
        Expr::Set(e) => e.elts.iter().for_each(|v| f(v, 1)),
        Expr::ListComp(e) => {
            f(&e.elt, 1);
            generators(&e.generators, &mut f);
        }
        Expr::SetComp(e) => {
            f(&e.elt, 1);
            generators(&e.generators, &mut f);
        }
        Expr::DictComp(e) => {
            f(&e.key, 1);
            f(&e.value, 1);
            generators(&e.generators, &mut f);
        }
        Expr::GeneratorExp(e) => {
            f(&e.elt, 1);
            generators(&e.generators, &mut f);
        }
        Expr::Yield(e) => {
            if let Some(v) = &e.value {
                f(v, 1);
            }
        }
        Expr::Compare(e) => {
            f(&e.left, 1);
            e.comparators.iter().for_each(|v| f(v, 1));
        }
        Expr::Call(e) => {
            f(&e.func, 1);
            e.args.iter().for_each(|v| f(v, 1));
            e.keywords.iter().for_each(|k| f(&k.value, 2));
            for v in [&e.starargs, &e.kwargs].into_iter().flatten() {
                f(v, 1);
            }
        }
        Expr::Repr(e) => f(&e.value, 1),
        Expr::Attribute(e) => f(&e.value, 1),
        Expr::Subscript(e) => {
            f(&e.value, 1);
            slice(&e.slice, &mut f);
        }
        Expr::List(e) => e.elts.iter().for_each(|v| f(v, 1)),
        Expr::Tuple(e) => e.elts.iter().for_each(|v| f(v, 1)),
        Expr::Num(_) | Expr::Str(_) | Expr::Name(_) => {}
    }
}

/// The depth of the deepest node under `expr` (1 for a leaf), and that
/// node. Does not recurse.
pub(crate) fn expr_depth(expr: &Expr) -> (usize, &Expr) {
    let mut deepest = (1, expr);
    let mut pending = vec![(expr, 1usize)];
    while let Some((expr, depth)) = pending.pop() {
        if depth > deepest.0 {
            deepest = (depth, expr);
        }
        for_each_child(expr, |child, levels| pending.push((child, depth + levels)));
    }
    deepest
}

/// Calls `f` with every expression that belongs to `stmt` itself (not to
/// nested statements), with the number of tree levels between them.
pub(crate) fn for_each_own_expr<'a>(stmt: &'a Stmt, mut f: impl FnMut(&'a Expr, usize)) {
    let opt = |e: &'a Option<Box<Expr>>, f: &mut dyn FnMut(&'a Expr, usize)| {
        if let Some(e) = e {
            f(e, 1);
        }
    };
    match stmt {
        Stmt::FunctionDef(s) => {
            for e in s.args.args.iter().chain(&s.args.defaults) {
                f(e, 2);
            }
            s.decorator_list.iter().for_each(|e| f(e, 1));
        }
        Stmt::ClassDef(s) => s
            .bases
            .iter()
            .chain(&s.decorator_list)
            .for_each(|e| f(e, 1)),
        Stmt::Return(s) => opt(&s.value, &mut f),
        Stmt::Delete(s) => s.targets.iter().for_each(|e| f(e, 1)),
        Stmt::Assign(s) => {
            s.targets.iter().for_each(|e| f(e, 1));
            f(&s.value, 1);
        }
        Stmt::AugAssign(s) => {
            f(&s.target, 1);
            f(&s.value, 1);
        }
        Stmt::Print(s) => {
            opt(&s.dest, &mut f);
            s.values.iter().for_each(|e| f(e, 1));
        }
        Stmt::For(s) => {
            f(&s.target, 1);
            f(&s.iter, 1);
        }
        Stmt::While(s) => f(&s.test, 1),
        Stmt::If(s) => f(&s.test, 1),
        Stmt::With(s) => {
            f(&s.context_expr, 1);
            opt(&s.optional_vars, &mut f);
        }
        Stmt::Raise(s) => {
            opt(&s.type_, &mut f);
            opt(&s.inst, &mut f);
            opt(&s.tback, &mut f);
        }
        Stmt::TryExcept(s) => {
            for handler in &s.handlers {
                for e in [&handler.type_, &handler.name].into_iter().flatten() {
                    f(e, 2);
                }
            }
        }
        Stmt::Assert(s) => {
            f(&s.test, 1);
            opt(&s.msg, &mut f);
        }
        Stmt::Exec(s) => {
            f(&s.body, 1);
            opt(&s.globals, &mut f);
            opt(&s.locals, &mut f);
        }
        Stmt::Expr(s) => f(&s.value, 1),
        Stmt::TryFinally(_)
        | Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Global(_)
        | Stmt::Pass(_)
        | Stmt::Break(_)
        | Stmt::Continue(_) => {}
    }
}

/// How many levels the tree of `stmt` reaches below the statement itself,
/// ignoring nested statements, and the deepest expression (if any). Does
/// not recurse.
pub(crate) fn own_depth(stmt: &Stmt) -> (usize, Option<&Expr>) {
    let mut deepest = (0, None);
    for_each_own_expr(stmt, |expr, levels| {
        let (depth, node) = expr_depth(expr);
        if depth + levels - 1 > deepest.0 {
            deepest = (depth + levels - 1, Some(node));
        }
    });
    deepest
}

/// The depth of the tree below `body` (statements and expressions). Does
/// not recurse.
pub fn body_depth<'a>(body: &'a [Stmt]) -> usize {
    let mut max = 0;
    let mut pending: Vec<(&'a Stmt, usize)> = body.iter().map(|s| (s, 1)).collect();
    while let Some((stmt, depth)) = pending.pop() {
        max = max.max(depth + own_depth(stmt).0);
        let mut nested = |body: &'a [Stmt], levels: usize| {
            for s in body {
                pending.push((s, depth + levels));
            }
        };
        match stmt {
            Stmt::FunctionDef(s) => nested(&s.body, 1),
            Stmt::ClassDef(s) => nested(&s.body, 1),
            Stmt::For(s) => {
                nested(&s.body, 1);
                nested(&s.orelse, 1);
            }
            Stmt::While(s) => {
                nested(&s.body, 1);
                nested(&s.orelse, 1);
            }
            Stmt::If(s) => {
                nested(&s.body, 1);
                nested(&s.orelse, 1);
            }
            Stmt::With(s) => nested(&s.body, 1),
            Stmt::TryExcept(s) => {
                nested(&s.body, 1);
                for handler in &s.handlers {
                    nested(&handler.body, 2);
                }
                nested(&s.orelse, 1);
            }
            Stmt::TryFinally(s) => {
                nested(&s.body, 1);
                nested(&s.finalbody, 1);
            }
            _ => {}
        }
    }
    max
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_trees_drop_without_recursion() {
        // Built directly: far deeper than the parser allows.
        let mut expr = placeholder();
        for _ in 0..1_000_000 {
            expr = Expr::UnaryOp(ExprUnaryOp {
                range: Default::default(),
                anchor: Default::default(),
                op: UnaryOp::USub,
                operand: Box::new(expr),
            });
        }
        assert_eq!(expr_depth(&expr).0, 1_000_001);
        let mut stmt = Stmt::Pass(StmtPass {
            range: Default::default(),
            anchor: Default::default(),
        });
        for _ in 0..1_000_000 {
            stmt = Stmt::If(StmtIf {
                range: Default::default(),
                anchor: Default::default(),
                test: Box::new(placeholder()),
                body: vec![],
                orelse: vec![stmt],
            });
        }
        let body = vec![stmt];
        assert_eq!(body_depth(&body), 1_000_001);
        drop(body);
        drop(expr);
    }

    #[test]
    fn depth_counts_statements_and_expressions() {
        let parsed = crate::parse_module("if a:\n    x = -(-b)\n");
        // If > Assign > UnaryOp > UnaryOp > Name.
        assert_eq!(body_depth(&parsed.module.body), 5);
    }
}
