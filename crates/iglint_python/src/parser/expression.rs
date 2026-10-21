//! Expressions: `test` and everything below it.

use iglint_text::{Ranged, TextRange, TextSize};

use super::{Failure, PResult, Parser};
use crate::anchor::Anchor;
use crate::ast::{
    Arguments, BoolOp, CmpOp, Comprehension, Expr, ExprAttribute, ExprBinOp, ExprBoolOp, ExprCall,
    ExprCompare, ExprContext, ExprDict, ExprDictComp, ExprGeneratorExp, ExprIfExp, ExprLambda,
    ExprList, ExprListComp, ExprName, ExprNum, ExprRepr, ExprSet, ExprSetComp, ExprStr,
    ExprSubscript, ExprTuple, ExprUnaryOp, ExprYield, Identifier, Keyword, Number, Operator, Slice,
    SliceEllipsis, SliceExtSlice, SliceIndex, SliceSlice, UnaryOp,
};
use crate::error::{ParseError, ParseErrorKind};
use crate::literal;
use crate::token::{Token, TokenKind};

/// A parsed expression together with its first token, which may be an
/// opening parenthesis that is not part of the expression's own range.
/// Jython anchors several nodes at the first token of an operand.
#[derive(Debug)]
pub(super) struct Operand {
    pub(super) expr: Expr,
    /// The anchor of the first token.
    pub(super) first: Anchor,
    /// The start of the first token.
    pub(super) start: TextSize,
}

impl Operand {
    fn plain(expr: Expr, token: &Token) -> Self {
        Operand {
            expr,
            first: token.anchor,
            start: token.range.start(),
        }
    }
}

/// Binary operator levels below `comparison`, from loosest to tightest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    BitOr,
    BitXor,
    BitAnd,
    Shift,
    Arith,
    Term,
}

/// The level and operator of a binary operator token.
fn binary_operator(kind: TokenKind) -> Option<(Level, Operator)> {
    use TokenKind::*;
    Some(match kind {
        Vbar => (Level::BitOr, Operator::BitOr),
        Circumflex => (Level::BitXor, Operator::BitXor),
        Amper => (Level::BitAnd, Operator::BitAnd),
        LeftShift => (Level::Shift, Operator::LShift),
        RightShift => (Level::Shift, Operator::RShift),
        Plus => (Level::Arith, Operator::Add),
        Minus => (Level::Arith, Operator::Sub),
        Star => (Level::Term, Operator::Mult),
        Slash => (Level::Term, Operator::Div),
        Percent => (Level::Term, Operator::Mod),
        DoubleSlash => (Level::Term, Operator::FloorDiv),
        _ => return None,
    })
}

/// An operand on the operator stack of [`Parser::expr`].
struct Chain {
    operand: Operand,
    /// The level of the binary operation this operand is, if it was built
    /// by the stack (not parenthesized).
    level: Option<Level>,
    /// The anchor of the first operand node of that operation chain.
    chain_first_node: Anchor,
}

impl Chain {
    fn new(operand: Operand) -> Self {
        Chain {
            operand,
            level: None,
            chain_first_node: Anchor::default(),
        }
    }
}

/// Tokens that may start a `test` (Jython's prediction sets; `print` only
/// when it is a name).
pub(super) fn starts_test(kind: TokenKind, print_function: bool) -> bool {
    use TokenKind::*;
    match kind {
        Backquote | Lsqb | Lbrace | Lpar | Minus | Name | Not | Plus | Tilde | Complex | Float
        | Int | Lambda | LongInt | String => true,
        Print => print_function,
        _ => false,
    }
}

/// The second-token sets of Jython's `k=2` loop decisions, which include
/// `print` unconditionally.
pub(super) fn starts_test_loosely(kind: TokenKind) -> bool {
    kind == TokenKind::Print || starts_test(kind, false)
}

/// Tokens that may start an `expr` (no `not`, no `lambda`).
fn starts_expr(kind: TokenKind, print_function: bool) -> bool {
    !matches!(kind, TokenKind::Not | TokenKind::Lambda) && starts_test(kind, print_function)
}

/// What may follow a complete `term` anywhere in the grammar (the follow set
/// of Jython's decision after the first factor of a term). Anything else is
/// a syntax error at that point.
fn follows_term(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        EndOfFile
            | AltNotEqual
            | Amper
            | AmperEqual
            | And
            | As
            | Equal
            | Backquote
            | Circumflex
            | CircumflexEqual
            | Colon
            | Comma
            | DoubleSlashEqual
            | DoubleStarEqual
            | EqEqual
            | For
            | Greater
            | GreaterEqual
            | If
            | In
            | Is
            | LeftShift
            | LeftShiftEqual
            | Less
            | LessEqual
            | Minus
            | MinusEqual
            | Newline
            | Not
            | NotEqual
            | Or
            | Else
            | PercentEqual
            | Plus
            | PlusEqual
            | Rsqb
            | Rbrace
            | RightShift
            | RightShiftEqual
            | Rpar
            | Semi
            | SlashEqual
            | StarEqual
            | Vbar
            | VbarEqual
            // The term operators themselves.
            | Star
            | Slash
            | Percent
            | DoubleSlash
    )
}

impl Parser<'_> {
    pub(super) fn starts_test(&mut self) -> bool {
        let kind = self.kind();
        starts_test(kind, self.print_function)
    }

    // -- test ---------------------------------------------------------------

    /// `test: or_test ['if' or_test 'else' test] | lambdef`
    pub(super) fn test(&mut self) -> PResult<Operand> {
        self.nested(Self::test_inner)
    }

    fn test_inner(&mut self) -> PResult<Operand> {
        if self.at(TokenKind::Lambda) {
            return self.lambdef();
        }
        if !self.starts_test() {
            return Err(self.invalid_syntax());
        }
        let body = self.or_test()?;
        match self.kind() {
            TokenKind::If => {
                // Syntactic predicate `(IF or_test ORELSE)=>`.
                let condition = self.speculate(|parser| {
                    parser.bump();
                    let condition = parser.or_test()?;
                    if parser.at(TokenKind::Else) {
                        Ok(condition)
                    } else {
                        Err(Failure)
                    }
                });
                let Some(condition) = condition else {
                    return Ok(body);
                };
                self.bump();
                let orelse = self.test()?;
                let range = self.range_from(body.start);
                Ok(Operand {
                    first: body.first,
                    start: body.start,
                    expr: Expr::IfExp(ExprIfExp {
                        range,
                        anchor: body.first,
                        test: Box::new(condition.expr),
                        body: Box::new(body.expr),
                        orelse: Box::new(orelse.expr),
                    }),
                })
            }
            // `else` can follow an `or_test` (inside a conditional
            // expression) but never a `test`.
            TokenKind::Else => Err(self.invalid_syntax()),
            _ => Ok(body),
        }
    }

    /// `or_test: and_test ('or' and_test)*`
    pub(super) fn or_test(&mut self) -> PResult<Operand> {
        self.bool_op(TokenKind::Or)
    }

    fn bool_op(&mut self, operator: TokenKind) -> PResult<Operand> {
        let first = if operator == TokenKind::Or {
            self.bool_op(TokenKind::And)?
        } else {
            self.not_test()?
        };
        if !self.at(operator) {
            return Ok(first);
        }
        let mut values = vec![first.expr];
        while self.eat(operator).is_some() {
            let operand = if operator == TokenKind::Or {
                self.bool_op(TokenKind::And)?
            } else {
                self.not_test()?
            };
            values.push(operand.expr);
        }
        let op = if operator == TokenKind::Or {
            BoolOp::Or
        } else {
            BoolOp::And
        };
        Ok(Operand {
            expr: Expr::BoolOp(ExprBoolOp {
                range: self.range_from(first.start),
                anchor: first.first,
                op,
                values,
            }),
            first: first.first,
            start: first.start,
        })
    }

    /// `not_test: 'not' not_test | comparison`
    fn not_test(&mut self) -> PResult<Operand> {
        // Prefix `not`s are collected first so that long chains do not
        // recurse.
        let mut nots = Vec::new();
        while self.at(TokenKind::Not) {
            nots.push(self.bump());
        }
        if !self.starts_test() || self.at(TokenKind::Lambda) {
            return Err(self.invalid_syntax());
        }
        let mut operand = self.comparison()?;
        for not in nots.into_iter().rev() {
            operand = Operand::plain(
                Expr::UnaryOp(ExprUnaryOp {
                    range: self.range_from(not.range.start()),
                    anchor: not.anchor,
                    op: UnaryOp::Not,
                    operand: Box::new(operand.expr),
                }),
                &not,
            );
        }
        Ok(operand)
    }

    /// `comparison: expr (comp_op expr)*`
    fn comparison(&mut self) -> PResult<Operand> {
        let left = self.expr()?;
        let mut ops = Vec::new();
        let mut comparators = Vec::new();
        loop {
            let op = match self.kind() {
                TokenKind::Less => CmpOp::Lt,
                TokenKind::Greater => CmpOp::Gt,
                TokenKind::EqEqual => CmpOp::Eq,
                TokenKind::GreaterEqual => CmpOp::GtE,
                TokenKind::LessEqual => CmpOp::LtE,
                TokenKind::AltNotEqual | TokenKind::NotEqual => CmpOp::NotEq,
                TokenKind::In => CmpOp::In,
                TokenKind::Not => {
                    self.bump();
                    self.expect(TokenKind::In)?;
                    CmpOp::NotIn
                }
                TokenKind::Is => {
                    self.bump();
                    if self.eat(TokenKind::Not).is_some() {
                        CmpOp::IsNot
                    } else {
                        CmpOp::Is
                    }
                }
                _ => break,
            };
            if !matches!(op, CmpOp::NotIn | CmpOp::Is | CmpOp::IsNot) {
                self.bump();
            }
            ops.push(op);
            comparators.push(self.expr()?.expr);
        }
        if ops.is_empty() {
            return Ok(left);
        }
        Ok(Operand {
            expr: Expr::Compare(ExprCompare {
                range: self.range_from(left.start),
                anchor: left.first,
                left: Box::new(left.expr),
                ops,
                comparators,
            }),
            first: left.first,
            start: left.start,
        })
    }

    /// `expr: xor_expr ('|' xor_expr)*` and the binary levels below it,
    /// parsed with an operator stack instead of one recursion per level.
    pub(super) fn expr(&mut self) -> PResult<Operand> {
        if !starts_expr(self.kind(), self.print_function) {
            return Err(self.invalid_syntax());
        }
        let mut pending: Vec<(Chain, Operator, Level, Anchor)> = Vec::new();
        let mut current = Chain::new(self.factor()?);
        loop {
            // Jython checks after every term that the next token may follow
            // one.
            if !follows_term(self.kind()) {
                return Err(self.invalid_syntax());
            }
            let next = binary_operator(self.kind());
            while let Some((_, _, level, _)) = pending.last() {
                if next.is_some_and(|(next_level, _)| next_level > *level) {
                    break;
                }
                let Some((left, op, level, operator)) = pending.pop() else {
                    break;
                };
                current = self.combine(left, op, level, operator, current);
            }
            let Some((level, op)) = next else {
                break;
            };
            let operator = self.bump();
            pending.push((current, op, level, operator.anchor));
            current = Chain::new(self.factor()?);
        }
        Ok(current.operand)
    }

    /// Builds `left op right` with Jython's anchor: the first token of the
    /// left operand for the first operation of a chain; for later ones the
    /// operator (`<<`, `>>`, `+`, `-`, `*`, `/`, `%`, `//`) or the first
    /// operand of the chain (`|`, `^`, `&`).
    fn combine(
        &self,
        left: Chain,
        op: Operator,
        level: Level,
        operator: Anchor,
        right: Chain,
    ) -> Chain {
        let continues = left.level == Some(level);
        let anchor = if !continues {
            left.operand.first
        } else {
            match level {
                Level::BitOr | Level::BitXor | Level::BitAnd => left.chain_first_node,
                _ => operator,
            }
        };
        let chain_first_node = if continues {
            left.chain_first_node
        } else {
            left.operand.expr.anchor()
        };
        let expr = Expr::BinOp(ExprBinOp {
            range: self.range_from(left.operand.start),
            anchor,
            left: Box::new(left.operand.expr),
            op,
            right: Box::new(right.operand.expr),
        });
        Chain {
            operand: Operand {
                expr,
                first: left.operand.first,
                start: left.operand.start,
            },
            level: Some(level),
            chain_first_node,
        }
    }

    /// `factor: ('+'|'-'|'~') factor | power`
    fn factor(&mut self) -> PResult<Operand> {
        // Prefix operators are collected first so that long chains do not
        // recurse; they apply innermost first.
        let mut operators: Vec<(UnaryOp, Token)> = Vec::new();
        loop {
            let op = match self.kind() {
                TokenKind::Plus => UnaryOp::UAdd,
                TokenKind::Minus => UnaryOp::USub,
                TokenKind::Tilde => UnaryOp::Invert,
                TokenKind::Backquote
                | TokenKind::Lsqb
                | TokenKind::Lbrace
                | TokenKind::Lpar
                | TokenKind::Name
                | TokenKind::Complex
                | TokenKind::Float
                | TokenKind::Int
                | TokenKind::LongInt
                | TokenKind::Print
                | TokenKind::String => break,
                _ => return Err(self.invalid_syntax()),
            };
            operators.push((op, self.bump()));
        }
        let mut operand = self.power()?;
        for (op, token) in operators.into_iter().rev() {
            let range = self.range_from(token.range.start());
            let folded = match (&operand.expr, op) {
                // Jython folds a minus into a number literal.
                (Expr::Num(num), UnaryOp::USub) => literal::negate(&num.n).map(|n| {
                    Expr::Num(ExprNum {
                        range,
                        anchor: num.anchor,
                        n,
                    })
                }),
                _ => None,
            };
            let expr = match folded {
                Some(expr) => expr,
                None => Expr::UnaryOp(ExprUnaryOp {
                    range,
                    anchor: token.anchor,
                    op,
                    operand: Box::new(operand.expr),
                }),
            };
            operand = Operand::plain(expr, &token);
        }
        Ok(operand)
    }

    /// `power: atom trailer* ['**' factor]`
    fn power(&mut self) -> PResult<Operand> {
        let atom = self.atom()?;
        let first = atom.first;
        let start = atom.start;
        let mut expr = atom.expr;
        loop {
            expr = match self.kind() {
                TokenKind::Lpar => self.call_trailer(expr, first, start)?,
                TokenKind::Lsqb => self.subscript_trailer(expr, first, start)?,
                TokenKind::Dot => self.attribute_trailer(expr, first, start)?,
                _ => break,
            };
        }
        if self.eat(TokenKind::DoubleStar).is_some() {
            let right = self.nested(Self::factor)?;
            expr = Expr::BinOp(ExprBinOp {
                range: self.range_from(start),
                anchor: first,
                left: Box::new(expr),
                op: Operator::Pow,
                right: Box::new(right.expr),
            });
        }
        Ok(Operand { expr, first, start })
    }

    // The trailers and atoms are separate functions so that the frames on
    // the recursion path through nested expressions stay small.

    /// `'(' [arglist] ')'` after `func`.
    #[inline(never)]
    fn call_trailer(&mut self, func: Expr, first: Anchor, start: TextSize) -> PResult<Expr> {
        self.bump();
        let (args, keywords, starargs, kwargs) = self.call_arguments()?;
        self.expect(TokenKind::Rpar)?;
        Ok(Expr::Call(ExprCall {
            range: self.range_from(start),
            anchor: first,
            func: Box::new(func),
            args,
            keywords,
            starargs,
            kwargs,
        }))
    }

    /// `'[' subscriptlist ']'` after `value`.
    #[inline(never)]
    fn subscript_trailer(&mut self, value: Expr, first: Anchor, start: TextSize) -> PResult<Expr> {
        self.bump();
        let slice = self.subscriptlist(first)?;
        self.expect(TokenKind::Rsqb)?;
        Ok(Expr::Subscript(ExprSubscript {
            range: self.range_from(start),
            anchor: first,
            value: Box::new(value),
            slice: Box::new(slice),
            ctx: ExprContext::Load,
        }))
    }

    /// `'.' NAME` after `value`.
    #[inline(never)]
    fn attribute_trailer(&mut self, value: Expr, first: Anchor, start: TextSize) -> PResult<Expr> {
        self.bump();
        let attr = self.attribute_name()?;
        Ok(Expr::Attribute(ExprAttribute {
            range: self.range_from(start),
            anchor: first,
            value: Box::new(value),
            attr,
            ctx: ExprContext::Load,
        }))
    }

    /// The name after `.`: Jython accepts every keyword there.
    pub(super) fn attribute_name(&mut self) -> PResult<Identifier> {
        let token = self.lt1();
        if token.kind == TokenKind::Name || token.kind.is_keyword() {
            self.bump();
            Ok(self.identifier(&token))
        } else {
            Err(self.expected("attribute name"))
        }
    }

    pub(super) fn identifier(&self, token: &Token) -> Identifier {
        Identifier {
            id: self.text(token).into(),
            range: token.range,
        }
    }

    /// `atom`
    fn atom(&mut self) -> PResult<Operand> {
        let token = self.lt1();
        let expr = match token.kind {
            TokenKind::Lpar => self.parenthesized(&token)?,
            TokenKind::Lsqb => self.list_display(&token)?,
            TokenKind::Lbrace => self.brace_display(&token)?,
            TokenKind::Backquote => self.repr_display(&token)?,
            TokenKind::Name => {
                self.bump();
                self.name(&token)
            }
            TokenKind::Print if self.print_function => {
                self.bump();
                self.name(&token)
            }
            TokenKind::Int | TokenKind::LongInt | TokenKind::Float | TokenKind::Complex => {
                self.number(&token)
            }
            TokenKind::String => self.strings(&token),
            _ => return Err(self.invalid_syntax()),
        };
        Ok(Operand::plain(expr, &token))
    }

    /// `'(' [yield_expr | testlist_gexp] ')'`
    #[inline(never)]
    fn parenthesized(&mut self, lpar: &Token) -> PResult<Expr> {
        self.bump();
        let inner = match self.kind() {
            TokenKind::Yield => Some(self.yield_expr()?),
            TokenKind::Rpar => None,
            _ if self.starts_test() => Some(self.testlist_gexp(lpar)?),
            _ => return Err(self.invalid_syntax()),
        };
        self.expect(TokenKind::Rpar)?;
        let range = self.range_from(lpar.range.start());
        let mut expr = match inner {
            Some(expr) => expr,
            None => Expr::Tuple(ExprTuple {
                range,
                anchor: lpar.anchor,
                elts: Vec::new(),
                ctx: ExprContext::Load,
            }),
        };
        // Parentheses belong to the syntax of tuples and generator
        // expressions.
        match &mut expr {
            Expr::Tuple(tuple) => tuple.range = range,
            Expr::GeneratorExp(generator) => generator.range = range,
            _ => {}
        }
        Ok(expr)
    }

    /// `'[' [listmaker] ']'`
    #[inline(never)]
    fn list_display(&mut self, lsqb: &Token) -> PResult<Expr> {
        self.bump();
        if self.at(TokenKind::Rsqb) {
            self.bump();
            Ok(Expr::List(ExprList {
                range: self.range_from(lsqb.range.start()),
                anchor: lsqb.anchor,
                elts: Vec::new(),
                ctx: ExprContext::Load,
            }))
        } else if self.starts_test() {
            self.listmaker(lsqb)
        } else {
            Err(self.invalid_syntax())
        }
    }

    /// `'{' [dictorsetmaker] '}'`
    #[inline(never)]
    fn brace_display(&mut self, lbrace: &Token) -> PResult<Expr> {
        self.bump();
        if self.at(TokenKind::Rbrace) {
            self.bump();
            Ok(Expr::Dict(ExprDict {
                range: self.range_from(lbrace.range.start()),
                anchor: lbrace.anchor,
                keys: Vec::new(),
                values: Vec::new(),
            }))
        } else if self.starts_test() {
            self.dictorsetmaker(lbrace)
        } else {
            Err(self.invalid_syntax())
        }
    }

    /// `` '`' testlist1 '`' ``
    #[inline(never)]
    fn repr_display(&mut self, backquote: &Token) -> PResult<Expr> {
        self.bump();
        let value = self.testlist1()?;
        self.expect(TokenKind::Backquote)?;
        Ok(Expr::Repr(ExprRepr {
            range: self.range_from(backquote.range.start()),
            anchor: backquote.anchor,
            value: Box::new(value),
        }))
    }

    /// A number literal.
    #[inline(never)]
    fn number(&mut self, token: &Token) -> Expr {
        self.bump();
        let text = self.text(token);
        let n = match token.kind {
            TokenKind::Float => Number::Float(literal::float(text)),
            TokenKind::Complex => Number::Complex(literal::imaginary(text)),
            _ => match literal::integer(text) {
                Ok(n) => n,
                Err(error) => {
                    self.report(ParseError::new(
                        ParseErrorKind::InternalError,
                        error.message(),
                        token.range,
                        token.anchor,
                    ));
                    Number::Int(0)
                }
            },
        };
        Expr::Num(ExprNum {
            range: token.range,
            anchor: token.anchor,
            n,
        })
    }

    /// One or more adjacent string literals.
    #[inline(never)]
    fn strings(&mut self, first: &Token) -> Expr {
        let mut tokens = Vec::new();
        while self.at(TokenKind::String) {
            tokens.push(self.bump());
        }
        let range = self.range_from(first.range.start());
        let source = self.source;
        let s = match literal::decode_strings(
            tokens.iter().map(|token| token.range.slice(source)),
            self.encoding,
            self.unicode_literals,
        ) {
            Ok(value) => value,
            Err(error) => {
                self.report(ParseError::new(
                    ParseErrorKind::InternalError,
                    error.message(),
                    range,
                    first.anchor,
                ));
                crate::ast::StringValue::Bytes(Box::default())
            }
        };
        Expr::Str(ExprStr {
            range,
            anchor: first.anchor,
            s,
        })
    }

    pub(super) fn name(&self, token: &Token) -> Expr {
        Expr::Name(ExprName {
            range: token.range,
            anchor: token.anchor,
            id: self.text(token).into(),
            ctx: ExprContext::Load,
        })
    }

    /// The contents of `( ... )` after the opening parenthesis:
    /// `testlist_gexp: test ( comp_for | (',' test)* [','] )`
    fn testlist_gexp(&mut self, lpar: &Token) -> PResult<Expr> {
        let first = self.test()?;
        match self.kind() {
            TokenKind::Comma => {
                let mut elts = vec![first.expr];
                while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                    self.bump();
                    elts.push(self.test()?.expr);
                }
                self.eat(TokenKind::Comma);
                Ok(Expr::Tuple(ExprTuple {
                    range: self.range_from(lpar.range.start()),
                    anchor: first.first,
                    elts,
                    ctx: ExprContext::Load,
                }))
            }
            TokenKind::For => {
                let generators = self.comp_for()?;
                let mut elt = first.expr;
                elt.set_ctx(ExprContext::Load);
                Ok(Expr::GeneratorExp(ExprGeneratorExp {
                    range: self.range_from(lpar.range.start()),
                    anchor: first.first,
                    elt: Box::new(elt),
                    generators,
                }))
            }
            TokenKind::Rpar => Ok(first.expr),
            _ => Err(self.invalid_syntax()),
        }
    }

    /// The contents of `[ ... ]`:
    /// `listmaker: test ( list_for | (',' test)* ) [',']`
    fn listmaker(&mut self, lsqb: &Token) -> PResult<Expr> {
        let first = self.test()?;
        let expr = match self.kind() {
            TokenKind::For => {
                let generators = self.list_for()?;
                self.eat(TokenKind::Comma);
                self.expect(TokenKind::Rsqb)?;
                Expr::ListComp(ExprListComp {
                    range: self.range_from(lsqb.range.start()),
                    anchor: first.first,
                    elt: Box::new(first.expr),
                    generators,
                })
            }
            TokenKind::Comma | TokenKind::Rsqb => {
                let mut elts = vec![first.expr];
                while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                    self.bump();
                    elts.push(self.test()?.expr);
                }
                self.eat(TokenKind::Comma);
                self.expect(TokenKind::Rsqb)?;
                Expr::List(ExprList {
                    range: self.range_from(lsqb.range.start()),
                    anchor: lsqb.anchor,
                    elts,
                    ctx: ExprContext::Load,
                })
            }
            _ => return Err(self.invalid_syntax()),
        };
        Ok(expr)
    }

    /// The contents of `{ ... }`.
    fn dictorsetmaker(&mut self, lbrace: &Token) -> PResult<Expr> {
        let key = self.test()?;
        let expr = match self.kind() {
            TokenKind::Colon => {
                self.bump();
                let value = self.test()?;
                match self.kind() {
                    TokenKind::For => {
                        let generators = self.comp_for()?;
                        self.eat(TokenKind::Comma);
                        self.expect(TokenKind::Rbrace)?;
                        Expr::DictComp(ExprDictComp {
                            range: self.range_from(lbrace.range.start()),
                            anchor: key.first,
                            key: Box::new(key.expr),
                            value: Box::new(value.expr),
                            generators,
                        })
                    }
                    TokenKind::Comma | TokenKind::Rbrace => {
                        let mut keys = vec![key.expr];
                        let mut values = vec![value.expr];
                        while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                            self.bump();
                            keys.push(self.test()?.expr);
                            self.expect(TokenKind::Colon)?;
                            values.push(self.test()?.expr);
                        }
                        self.eat(TokenKind::Comma);
                        self.expect(TokenKind::Rbrace)?;
                        Expr::Dict(ExprDict {
                            range: self.range_from(lbrace.range.start()),
                            anchor: lbrace.anchor,
                            keys,
                            values,
                        })
                    }
                    _ => return Err(self.invalid_syntax()),
                }
            }
            TokenKind::Comma | TokenKind::Rbrace => {
                let mut elts = vec![key.expr];
                while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                    self.bump();
                    elts.push(self.test()?.expr);
                }
                self.eat(TokenKind::Comma);
                self.expect(TokenKind::Rbrace)?;
                Expr::Set(ExprSet {
                    range: self.range_from(lbrace.range.start()),
                    anchor: lbrace.anchor,
                    elts,
                })
            }
            TokenKind::For => {
                let generators = self.comp_for()?;
                self.expect(TokenKind::Rbrace)?;
                let mut elt = key.expr;
                elt.set_ctx(ExprContext::Load);
                Expr::SetComp(ExprSetComp {
                    range: self.range_from(lbrace.range.start()),
                    anchor: lbrace.anchor,
                    elt: Box::new(elt),
                    generators,
                })
            }
            _ => return Err(self.invalid_syntax()),
        };
        Ok(expr)
    }

    /// `list_for: 'for' exprlist 'in' testlist [list_iter]`, with the
    /// following `list_for` / `list_if` clauses.
    fn list_for(&mut self) -> PResult<Vec<Comprehension>> {
        self.comprehension_clauses(true)
    }

    /// `comp_for: 'for' exprlist 'in' or_test [comp_iter]`, with the
    /// following clauses.
    pub(super) fn comp_for(&mut self) -> PResult<Vec<Comprehension>> {
        self.comprehension_clauses(false)
    }

    fn comprehension_clauses(&mut self, list: bool) -> PResult<Vec<Comprehension>> {
        let mut generators: Vec<Comprehension> = Vec::new();
        loop {
            let for_token = self.expect(TokenKind::For)?;
            let mut target = self.exprlist()?;
            super::target::apply_context(&mut target, ExprContext::Store);
            self.expect(TokenKind::In)?;
            let iter = if list {
                self.testlist()?.expr
            } else {
                self.or_test()?.expr
            };
            let mut comprehension = Comprehension {
                range: self.range_from(for_token.range.start()),
                target,
                iter,
                ifs: Vec::new(),
            };
            while self.eat(TokenKind::If).is_some() {
                comprehension.ifs.push(self.test()?.expr);
                comprehension.range = self.range_from(for_token.range.start());
            }
            generators.push(comprehension);
            if !self.at(TokenKind::For) {
                return Ok(generators);
            }
        }
    }

    /// The arguments of a call after `(`, up to `)`.
    #[allow(clippy::type_complexity)]
    pub(super) fn call_arguments(
        &mut self,
    ) -> PResult<(
        Vec<Expr>,
        Vec<Keyword>,
        Option<Box<Expr>>,
        Option<Box<Expr>>,
    )> {
        let kind = self.kind();
        if kind == TokenKind::Rpar {
            return Ok((Vec::new(), Vec::new(), None, None));
        }
        if !(self.starts_test() || matches!(kind, TokenKind::Star | TokenKind::DoubleStar)) {
            return Err(self.invalid_syntax());
        }
        let arglist = self.arglist()?;
        let keywords = self.make_keywords(arglist.keywords);
        Ok((arglist.args, keywords, arglist.starargs, arglist.kwargs))
    }

    /// `arglist`. Keyword arguments are returned unchecked (Jython checks
    /// them when it builds the call).
    pub(super) fn arglist(&mut self) -> PResult<ArgList> {
        let start = self.lt1();
        let mut list = ArgList::default();
        let mut generators = 0;
        match self.kind() {
            TokenKind::Star => {
                self.bump();
                list.starargs = Some(Box::new(self.test()?.expr));
                while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                    self.bump();
                    self.argument(&mut list, &mut generators, false, true)?;
                }
                if self.at(TokenKind::Comma) {
                    self.bump();
                    self.expect(TokenKind::DoubleStar)?;
                    list.kwargs = Some(Box::new(self.test()?.expr));
                }
                return Ok(list);
            }
            TokenKind::DoubleStar => {
                self.bump();
                list.kwargs = Some(Box::new(self.test()?.expr));
                return Ok(list);
            }
            _ => {}
        }
        self.argument(&mut list, &mut generators, true, false)?;
        while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
            self.bump();
            self.argument(&mut list, &mut generators, false, false)?;
        }
        if self.at(TokenKind::Comma) {
            self.bump();
            match self.kind() {
                TokenKind::Star => {
                    self.bump();
                    list.starargs = Some(Box::new(self.test()?.expr));
                    while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
                        self.bump();
                        self.argument(&mut list, &mut generators, false, true)?;
                    }
                    if self.at(TokenKind::Comma) {
                        self.bump();
                        self.expect(TokenKind::DoubleStar)?;
                        list.kwargs = Some(Box::new(self.test()?.expr));
                    }
                }
                TokenKind::DoubleStar => {
                    self.bump();
                    list.kwargs = Some(Box::new(self.test()?.expr));
                }
                _ => {}
            }
        }
        if list.args.len() > 1 && generators > 0 {
            self.report(ParseError::syntax(
                "Generator expression must be parenthesized if not sole argument",
                start.range,
                start.anchor,
            ));
        }
        Ok(list)
    }

    /// `argument: test [comp_for] | test '=' test`
    fn argument(
        &mut self,
        list: &mut ArgList,
        generators: &mut usize,
        first: bool,
        after_star: bool,
    ) -> PResult<()> {
        let key = self.test()?;
        match self.kind() {
            TokenKind::Equal => {
                self.bump();
                let value = self.test()?;
                if let Expr::Name(new) = &key.expr {
                    let repeated = list
                        .keywords
                        .iter()
                        .any(|(old, _)| matches!(old, Expr::Name(old) if old.id == new.id));
                    if repeated {
                        self.report(ParseError::syntax(
                            "keyword arguments repeated",
                            new.range,
                            new.anchor,
                        ));
                    }
                }
                list.keywords.push((key.expr, value.expr));
            }
            TokenKind::For => {
                let for_token = self.lt1();
                let generators_list = self.comp_for()?;
                if !first {
                    self.report(ParseError::syntax(
                        "Generator expression must be parenthesized if not sole argument",
                        for_token.range,
                        for_token.anchor,
                    ));
                }
                *generators += 1;
                let mut elt = key.expr;
                elt.set_ctx(ExprContext::Load);
                list.args.push(Expr::GeneratorExp(ExprGeneratorExp {
                    range: self.range_from(key.start),
                    anchor: key.first,
                    elt: Box::new(elt),
                    generators: generators_list,
                }));
            }
            TokenKind::Comma | TokenKind::Rpar => {
                if !list.keywords.is_empty() {
                    self.report(ParseError::syntax(
                        "non-keyword arg after keyword arg",
                        key.expr.range(),
                        key.expr.anchor(),
                    ));
                } else if after_star {
                    self.report(ParseError::syntax(
                        "only named arguments may follow *expression",
                        key.expr.range(),
                        key.expr.anchor(),
                    ));
                }
                list.args.push(key.expr);
            }
            _ => return Err(self.invalid_syntax()),
        }
        Ok(())
    }

    /// Jython's `makeKeywords`: every keyword must be a plain name.
    pub(super) fn make_keywords(&mut self, keywords: Vec<(Expr, Expr)>) -> Vec<Keyword> {
        let mut result = Vec::with_capacity(keywords.len());
        for (key, value) in keywords {
            if let Some(error) = super::target::check_assign(&key) {
                self.report(error);
            }
            match key {
                Expr::Name(name) => {
                    let range = TextRange::new(name.range.start(), value.range().end());
                    result.push(Keyword {
                        range,
                        arg: Identifier {
                            id: name.id,
                            range: name.range,
                        },
                        value,
                    });
                }
                other => {
                    self.report(ParseError::syntax(
                        "keyword must be a name",
                        other.range(),
                        other.anchor(),
                    ));
                }
            }
        }
        result
    }

    /// `subscriptlist` inside `[ ... ]`. `begin` is the first token of the
    /// subscripted expression, which anchors an index tuple.
    fn subscriptlist(&mut self, begin: Anchor) -> PResult<Slice> {
        let start = self.lt1().range.start();
        let mut slices = vec![self.subscript()?];
        let mut comma = false;
        while self.at(TokenKind::Comma) && starts_subscript(self.peek_kind(2)) {
            self.bump();
            comma = true;
            slices.push(self.subscript()?);
        }
        if self.eat(TokenKind::Comma).is_some() {
            comma = true;
        }
        let range = self.range_from(start);
        if !comma {
            return Ok(slices
                .pop()
                .unwrap_or(Slice::Ellipsis(SliceEllipsis { range })));
        }
        if slices.iter().all(|slice| matches!(slice, Slice::Index(_))) {
            let elts = slices
                .into_iter()
                .filter_map(|slice| match slice {
                    Slice::Index(index) => Some(*index.value),
                    _ => None,
                })
                .collect();
            return Ok(Slice::Index(SliceIndex {
                range,
                value: Box::new(Expr::Tuple(ExprTuple {
                    range,
                    anchor: begin,
                    elts,
                    ctx: ExprContext::Load,
                })),
            }));
        }
        Ok(Slice::ExtSlice(SliceExtSlice {
            range,
            dims: slices,
        }))
    }

    /// `subscript: '.' '.' '.' | test | [test] ':' [test] [sliceop]`
    fn subscript(&mut self) -> PResult<Slice> {
        let start = self.lt1().range.start();
        match self.kind() {
            TokenKind::Dot => {
                self.bump();
                self.expect(TokenKind::Dot)?;
                self.expect(TokenKind::Dot)?;
                return Ok(Slice::Ellipsis(SliceEllipsis {
                    range: self.range_from(start),
                }));
            }
            TokenKind::Colon => {
                self.bump();
                return self.slice_rest(start, None);
            }
            _ => {}
        }
        if !self.starts_test() {
            return Err(self.invalid_syntax());
        }
        // Syntactic predicate `(test COLON)=>`.
        let (lower, colon) = self.region(|parser| {
            let lower = parser.test()?.expr;
            let colon = parser.eat(TokenKind::Colon).is_some();
            Ok((lower, colon))
        })?;
        if colon {
            return self.slice_rest(start, Some(lower));
        }
        Ok(Slice::Index(SliceIndex {
            range: self.range_from(start),
            value: Box::new(lower),
        }))
    }

    /// The rest of a slice after its first colon.
    fn slice_rest(&mut self, start: TextSize, lower: Option<Expr>) -> PResult<Slice> {
        let upper = if self.starts_test() {
            Some(Box::new(self.test()?.expr))
        } else {
            None
        };
        let step = if self.at(TokenKind::Colon) {
            let colon = self.bump();
            if self.starts_test() {
                Some(Box::new(self.test()?.expr))
            } else if matches!(self.kind(), TokenKind::Comma | TokenKind::Rsqb) {
                // `x[a::]`: Jython's step is the name `None` at the colon.
                Some(Box::new(Expr::Name(ExprName {
                    range: colon.range,
                    anchor: colon.anchor,
                    id: "None".into(),
                    ctx: ExprContext::Load,
                })))
            } else {
                return Err(self.invalid_syntax());
            }
        } else {
            None
        };
        Ok(Slice::Slice(SliceSlice {
            range: self.range_from(start),
            lower: lower.map(Box::new),
            upper,
            step,
        }))
    }

    /// `testlist: test (',' test)* [',']`
    pub(super) fn testlist(&mut self) -> PResult<Operand> {
        // Jython's predicate `(test COMMA)=>` scans the first element and a
        // comma.
        let (first, comma) = self.region(|parser| {
            let first = parser.test()?;
            let comma = parser.eat(TokenKind::Comma).is_some();
            Ok((first, comma))
        })?;
        if !comma {
            return Ok(first);
        }
        // Jython decides this loop with a DFA that also validates what
        // follows: a token that can neither continue the tuple nor follow a
        // `testlist` is an error right there.
        let mut elts = vec![first.expr];
        loop {
            // After a comma.
            let kind = self.kind();
            if !starts_test_loosely(kind) {
                if kind == TokenKind::Comma || follows_testlist(kind) {
                    break;
                }
                return Err(self.invalid_syntax());
            }
            elts.push(self.test()?.expr);
            // After an element.
            let kind = self.kind();
            if kind == TokenKind::Comma {
                self.bump();
            } else if follows_testlist(kind) {
                break;
            } else {
                return Err(self.invalid_syntax());
            }
        }
        Ok(Operand {
            expr: Expr::Tuple(ExprTuple {
                range: self.range_from(first.start),
                anchor: first.first,
                elts,
                ctx: ExprContext::Load,
            }),
            first: first.first,
            start: first.start,
        })
    }

    /// `testlist1: test (',' test)*` (inside backquotes)
    fn testlist1(&mut self) -> PResult<Expr> {
        let first = self.test()?;
        if !self.at(TokenKind::Comma) {
            return Ok(first.expr);
        }
        let mut elts = vec![first.expr];
        while self.eat(TokenKind::Comma).is_some() {
            elts.push(self.test()?.expr);
        }
        Ok(Expr::Tuple(ExprTuple {
            range: self.range_from(first.start),
            anchor: first.first,
            elts,
            ctx: ExprContext::Load,
        }))
    }

    /// `exprlist: expr (',' expr)* [',']`, in the `Load` context.
    pub(super) fn exprlist(&mut self) -> PResult<Expr> {
        // Jython's predicate `(expr COMMA)=>` scans the first element and a
        // comma.
        let (first, comma) = self.region(|parser| {
            let first = parser.expr()?;
            let comma = parser.eat(TokenKind::Comma).is_some();
            Ok((first, comma))
        })?;
        if !comma {
            return Ok(first.expr);
        }
        let mut elts = vec![first.expr];
        while starts_expr_loosely(self.kind()) {
            elts.push(self.expr()?.expr);
            if self.eat(TokenKind::Comma).is_none() {
                break;
            }
        }
        Ok(Expr::Tuple(ExprTuple {
            range: self.range_from(first.start),
            anchor: first.first,
            elts,
            ctx: ExprContext::Load,
        }))
    }

    /// `yield_expr: 'yield' [testlist]`
    pub(super) fn yield_expr(&mut self) -> PResult<Expr> {
        let token = self.expect(TokenKind::Yield)?;
        let value = if self.starts_test() {
            Some(Box::new(self.testlist()?.expr))
        } else {
            None
        };
        Ok(Expr::Yield(ExprYield {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            value,
        }))
    }

    /// `lambdef: 'lambda' [varargslist] ':' test`
    fn lambdef(&mut self) -> PResult<Operand> {
        let token = self.bump();
        let args = if matches!(
            self.kind(),
            TokenKind::Name | TokenKind::Lpar | TokenKind::Star | TokenKind::DoubleStar
        ) {
            self.varargslist()?
        } else {
            Arguments {
                range: TextRange::empty(self.previous_end),
                ..Arguments::default()
            }
        };
        self.expect(TokenKind::Colon)?;
        let body = self.test()?;
        Ok(Operand::plain(
            Expr::Lambda(ExprLambda {
                range: self.range_from(token.range.start()),
                anchor: token.anchor,
                args: Box::new(args),
                body: Box::new(body.expr),
            }),
            &token,
        ))
    }
}

/// Arguments of a call before Jython validates the keywords.
#[derive(Default)]
pub(super) struct ArgList {
    pub(super) args: Vec<Expr>,
    pub(super) keywords: Vec<(Expr, Expr)>,
    pub(super) starargs: Option<Box<Expr>>,
    pub(super) kwargs: Option<Box<Expr>>,
}

/// What may follow a `testlist` (the exit set of Jython's DFA for the
/// tuple loop in `testlist`).
fn follows_testlist(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        AmperEqual
            | Equal
            | CircumflexEqual
            | Colon
            | DoubleSlashEqual
            | DoubleStarEqual
            | For
            | If
            | LeftShiftEqual
            | MinusEqual
            | Newline
            | PercentEqual
            | PlusEqual
            | Rsqb
            | RightShiftEqual
            | Rpar
            | Semi
            | SlashEqual
            | StarEqual
            | VbarEqual
    )
}

/// Second-token set of the `exprlist` loop.
fn starts_expr_loosely(kind: TokenKind) -> bool {
    !matches!(kind, TokenKind::Not | TokenKind::Lambda) && starts_test_loosely(kind)
}

/// Second-token set of the `subscriptlist` loop.
fn starts_subscript(kind: TokenKind) -> bool {
    matches!(kind, TokenKind::Colon | TokenKind::Dot) || starts_test_loosely(kind)
}
