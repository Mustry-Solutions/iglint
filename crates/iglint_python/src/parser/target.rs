//! Assignment targets: expression contexts and Jython's target checks
//! (`GrammarActions.checkAssign` and friends).

use iglint_text::Ranged;

use crate::ast::{Expr, ExprContext};
use crate::error::ParseError;

/// Gives a parsed expression the context it would have had if Jython had
/// parsed it as a target (`testlist[Store]`, `exprlist[Store]`, ...).
///
/// Jython passes the context to every atom parsed in the same `expr` rule
/// invocation: names, list and tuple displays (recursively), and the last
/// trailer of an atom. Operands of operators inside a target receive it too,
/// which is visible when Jython accepts such a target (`-a = 1`,
/// `for a + b in c`).
pub(super) fn apply_context(expr: &mut Expr, ctx: ExprContext) {
    match expr {
        Expr::Name(node) => node.ctx = ctx,
        Expr::Attribute(node) => node.ctx = ctx,
        Expr::Subscript(node) => node.ctx = ctx,
        Expr::List(node) => {
            node.ctx = ctx;
            for elt in &mut node.elts {
                apply_context(elt, ctx);
            }
        }
        Expr::Tuple(node) => {
            node.ctx = ctx;
            for elt in &mut node.elts {
                apply_context(elt, ctx);
            }
        }
        Expr::BinOp(node) => {
            apply_context(&mut node.left, ctx);
            apply_context(&mut node.right, ctx);
        }
        Expr::UnaryOp(node) => apply_context(&mut node.operand, ctx),
        Expr::BoolOp(node) => {
            for value in &mut node.values {
                apply_context(value, ctx);
            }
        }
        Expr::Compare(node) => {
            apply_context(&mut node.left, ctx);
            for comparator in &mut node.comparators {
                apply_context(comparator, ctx);
            }
        }
        // The `else` branch is parsed as `test[Load]`.
        Expr::IfExp(node) => {
            apply_context(&mut node.body, ctx);
            apply_context(&mut node.test, ctx);
        }
        Expr::ListComp(node) => apply_context(&mut node.elt, ctx),
        Expr::GeneratorExp(node) => {
            apply_context(&mut node.elt, ctx);
            node.elt.set_ctx(ExprContext::Load);
        }
        Expr::Lambda(_)
        | Expr::Dict(_)
        | Expr::Set(_)
        | Expr::SetComp(_)
        | Expr::DictComp(_)
        | Expr::Yield(_)
        | Expr::Call(_)
        | Expr::Repr(_)
        | Expr::Num(_)
        | Expr::Str(_) => {}
    }
}

fn error(message: &str, expr: &Expr) -> Option<ParseError> {
    Some(ParseError::syntax(message, expr.range(), expr.anchor()))
}

/// `checkGenericAssign`.
fn check_generic_assign(expr: &Expr) -> Option<ParseError> {
    match expr {
        Expr::Name(name) if &*name.id == "None" => error("assignment to None", expr),
        Expr::GeneratorExp(_) => error("can't assign to generator expression", expr),
        Expr::Num(_) => error("can't assign to number", expr),
        Expr::Str(_) => error("can't assign to string", expr),
        Expr::Yield(_) => error("can't assign to yield expression", expr),
        Expr::BinOp(_) | Expr::BoolOp(_) => error("can't assign to operator", expr),
        Expr::Lambda(_) => error("can't assign to lambda", expr),
        Expr::Call(_) => error("can't assign to function call", expr),
        Expr::Repr(_) => error("can't assign to repr", expr),
        Expr::IfExp(_) => error("can't assign to conditional expression", expr),
        Expr::ListComp(_) => error("can't assign to list comprehension", expr),
        Expr::SetComp(_) => error("can't assign to set comprehension", expr),
        Expr::DictComp(_) => error("can't assign to dict comprehension", expr),
        _ => None,
    }
}

/// `checkAssign`: the first problem with an assignment target, if any.
pub(super) fn check_assign(expr: &Expr) -> Option<ParseError> {
    if let Some(error) = check_generic_assign(expr) {
        return Some(error);
    }
    match expr {
        Expr::Tuple(tuple) => {
            if tuple.elts.is_empty() {
                return error("can't assign to ()", expr);
            }
            tuple.elts.iter().find_map(check_assign)
        }
        Expr::List(list) => list.elts.iter().find_map(check_assign),
        _ => None,
    }
}

/// `checkAugAssign`.
pub(super) fn check_aug_assign(expr: &Expr) -> Option<ParseError> {
    if let Some(error) = check_generic_assign(expr) {
        return Some(error);
    }
    match expr {
        Expr::Tuple(_) => error("assignment to tuple illegal for augmented assignment", expr),
        Expr::List(_) => error("assignment to list illegal for augmented assignment", expr),
        _ => None,
    }
}

/// `checkDelete`.
pub(super) fn check_delete(expr: &Expr) -> Option<ParseError> {
    match expr {
        Expr::Call(_) => error("can't delete function call", expr),
        Expr::Num(_) => error("can't delete number", expr),
        Expr::Str(_) => error("can't delete string", expr),
        Expr::Tuple(tuple) => {
            if tuple.elts.is_empty() {
                return error("can't delete ()", expr);
            }
            tuple.elts.iter().find_map(check_delete)
        }
        Expr::List(list) => list.elts.iter().find_map(check_delete),
        _ => None,
    }
}
