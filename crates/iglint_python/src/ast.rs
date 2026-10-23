//! The syntax tree. The node set and field names mirror Python 2.7's `_ast`
//! module exactly, as exposed by Jython.
//!
//! Every statement and expression node carries:
//!
//! * `range`: the node's text in the source. Parentheses around an
//!   expression are not part of it, except for parenthesized tuples and
//!   generator expressions, whose parentheses are part of their syntax.
//! * `anchor`: the token Jython takes `lineno` and `col_offset` from (see
//!   [`Anchor`]).

use iglint_text::{Ranged, TextRange};

use crate::anchor::Anchor;

/// An identifier. Jython identifiers are ASCII.
pub type Name = Box<str>;

/// A name with its source range.
#[derive(Clone, Debug, PartialEq)]
pub struct Identifier {
    pub id: Name,
    pub range: TextRange,
}

impl Identifier {
    pub fn as_str(&self) -> &str {
        &self.id
    }
}

impl Ranged for Identifier {
    fn range(&self) -> TextRange {
        self.range
    }
}

/// `mod`: the root of a tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Mod {
    Module(Module),
    Expression(ModExpression),
}

/// A module (`exec` mode).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub body: Vec<Stmt>,
    pub range: TextRange,
}

/// An expression (`eval` mode).
#[derive(Clone, Debug, PartialEq)]
pub struct ModExpression {
    pub body: Box<Expr>,
    pub range: TextRange,
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// `stmt`.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    FunctionDef(StmtFunctionDef),
    ClassDef(StmtClassDef),
    Return(StmtReturn),
    Delete(StmtDelete),
    Assign(StmtAssign),
    AugAssign(StmtAugAssign),
    Print(StmtPrint),
    For(StmtFor),
    While(StmtWhile),
    If(StmtIf),
    With(StmtWith),
    Raise(StmtRaise),
    TryExcept(StmtTryExcept),
    TryFinally(StmtTryFinally),
    Assert(StmtAssert),
    Import(StmtImport),
    ImportFrom(StmtImportFrom),
    Exec(StmtExec),
    Global(StmtGlobal),
    Expr(StmtExpr),
    Pass(StmtPass),
    Break(StmtBreak),
    Continue(StmtContinue),
}

/// `def name(args): body`, with decorators.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtFunctionDef {
    pub range: TextRange,
    pub anchor: Anchor,
    pub name: Identifier,
    pub args: Box<Arguments>,
    pub body: Vec<Stmt>,
    pub decorator_list: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtClassDef {
    pub range: TextRange,
    pub anchor: Anchor,
    pub name: Identifier,
    pub bases: Vec<Expr>,
    pub body: Vec<Stmt>,
    pub decorator_list: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtReturn {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Option<Box<Expr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtDelete {
    pub range: TextRange,
    pub anchor: Anchor,
    pub targets: Vec<Expr>,
}

/// `a = b = value`.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtAssign {
    pub range: TextRange,
    pub anchor: Anchor,
    pub targets: Vec<Expr>,
    pub value: Box<Expr>,
}

/// `target op= value`. Jython gives the target the `AugStore` context.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtAugAssign {
    pub range: TextRange,
    pub anchor: Anchor,
    pub target: Box<Expr>,
    pub op: Operator,
    pub value: Box<Expr>,
}

/// `print >>dest, values` (Python 2 print statement). `nl` is false when the
/// statement ends with a comma.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtPrint {
    pub range: TextRange,
    pub anchor: Anchor,
    pub dest: Option<Box<Expr>>,
    pub values: Vec<Expr>,
    pub nl: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtFor {
    pub range: TextRange,
    pub anchor: Anchor,
    pub target: Box<Expr>,
    pub iter: Box<Expr>,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtWhile {
    pub range: TextRange,
    pub anchor: Anchor,
    pub test: Box<Expr>,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
}

/// `if`; an `elif` chain is a nested `If` in `orelse`.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtIf {
    pub range: TextRange,
    pub anchor: Anchor,
    pub test: Box<Expr>,
    pub body: Vec<Stmt>,
    pub orelse: Vec<Stmt>,
}

/// `with context_expr as optional_vars: body`. A statement with several
/// items is a chain of nested `With` nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtWith {
    pub range: TextRange,
    pub anchor: Anchor,
    pub context_expr: Box<Expr>,
    pub optional_vars: Option<Box<Expr>>,
    pub body: Vec<Stmt>,
}

/// `raise type, inst, tback`.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtRaise {
    pub range: TextRange,
    pub anchor: Anchor,
    pub type_: Option<Box<Expr>>,
    pub inst: Option<Box<Expr>>,
    pub tback: Option<Box<Expr>>,
}

/// `try`/`except`/`else`. With a `finally` clause it is wrapped in a
/// [`StmtTryFinally`].
#[derive(Clone, Debug, PartialEq)]
pub struct StmtTryExcept {
    pub range: TextRange,
    pub anchor: Anchor,
    pub body: Vec<Stmt>,
    pub handlers: Vec<ExceptHandler>,
    pub orelse: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtTryFinally {
    pub range: TextRange,
    pub anchor: Anchor,
    pub body: Vec<Stmt>,
    pub finalbody: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtAssert {
    pub range: TextRange,
    pub anchor: Anchor,
    pub test: Box<Expr>,
    pub msg: Option<Box<Expr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtImport {
    pub range: TextRange,
    pub anchor: Anchor,
    pub names: Vec<Alias>,
}

/// `from module import names`. Jython reports a relative import without a
/// module name (`from . import x`) as module `''`, which is `None` here.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtImportFrom {
    pub range: TextRange,
    pub anchor: Anchor,
    pub module: Option<Identifier>,
    pub names: Vec<Alias>,
    pub level: u32,
}

/// `exec body in globals, locals`.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtExec {
    pub range: TextRange,
    pub anchor: Anchor,
    pub body: Box<Expr>,
    pub globals: Option<Box<Expr>>,
    pub locals: Option<Box<Expr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtGlobal {
    pub range: TextRange,
    pub anchor: Anchor,
    pub names: Vec<Identifier>,
}

/// An expression statement.
#[derive(Clone, Debug, PartialEq)]
pub struct StmtExpr {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtPass {
    pub range: TextRange,
    pub anchor: Anchor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtBreak {
    pub range: TextRange,
    pub anchor: Anchor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StmtContinue {
    pub range: TextRange,
    pub anchor: Anchor,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// `expr`.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    BoolOp(ExprBoolOp),
    BinOp(ExprBinOp),
    UnaryOp(ExprUnaryOp),
    Lambda(ExprLambda),
    IfExp(ExprIfExp),
    Dict(ExprDict),
    Set(ExprSet),
    ListComp(ExprListComp),
    SetComp(ExprSetComp),
    DictComp(ExprDictComp),
    GeneratorExp(ExprGeneratorExp),
    Yield(ExprYield),
    Compare(ExprCompare),
    Call(ExprCall),
    Repr(ExprRepr),
    Num(ExprNum),
    Str(ExprStr),
    Attribute(ExprAttribute),
    Subscript(ExprSubscript),
    Name(ExprName),
    List(ExprList),
    Tuple(ExprTuple),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprBoolOp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub op: BoolOp,
    pub values: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprBinOp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub left: Box<Expr>,
    pub op: Operator,
    pub right: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprUnaryOp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub op: UnaryOp,
    pub operand: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprLambda {
    pub range: TextRange,
    pub anchor: Anchor,
    pub args: Box<Arguments>,
    pub body: Box<Expr>,
}

/// `body if test else orelse`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprIfExp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub test: Box<Expr>,
    pub body: Box<Expr>,
    pub orelse: Box<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprDict {
    pub range: TextRange,
    pub anchor: Anchor,
    pub keys: Vec<Expr>,
    pub values: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprSet {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elts: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprListComp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elt: Box<Expr>,
    pub generators: Vec<Comprehension>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprSetComp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elt: Box<Expr>,
    pub generators: Vec<Comprehension>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprDictComp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub key: Box<Expr>,
    pub value: Box<Expr>,
    pub generators: Vec<Comprehension>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprGeneratorExp {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elt: Box<Expr>,
    pub generators: Vec<Comprehension>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprYield {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Option<Box<Expr>>,
}

/// `left op1 c1 op2 c2 ...`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprCompare {
    pub range: TextRange,
    pub anchor: Anchor,
    pub left: Box<Expr>,
    pub ops: Vec<CmpOp>,
    pub comparators: Vec<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprCall {
    pub range: TextRange,
    pub anchor: Anchor,
    pub func: Box<Expr>,
    pub args: Vec<Expr>,
    pub keywords: Vec<Keyword>,
    pub starargs: Option<Box<Expr>>,
    pub kwargs: Option<Box<Expr>>,
}

/// `` `value` ``
#[derive(Clone, Debug, PartialEq)]
pub struct ExprRepr {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Box<Expr>,
}

/// A number literal. A minus sign directly in front of a literal is folded
/// into it, as Jython does.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprNum {
    pub range: TextRange,
    pub anchor: Anchor,
    pub n: Number,
}

/// One or more adjacent string literals.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprStr {
    pub range: TextRange,
    pub anchor: Anchor,
    pub s: StringValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprAttribute {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Box<Expr>,
    /// Jython accepts keywords here (`x.print`, `x.from`).
    pub attr: Identifier,
    pub ctx: ExprContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprSubscript {
    pub range: TextRange,
    pub anchor: Anchor,
    pub value: Box<Expr>,
    pub slice: Box<Slice>,
    pub ctx: ExprContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprName {
    pub range: TextRange,
    pub anchor: Anchor,
    pub id: Name,
    pub ctx: ExprContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprList {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elts: Vec<Expr>,
    pub ctx: ExprContext,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExprTuple {
    pub range: TextRange,
    pub anchor: Anchor,
    pub elts: Vec<Expr>,
    pub ctx: ExprContext,
}

/// `expr_context`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExprContext {
    Load,
    Store,
    Del,
    AugLoad,
    AugStore,
    Param,
}

/// `slice`.
#[derive(Clone, Debug, PartialEq)]
pub enum Slice {
    Ellipsis(SliceEllipsis),
    Slice(SliceSlice),
    ExtSlice(SliceExtSlice),
    Index(SliceIndex),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SliceEllipsis {
    pub range: TextRange,
}

/// `lower:upper:step`. `x[a::]` has a `step` of `Name('None')`, positioned at
/// the second colon.
#[derive(Clone, Debug, PartialEq)]
pub struct SliceSlice {
    pub range: TextRange,
    pub lower: Option<Box<Expr>>,
    pub upper: Option<Box<Expr>>,
    pub step: Option<Box<Expr>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SliceExtSlice {
    pub range: TextRange,
    pub dims: Vec<Slice>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SliceIndex {
    pub range: TextRange,
    pub value: Box<Expr>,
}

/// `boolop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BoolOp {
    And,
    Or,
}

/// `operator`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operator {
    Add,
    Sub,
    Mult,
    Div,
    Mod,
    Pow,
    LShift,
    RShift,
    BitOr,
    BitXor,
    BitAnd,
    FloorDiv,
}

/// `unaryop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    Invert,
    Not,
    UAdd,
    USub,
}

/// `cmpop`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CmpOp {
    Eq,
    NotEq,
    Lt,
    LtE,
    Gt,
    GtE,
    Is,
    IsNot,
    In,
    NotIn,
}

/// `comprehension`: `for target in iter if ifs...`.
#[derive(Clone, Debug, PartialEq)]
pub struct Comprehension {
    pub range: TextRange,
    pub target: Expr,
    pub iter: Expr,
    pub ifs: Vec<Expr>,
}

/// `excepthandler`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptHandler {
    pub range: TextRange,
    pub anchor: Anchor,
    pub type_: Option<Box<Expr>>,
    pub name: Option<Box<Expr>>,
    pub body: Vec<Stmt>,
}

/// `arguments`. `args` holds `Name` nodes with the `Param` context, or
/// `Tuple` nodes for tuple parameters (`def f((a, b)):`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Arguments {
    pub range: TextRange,
    pub args: Vec<Expr>,
    pub vararg: Option<Identifier>,
    pub kwarg: Option<Identifier>,
    pub defaults: Vec<Expr>,
}

/// A keyword argument `arg=value`.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyword {
    pub range: TextRange,
    pub arg: Identifier,
    pub value: Expr,
}

/// An imported name. `name` may be dotted (`os.path`).
#[derive(Clone, Debug, PartialEq)]
pub struct Alias {
    pub range: TextRange,
    pub name: Identifier,
    pub asname: Option<Identifier>,
}

// ---------------------------------------------------------------------------
// Literal values
// ---------------------------------------------------------------------------

/// The value of a number literal, as Jython types it.
#[derive(Clone, Debug, PartialEq)]
pub enum Number {
    /// An `int`: a literal without suffix whose value fits in 31 bits and
    /// has at most 11 significant digits.
    Int(i64),
    /// A `long`, as a decimal string with optional leading minus sign.
    Long(Box<str>),
    Float(f64),
    /// An imaginary literal; the real part is zero.
    Complex(f64),
}

/// The value of a string literal (after implicit concatenation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StringValue {
    /// A `str`: bytes. Non-ASCII characters in the source are UTF-8 encoded.
    Bytes(Box<[u8]>),
    /// A `unicode` string. (Jython rejects unicode literals with unpaired
    /// surrogates, so the value is always valid.)
    Unicode(Box<str>),
}

impl StringValue {
    /// Whether the value is a `unicode` string.
    pub fn is_unicode(&self) -> bool {
        !matches!(self, StringValue::Bytes(_))
    }

    /// The value as text, if it is (a byte string holding) valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            StringValue::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
            StringValue::Unicode(text) => Some(text),
        }
    }

    /// The value decoded as Latin-1: every `str` byte becomes the Java
    /// char of the same value, which is how Jython 2.7 turns a byte string
    /// into characters (verified: `'\xe9' == u'é'`). Unlike
    /// [`StringValue::as_str`] this never fails, so a safety scan over the
    /// literal (for example for an introspection attribute name or a
    /// component name a script mentions) cannot silently see nothing for a
    /// byte string that is not valid UTF-8. Use this, not `as_str`,
    /// wherever a missed name would make a fix unsafe.
    pub fn to_latin1(&self) -> std::borrow::Cow<'_, str> {
        match self {
            StringValue::Unicode(text) => std::borrow::Cow::Borrowed(text),
            StringValue::Bytes(bytes) if bytes.is_ascii() => {
                // ASCII is valid UTF-8 and Latin-1 agrees with it there.
                std::borrow::Cow::Borrowed(std::str::from_utf8(bytes).expect("ascii is utf-8"))
            }
            StringValue::Bytes(bytes) => {
                std::borrow::Cow::Owned(bytes.iter().map(|&b| b as char).collect())
            }
        }
    }

    /// The value as text, replacing invalid UTF-8 sequences.
    pub fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        match self {
            StringValue::Bytes(bytes) => String::from_utf8_lossy(bytes),
            StringValue::Unicode(text) => std::borrow::Cow::Borrowed(text),
        }
    }
}

// ---------------------------------------------------------------------------
// Common accessors
// ---------------------------------------------------------------------------

macro_rules! impl_node {
    ($enum:ident { $($variant:ident),* $(,)? }) => {
        impl $enum {
            /// The token Jython takes `lineno` / `col_offset` from.
            pub fn anchor(&self) -> Anchor {
                match self {
                    $($enum::$variant(node) => node.anchor,)*
                }
            }
        }

        impl Ranged for $enum {
            fn range(&self) -> TextRange {
                match self {
                    $($enum::$variant(node) => node.range,)*
                }
            }
        }
    };
}

impl_node!(Stmt {
    FunctionDef,
    ClassDef,
    Return,
    Delete,
    Assign,
    AugAssign,
    Print,
    For,
    While,
    If,
    With,
    Raise,
    TryExcept,
    TryFinally,
    Assert,
    Import,
    ImportFrom,
    Exec,
    Global,
    Expr,
    Pass,
    Break,
    Continue,
});

impl_node!(Expr {
    BoolOp,
    BinOp,
    UnaryOp,
    Lambda,
    IfExp,
    Dict,
    Set,
    ListComp,
    SetComp,
    DictComp,
    GeneratorExp,
    Yield,
    Compare,
    Call,
    Repr,
    Num,
    Str,
    Attribute,
    Subscript,
    Name,
    List,
    Tuple,
});

impl Expr {
    /// The expression context of `Name`, `Attribute`, `Subscript`, `List`
    /// and `Tuple` nodes.
    pub fn ctx(&self) -> Option<ExprContext> {
        match self {
            Expr::Attribute(node) => Some(node.ctx),
            Expr::Subscript(node) => Some(node.ctx),
            Expr::Name(node) => Some(node.ctx),
            Expr::List(node) => Some(node.ctx),
            Expr::Tuple(node) => Some(node.ctx),
            _ => None,
        }
    }

    pub(crate) fn set_ctx(&mut self, ctx: ExprContext) {
        match self {
            Expr::Attribute(node) => node.ctx = ctx,
            Expr::Subscript(node) => node.ctx = ctx,
            Expr::Name(node) => node.ctx = ctx,
            Expr::List(node) => node.ctx = ctx,
            Expr::Tuple(node) => node.ctx = ctx,
            _ => {}
        }
    }

    /// The name if this is a `Name` node.
    pub fn as_name(&self) -> Option<&ExprName> {
        match self {
            Expr::Name(node) => Some(node),
            _ => None,
        }
    }

    /// Jython's node class name.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Expr::BoolOp(_) => "BoolOp",
            Expr::BinOp(_) => "BinOp",
            Expr::UnaryOp(_) => "UnaryOp",
            Expr::Lambda(_) => "Lambda",
            Expr::IfExp(_) => "IfExp",
            Expr::Dict(_) => "Dict",
            Expr::Set(_) => "Set",
            Expr::ListComp(_) => "ListComp",
            Expr::SetComp(_) => "SetComp",
            Expr::DictComp(_) => "DictComp",
            Expr::GeneratorExp(_) => "GeneratorExp",
            Expr::Yield(_) => "Yield",
            Expr::Compare(_) => "Compare",
            Expr::Call(_) => "Call",
            Expr::Repr(_) => "Repr",
            Expr::Num(_) => "Num",
            Expr::Str(_) => "Str",
            Expr::Attribute(_) => "Attribute",
            Expr::Subscript(_) => "Subscript",
            Expr::Name(_) => "Name",
            Expr::List(_) => "List",
            Expr::Tuple(_) => "Tuple",
        }
    }
}

impl Slice {
    pub fn range(&self) -> TextRange {
        match self {
            Slice::Ellipsis(node) => node.range,
            Slice::Slice(node) => node.range,
            Slice::ExtSlice(node) => node.range,
            Slice::Index(node) => node.range,
        }
    }
}

impl Ranged for Slice {
    fn range(&self) -> TextRange {
        Slice::range(self)
    }
}

impl Ranged for Module {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for ExceptHandler {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Arguments {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Keyword {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Alias {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Ranged for Comprehension {
    fn range(&self) -> TextRange {
        self.range
    }
}

impl Operator {
    /// Jython's class name for the operator.
    pub fn name(self) -> &'static str {
        match self {
            Operator::Add => "Add",
            Operator::Sub => "Sub",
            Operator::Mult => "Mult",
            Operator::Div => "Div",
            Operator::Mod => "Mod",
            Operator::Pow => "Pow",
            Operator::LShift => "LShift",
            Operator::RShift => "RShift",
            Operator::BitOr => "BitOr",
            Operator::BitXor => "BitXor",
            Operator::BitAnd => "BitAnd",
            Operator::FloorDiv => "FloorDiv",
        }
    }
}

impl BoolOp {
    pub fn name(self) -> &'static str {
        match self {
            BoolOp::And => "And",
            BoolOp::Or => "Or",
        }
    }
}

impl UnaryOp {
    pub fn name(self) -> &'static str {
        match self {
            UnaryOp::Invert => "Invert",
            UnaryOp::Not => "Not",
            UnaryOp::UAdd => "UAdd",
            UnaryOp::USub => "USub",
        }
    }
}

impl CmpOp {
    pub fn name(self) -> &'static str {
        match self {
            CmpOp::Eq => "Eq",
            CmpOp::NotEq => "NotEq",
            CmpOp::Lt => "Lt",
            CmpOp::LtE => "LtE",
            CmpOp::Gt => "Gt",
            CmpOp::GtE => "GtE",
            CmpOp::Is => "Is",
            CmpOp::IsNot => "IsNot",
            CmpOp::In => "In",
            CmpOp::NotIn => "NotIn",
        }
    }
}

impl ExprContext {
    pub fn name(self) -> &'static str {
        match self {
            ExprContext::Load => "Load",
            ExprContext::Store => "Store",
            ExprContext::Del => "Del",
            ExprContext::AugLoad => "AugLoad",
            ExprContext::AugStore => "AugStore",
            ExprContext::Param => "Param",
        }
    }
}
