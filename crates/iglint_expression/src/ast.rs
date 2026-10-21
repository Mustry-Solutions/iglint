//! Expression syntax tree.

use std::fmt;

use iglint_text::{Ranged, TextRange};

use crate::reference::{Reference, ReferenceContext};

/// An expression node with its byte range in the expression source.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub range: TextRange,
    pub kind: ExprKind,
}

impl Ranged for Expr {
    fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    /// Decimal (`42`) or hexadecimal (`0x2A`) integer, a Java `long`.
    Integer(i64),
    /// Floating point literal, a Java `double`. Includes integers written
    /// with leading zeros: `007` is the double `7.0` in Ignition.
    Float(f64),
    /// Decoded string literal.
    String(String),
    Bool(bool),
    /// `null` or `none`.
    Null,
    /// `{...}`: a property, tag or context reference.
    Reference(BoundRef),
    Unary(Box<Unary>),
    Binary(Box<Binary>),
    Call(Call),
    /// `target[index]` or `target[row, column]` (datasets).
    Subscript(Box<Subscript>),
    /// Parentheses. Ignition drops them; they are kept here for ranges and
    /// faithful printing.
    Paren(Box<Expr>),
    /// Placeholder produced by error recovery, holding any sub-expressions
    /// that could still be parsed.
    Error(Vec<Expr>),
}

/// The raw text of a `{...}` reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundRef {
    /// Text between the braces, verbatim (Ignition does not trim it).
    pub path: String,
    /// Range of `path` in the expression source.
    pub path_range: TextRange,
}

impl BoundRef {
    /// Classifies and parses the path for the context the expression runs in.
    pub fn resolve(&self, context: ReferenceContext) -> Reference {
        crate::reference::classify(&self.path, self.path_range.start(), context)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `!x`
    Not,
    /// `~x`
    BitNot,
}

impl UnaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Unary {
    pub op: UnaryOp,
    pub op_range: TextRange,
    pub operand: Expr,
}

/// Binary operators. All are left-associative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `||`
    Or,
    /// `&&`
    And,
    /// `xor` (bitwise)
    BitXor,
    /// `|`
    BitOr,
    /// `&`
    BitAnd,
    /// `=`
    Eq,
    /// `!=`
    NotEq,
    /// `like`
    Like,
    Lt,
    LtEq,
    Gt,
    GtEq,
    /// `<<`
    Shl,
    /// `>>`
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    /// `%`
    Mod,
    /// `^` (power)
    Pow,
}

impl BinaryOp {
    /// Binding strength from Ignition's CUP grammar (`HMIEL.cup`); higher
    /// binds tighter. Unary operators bind tighter than all of these, and
    /// subscripts tighter still.
    pub fn precedence(self) -> u8 {
        match self {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::BitXor => 3,
            BinaryOp::BitOr => 4,
            BinaryOp::BitAnd => 5,
            BinaryOp::Eq | BinaryOp::NotEq | BinaryOp::Like => 6,
            BinaryOp::Lt | BinaryOp::LtEq | BinaryOp::Gt | BinaryOp::GtEq => 7,
            BinaryOp::Shl | BinaryOp::Shr => 8,
            BinaryOp::Add | BinaryOp::Sub => 9,
            BinaryOp::Mul | BinaryOp::Div => 10,
            BinaryOp::Mod | BinaryOp::Pow => 11,
        }
    }

    /// The operator as written (keywords in lower case).
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Or => "||",
            BinaryOp::And => "&&",
            BinaryOp::BitXor => "xor",
            BinaryOp::BitOr => "|",
            BinaryOp::BitAnd => "&",
            BinaryOp::Eq => "=",
            BinaryOp::NotEq => "!=",
            BinaryOp::Like => "like",
            BinaryOp::Lt => "<",
            BinaryOp::LtEq => "<=",
            BinaryOp::Gt => ">",
            BinaryOp::GtEq => ">=",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "^",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinaryOp::Eq
                | BinaryOp::NotEq
                | BinaryOp::Like
                | BinaryOp::Lt
                | BinaryOp::LtEq
                | BinaryOp::Gt
                | BinaryOp::GtEq
        )
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinaryOp::And | BinaryOp::Or)
    }

    pub fn is_bitwise(self) -> bool {
        matches!(
            self,
            BinaryOp::BitXor | BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Shl | BinaryOp::Shr
        )
    }

    pub fn is_arithmetic(self) -> bool {
        matches!(
            self,
            BinaryOp::Add
                | BinaryOp::Sub
                | BinaryOp::Mul
                | BinaryOp::Div
                | BinaryOp::Mod
                | BinaryOp::Pow
        )
    }
}

impl fmt::Display for BinaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binary {
    pub op: BinaryOp,
    pub op_range: TextRange,
    pub left: Expr,
    pub right: Expr,
}

/// `name(args...)`. Function names are matched case-insensitively by
/// Ignition.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub name_range: TextRange,
    pub args: Vec<Expr>,
}

impl Call {
    /// The name in lower case, as Ignition looks functions up.
    pub fn name_lower(&self) -> String {
        self.name.to_lowercase()
    }

    /// Case-insensitive name comparison.
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Subscript {
    pub target: Expr,
    pub index: Expr,
    /// Column of a two-index dataset subscript `ds[row, column]`.
    pub column: Option<Expr>,
}

impl Expr {
    /// Direct children in source order.
    pub fn children(&self) -> Vec<&Expr> {
        match &self.kind {
            ExprKind::Unary(u) => vec![&u.operand],
            ExprKind::Binary(b) => vec![&b.left, &b.right],
            ExprKind::Call(c) => c.args.iter().collect(),
            ExprKind::Subscript(s) => {
                let mut v = vec![&s.target, &s.index];
                if let Some(c) = &s.column {
                    v.push(c);
                }
                v
            }
            ExprKind::Paren(e) => vec![e],
            ExprKind::Error(items) => items.iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Calls `f` on this node and every descendant, parents first.
    pub fn walk<'e>(&'e self, f: &mut impl FnMut(&'e Expr)) {
        f(self);
        match &self.kind {
            ExprKind::Unary(u) => u.operand.walk(f),
            ExprKind::Binary(b) => {
                b.left.walk(f);
                b.right.walk(f);
            }
            ExprKind::Call(c) => c.args.iter().for_each(|a| a.walk(f)),
            ExprKind::Subscript(s) => {
                s.target.walk(f);
                s.index.walk(f);
                if let Some(c) = &s.column {
                    c.walk(f);
                }
            }
            ExprKind::Paren(e) => e.walk(f),
            ExprKind::Error(items) => items.iter().for_each(|e| e.walk(f)),
            _ => {}
        }
    }

    /// Every `{...}` reference, in source order.
    pub fn references(&self) -> Vec<&BoundRef> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let ExprKind::Reference(r) = &e.kind {
                out.push(r);
            }
        });
        out
    }

    /// Every function call, outer calls before inner ones.
    pub fn calls(&self) -> Vec<&Call> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let ExprKind::Call(c) = &e.kind {
                out.push(c);
            }
        });
        out
    }

    /// The expression with any enclosing parentheses removed.
    pub fn unparenthesized(&self) -> &Expr {
        let mut e = self;
        while let ExprKind::Paren(inner) = &e.kind {
            e = inner;
        }
        e
    }

    /// True for literals (numbers, strings, booleans, null).
    pub fn is_literal(&self) -> bool {
        matches!(
            self.kind,
            ExprKind::Integer(_)
                | ExprKind::Float(_)
                | ExprKind::String(_)
                | ExprKind::Bool(_)
                | ExprKind::Null
        )
    }
}
