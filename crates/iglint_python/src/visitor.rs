//! Traversal of the syntax tree.
//!
//! Implement [`Visitor`] and override the methods for the nodes of interest;
//! call the matching `walk_*` function to continue into the children. The
//! default implementations visit everything in source order.
//!
//! ```
//! use iglint_python::ast::{Expr, ExprCall};
//! use iglint_python::visitor::{walk_expr, Visitor};
//!
//! #[derive(Default)]
//! struct Calls(usize);
//!
//! impl<'a> Visitor<'a> for Calls {
//!     fn visit_expr(&mut self, expr: &'a Expr) {
//!         if let Expr::Call(_) = expr {
//!             self.0 += 1;
//!         }
//!         walk_expr(self, expr);
//!     }
//! }
//!
//! let parsed = iglint_python::parse_module("f(g(1), x.h())\n");
//! let mut calls = Calls::default();
//! calls.visit_module(&parsed.module);
//! assert_eq!(calls.0, 3);
//! ```

use crate::ast::*;

/// A read-only tree visitor.
pub trait Visitor<'a> {
    fn visit_module(&mut self, module: &'a Module) {
        walk_body(self, &module.body);
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        walk_expr(self, expr);
    }

    fn visit_slice(&mut self, slice: &'a Slice) {
        walk_slice(self, slice);
    }

    fn visit_except_handler(&mut self, handler: &'a ExceptHandler) {
        walk_except_handler(self, handler);
    }

    fn visit_arguments(&mut self, arguments: &'a Arguments) {
        walk_arguments(self, arguments);
    }

    fn visit_keyword(&mut self, keyword: &'a Keyword) {
        self.visit_expr(&keyword.value);
    }

    fn visit_alias(&mut self, _alias: &'a Alias) {}

    fn visit_comprehension(&mut self, comprehension: &'a Comprehension) {
        walk_comprehension(self, comprehension);
    }
}

/// Visits every statement of a body.
pub fn walk_body<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, body: &'a [Stmt]) {
    for stmt in body {
        visitor.visit_stmt(stmt);
    }
}

fn walk_exprs<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, exprs: &'a [Expr]) {
    for expr in exprs {
        visitor.visit_expr(expr);
    }
}

fn walk_opt_expr<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, expr: Option<&'a Expr>) {
    if let Some(expr) = expr {
        visitor.visit_expr(expr);
    }
}

/// Visits the children of a statement, in source order.
pub fn walk_stmt<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, stmt: &'a Stmt) {
    match stmt {
        Stmt::FunctionDef(s) => {
            walk_exprs(visitor, &s.decorator_list);
            visitor.visit_arguments(&s.args);
            walk_body(visitor, &s.body);
        }
        Stmt::ClassDef(s) => {
            walk_exprs(visitor, &s.decorator_list);
            walk_exprs(visitor, &s.bases);
            walk_body(visitor, &s.body);
        }
        Stmt::Return(s) => walk_opt_expr(visitor, s.value.as_deref()),
        Stmt::Delete(s) => walk_exprs(visitor, &s.targets),
        Stmt::Assign(s) => {
            walk_exprs(visitor, &s.targets);
            visitor.visit_expr(&s.value);
        }
        Stmt::AugAssign(s) => {
            visitor.visit_expr(&s.target);
            visitor.visit_expr(&s.value);
        }
        Stmt::Print(s) => {
            walk_opt_expr(visitor, s.dest.as_deref());
            walk_exprs(visitor, &s.values);
        }
        Stmt::For(s) => {
            visitor.visit_expr(&s.target);
            visitor.visit_expr(&s.iter);
            walk_body(visitor, &s.body);
            walk_body(visitor, &s.orelse);
        }
        Stmt::While(s) => {
            visitor.visit_expr(&s.test);
            walk_body(visitor, &s.body);
            walk_body(visitor, &s.orelse);
        }
        Stmt::If(s) => {
            visitor.visit_expr(&s.test);
            walk_body(visitor, &s.body);
            walk_body(visitor, &s.orelse);
        }
        Stmt::With(s) => {
            visitor.visit_expr(&s.context_expr);
            walk_opt_expr(visitor, s.optional_vars.as_deref());
            walk_body(visitor, &s.body);
        }
        Stmt::Raise(s) => {
            walk_opt_expr(visitor, s.type_.as_deref());
            walk_opt_expr(visitor, s.inst.as_deref());
            walk_opt_expr(visitor, s.tback.as_deref());
        }
        Stmt::TryExcept(s) => {
            walk_body(visitor, &s.body);
            for handler in &s.handlers {
                visitor.visit_except_handler(handler);
            }
            walk_body(visitor, &s.orelse);
        }
        Stmt::TryFinally(s) => {
            walk_body(visitor, &s.body);
            walk_body(visitor, &s.finalbody);
        }
        Stmt::Assert(s) => {
            visitor.visit_expr(&s.test);
            walk_opt_expr(visitor, s.msg.as_deref());
        }
        Stmt::Import(s) => {
            for alias in &s.names {
                visitor.visit_alias(alias);
            }
        }
        Stmt::ImportFrom(s) => {
            for alias in &s.names {
                visitor.visit_alias(alias);
            }
        }
        Stmt::Exec(s) => {
            visitor.visit_expr(&s.body);
            walk_opt_expr(visitor, s.globals.as_deref());
            walk_opt_expr(visitor, s.locals.as_deref());
        }
        Stmt::Expr(s) => visitor.visit_expr(&s.value),
        Stmt::Global(_) | Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) => {}
    }
}

/// Visits the children of an expression, in source order.
pub fn walk_expr<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, expr: &'a Expr) {
    match expr {
        Expr::BoolOp(e) => walk_exprs(visitor, &e.values),
        Expr::BinOp(e) => {
            visitor.visit_expr(&e.left);
            visitor.visit_expr(&e.right);
        }
        Expr::UnaryOp(e) => visitor.visit_expr(&e.operand),
        Expr::Lambda(e) => {
            visitor.visit_arguments(&e.args);
            visitor.visit_expr(&e.body);
        }
        Expr::IfExp(e) => {
            visitor.visit_expr(&e.body);
            visitor.visit_expr(&e.test);
            visitor.visit_expr(&e.orelse);
        }
        Expr::Dict(e) => {
            for (key, value) in e.keys.iter().zip(&e.values) {
                visitor.visit_expr(key);
                visitor.visit_expr(value);
            }
        }
        Expr::Set(e) => walk_exprs(visitor, &e.elts),
        Expr::ListComp(e) => {
            visitor.visit_expr(&e.elt);
            for generator in &e.generators {
                visitor.visit_comprehension(generator);
            }
        }
        Expr::SetComp(e) => {
            visitor.visit_expr(&e.elt);
            for generator in &e.generators {
                visitor.visit_comprehension(generator);
            }
        }
        Expr::DictComp(e) => {
            visitor.visit_expr(&e.key);
            visitor.visit_expr(&e.value);
            for generator in &e.generators {
                visitor.visit_comprehension(generator);
            }
        }
        Expr::GeneratorExp(e) => {
            visitor.visit_expr(&e.elt);
            for generator in &e.generators {
                visitor.visit_comprehension(generator);
            }
        }
        Expr::Yield(e) => walk_opt_expr(visitor, e.value.as_deref()),
        Expr::Compare(e) => {
            visitor.visit_expr(&e.left);
            walk_exprs(visitor, &e.comparators);
        }
        Expr::Call(e) => {
            visitor.visit_expr(&e.func);
            walk_exprs(visitor, &e.args);
            for keyword in &e.keywords {
                visitor.visit_keyword(keyword);
            }
            walk_opt_expr(visitor, e.starargs.as_deref());
            walk_opt_expr(visitor, e.kwargs.as_deref());
        }
        Expr::Repr(e) => visitor.visit_expr(&e.value),
        Expr::Attribute(e) => visitor.visit_expr(&e.value),
        Expr::Subscript(e) => {
            visitor.visit_expr(&e.value);
            visitor.visit_slice(&e.slice);
        }
        Expr::List(e) => walk_exprs(visitor, &e.elts),
        Expr::Tuple(e) => walk_exprs(visitor, &e.elts),
        Expr::Num(_) | Expr::Str(_) | Expr::Name(_) => {}
    }
}

/// Visits the expressions of a slice.
pub fn walk_slice<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, slice: &'a Slice) {
    match slice {
        Slice::Ellipsis(_) => {}
        Slice::Slice(s) => {
            walk_opt_expr(visitor, s.lower.as_deref());
            walk_opt_expr(visitor, s.upper.as_deref());
            walk_opt_expr(visitor, s.step.as_deref());
        }
        Slice::ExtSlice(s) => {
            for dim in &s.dims {
                visitor.visit_slice(dim);
            }
        }
        Slice::Index(s) => visitor.visit_expr(&s.value),
    }
}

pub fn walk_except_handler<'a, V: Visitor<'a> + ?Sized>(
    visitor: &mut V,
    handler: &'a ExceptHandler,
) {
    walk_opt_expr(visitor, handler.type_.as_deref());
    walk_opt_expr(visitor, handler.name.as_deref());
    walk_body(visitor, &handler.body);
}

/// Visits parameters and default values, in source order.
pub fn walk_arguments<'a, V: Visitor<'a> + ?Sized>(visitor: &mut V, arguments: &'a Arguments) {
    let first_default = arguments
        .args
        .len()
        .saturating_sub(arguments.defaults.len());
    for (index, arg) in arguments.args.iter().enumerate() {
        visitor.visit_expr(arg);
        if index >= first_default {
            visitor.visit_expr(&arguments.defaults[index - first_default]);
        }
    }
}

pub fn walk_comprehension<'a, V: Visitor<'a> + ?Sized>(
    visitor: &mut V,
    comprehension: &'a Comprehension,
) {
    visitor.visit_expr(&comprehension.target);
    visitor.visit_expr(&comprehension.iter);
    walk_exprs(visitor, &comprehension.ifs);
}
