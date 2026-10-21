//! Visitor over the expression tree.

use crate::ast::{Binary, BoundRef, Call, Expr, ExprKind, Subscript, Unary};

/// Override the hooks you need; call the matching `walk_*` function to keep
/// descending.
pub trait Visitor {
    fn visit_expr(&mut self, expr: &Expr) {
        walk_expr(self, expr);
    }

    fn visit_reference(&mut self, _expr: &Expr, _reference: &BoundRef) {}

    fn visit_call(&mut self, expr: &Expr, call: &Call) {
        walk_call(self, expr, call);
    }

    fn visit_unary(&mut self, _expr: &Expr, unary: &Unary) {
        self.visit_expr(&unary.operand);
    }

    fn visit_binary(&mut self, _expr: &Expr, binary: &Binary) {
        self.visit_expr(&binary.left);
        self.visit_expr(&binary.right);
    }

    fn visit_subscript(&mut self, _expr: &Expr, subscript: &Subscript) {
        self.visit_expr(&subscript.target);
        self.visit_expr(&subscript.index);
        if let Some(c) = &subscript.column {
            self.visit_expr(c);
        }
    }

    /// Literals, including strings.
    fn visit_literal(&mut self, _expr: &Expr) {}

    fn visit_error(&mut self, _expr: &Expr, items: &[Expr]) {
        for e in items {
            self.visit_expr(e);
        }
    }
}

/// Dispatches `expr` to the specific hook.
pub fn walk_expr<V: Visitor + ?Sized>(v: &mut V, expr: &Expr) {
    match &expr.kind {
        ExprKind::Integer(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Null => v.visit_literal(expr),
        ExprKind::Reference(r) => v.visit_reference(expr, r),
        ExprKind::Unary(u) => v.visit_unary(expr, u),
        ExprKind::Binary(b) => v.visit_binary(expr, b),
        ExprKind::Call(c) => v.visit_call(expr, c),
        ExprKind::Subscript(s) => v.visit_subscript(expr, s),
        ExprKind::Paren(inner) => v.visit_expr(inner),
        ExprKind::Error(items) => v.visit_error(expr, items),
    }
}

/// Visits the arguments of a call.
pub fn walk_call<V: Visitor + ?Sized>(v: &mut V, _expr: &Expr, call: &Call) {
    for a in &call.args {
        v.visit_expr(a);
    }
}
