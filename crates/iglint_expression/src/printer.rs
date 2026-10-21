//! Printing: an S-expression dump for tests and a source printer.

use std::fmt::Write;

use crate::ast::{Expr, ExprKind, UnaryOp};

/// Compact S-expression form, for tests and debugging:
/// `(+ 1 (* 2 {view.params.x}))`, `(call if (= {a} 1) "yes" "no")`.
pub fn dump(expr: &Expr) -> String {
    let mut out = String::new();
    dump_into(expr, &mut out);
    out
}

fn dump_into(expr: &Expr, out: &mut String) {
    match &expr.kind {
        ExprKind::Integer(v) => {
            let _ = write!(out, "{v}");
        }
        ExprKind::Float(v) => {
            let _ = write!(out, "{v:?}");
        }
        ExprKind::String(s) => {
            let _ = write!(out, "{s:?}");
        }
        ExprKind::Bool(b) => {
            let _ = write!(out, "{b}");
        }
        ExprKind::Null => out.push_str("null"),
        ExprKind::Reference(r) => {
            out.push('{');
            out.push_str(&r.path);
            out.push('}');
        }
        ExprKind::Unary(u) => {
            out.push('(');
            out.push_str(match u.op {
                UnaryOp::Neg => "neg",
                UnaryOp::Not => "not",
                UnaryOp::BitNot => "bitnot",
            });
            out.push(' ');
            dump_into(&u.operand, out);
            out.push(')');
        }
        ExprKind::Binary(b) => {
            out.push('(');
            out.push_str(b.op.symbol());
            out.push(' ');
            dump_into(&b.left, out);
            out.push(' ');
            dump_into(&b.right, out);
            out.push(')');
        }
        ExprKind::Call(c) => {
            out.push_str("(call ");
            out.push_str(&c.name);
            for a in &c.args {
                out.push(' ');
                dump_into(a, out);
            }
            out.push(')');
        }
        ExprKind::Subscript(s) => {
            out.push_str("(index ");
            dump_into(&s.target, out);
            out.push(' ');
            dump_into(&s.index, out);
            if let Some(c) = &s.column {
                out.push(' ');
                dump_into(c, out);
            }
            out.push(')');
        }
        ExprKind::Paren(inner) => {
            out.push_str("(paren ");
            dump_into(inner, out);
            out.push(')');
        }
        ExprKind::Error(items) => {
            out.push_str("(error");
            for e in items {
                out.push(' ');
                dump_into(e, out);
            }
            out.push(')');
        }
    }
}

/// Writes `expr` back as expression source that Ignition parses to the same
/// tree: explicit [`ExprKind::Paren`] nodes are kept and parentheses are
/// added only where precedence requires them. Strings use double quotes.
pub fn to_source(expr: &Expr) -> String {
    let mut out = String::new();
    print(expr, &mut out);
    out
}

/// Escapes `value` as an Ignition double-quoted string literal.
pub fn quote_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Precedence of a node as an operand: binary ops use their own, unary 12,
/// everything else is atomic (13).
fn node_precedence(expr: &Expr) -> u8 {
    match &expr.kind {
        ExprKind::Binary(b) => b.op.precedence(),
        ExprKind::Unary(_) => 12,
        _ => 13,
    }
}

fn print(expr: &Expr, out: &mut String) {
    match &expr.kind {
        ExprKind::Integer(v) => {
            if *v < 0 {
                // Only reachable for hand-built trees: the lexer has no
                // negative literals.
                let _ = write!(out, "({v})");
            } else {
                let _ = write!(out, "{v}");
            }
        }
        ExprKind::Float(v) => {
            let s = if v.is_finite() {
                let s = format!("{v:?}");
                if s.starts_with('-') {
                    format!("({s})")
                } else {
                    s
                }
            } else if v.is_nan() {
                // No literal produces NaN; print an equivalent expression.
                "(0.0 / 0.0)".to_owned()
            } else if *v > 0.0 {
                // Overflows to infinity, as Java's `Double.valueOf` does.
                "1e999".to_owned()
            } else {
                "(-1e999)".to_owned()
            };
            out.push_str(&s);
        }
        ExprKind::String(s) => out.push_str(&quote_string(s)),
        ExprKind::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        ExprKind::Null => out.push_str("null"),
        ExprKind::Reference(r) => {
            out.push('{');
            out.push_str(&r.path);
            out.push('}');
        }
        ExprKind::Unary(u) => {
            out.push_str(u.op.symbol());
            print_operand(&u.operand, 12, out);
        }
        ExprKind::Binary(b) => {
            let p = b.op.precedence();
            print_operand(&b.left, p, out);
            out.push(' ');
            out.push_str(b.op.symbol());
            out.push(' ');
            // Left-associative: an equal-precedence right operand needs
            // parentheses.
            print_operand(&b.right, p + 1, out);
        }
        ExprKind::Call(c) => {
            out.push_str(&c.name);
            out.push('(');
            for (i, a) in c.args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                print(a, out);
            }
            out.push(')');
        }
        ExprKind::Subscript(s) => {
            print_operand(&s.target, 13, out);
            out.push('[');
            print(&s.index, out);
            if let Some(c) = &s.column {
                out.push_str(", ");
                print(c, out);
            }
            out.push(']');
        }
        ExprKind::Paren(inner) => {
            out.push('(');
            print(inner, out);
            out.push(')');
        }
        ExprKind::Error(items) => {
            for (i, e) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                print(e, out);
            }
        }
    }
}

fn print_operand(expr: &Expr, min: u8, out: &mut String) {
    if node_precedence(expr) < min {
        out.push('(');
        print(expr, out);
        out.push(')');
    } else {
        print(expr, out);
    }
}
