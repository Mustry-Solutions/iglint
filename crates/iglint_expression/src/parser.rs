//! Precedence-climbing parser equivalent to Ignition's LALR grammar.

use std::fmt;

use iglint_text::{TextRange, TextSize};

use crate::ast::{Binary, BinaryOp, BoundRef, Call, Expr, ExprKind, Subscript, Unary, UnaryOp};
use crate::lexer::{DroppedTail, LexErrorKind, Token, TokenKind, TokenValue, lex};

/// A syntax or lexical error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub range: TextRange,
    pub kind: ParseErrorKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseErrorKind {
    Lex(LexErrorKind),
    /// A token that cannot appear here. `expected` describes what could.
    UnexpectedToken {
        found: TokenKind,
        expected: &'static str,
    },
    /// An identifier not followed by `(`: Ignition has no variables.
    BareIdentifier,
    /// Recursion limit reached (deeply nested input).
    TooDeep,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::Lex(k) => k.fmt(f),
            ParseErrorKind::UnexpectedToken { found, expected } => {
                write!(f, "unexpected {found}, expected {expected}")
            }
            ParseErrorKind::BareIdentifier => {
                f.write_str("identifier must be a function call; references need braces, like `{view.params.x}`")
            }
            ParseErrorKind::TooDeep => f.write_str("expression nested too deeply"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.range)
    }
}

impl std::error::Error for ParseError {}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The tree. With errors it contains [`ExprKind::Error`] nodes.
    pub expr: Expr,
    /// Errors in source order. Ignition rejects the expression when this is
    /// not empty; it reports only the first one.
    pub errors: Vec<ParseError>,
    /// `//` comment ranges.
    pub comments: Vec<TextRange>,
    /// An unterminated string or reference at the very end, which Ignition
    /// discards without an error.
    pub dropped_tail: Option<DroppedTail>,
}

impl Parsed {
    /// True when Ignition accepts the expression.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Maximum depth of the syntax tree. Deeper input is reported as
/// [`ParseErrorKind::TooDeep`] and its tree is replaced by an error node, so
/// that recursive consumers (printers, visitors, `Drop`) stay within a small
/// stack. Ignition has no such limit, but no real expression comes close.
pub const MAX_DEPTH: usize = 1000;
/// Limit on parser recursion (each tree level costs at most 13 calls).
const RECURSION_LIMIT: usize = 13 * MAX_DEPTH;
/// Recursion levels parsed on the caller's stack.
const INLINE_RECURSION: usize = 48;
/// Stack reserved per recursion level on the helper thread. Generous so
/// unoptimised builds fit too; only touched pages are committed.
const STACK_PER_LEVEL: usize = 16 * 1024;

/// Parses an Ignition expression.
///
/// Deeply nested input is parsed on a helper thread with a stack sized for
/// it, so nesting never overflows the caller's stack.
pub fn parse(source: &str) -> Parsed {
    let lexed = lex(source);
    let bound = recursion_bound(&lexed.tokens);
    if bound <= INLINE_RECURSION {
        return parse_lexed(lexed);
    }
    let stack = (1 << 20) + bound.min(RECURSION_LIMIT) * STACK_PER_LEVEL;
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("iglint-expression-parser".to_owned())
            .stack_size(stack)
            .spawn_scoped(scope, move || parse_lexed(lexed));
        match handle {
            Ok(h) => h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)),
            Err(_) => {
                // No thread available: refuse rather than risk the stack.
                let mut parsed = parse_lexed(lex(""));
                parsed.errors = vec![ParseError {
                    range: TextRange::new(TextSize::ZERO, TextSize::of(source)),
                    kind: ParseErrorKind::TooDeep,
                }];
                parsed
            }
        }
    })
}

/// Upper bound on parser recursion for `tokens`: each bracket level allows
/// at most 13 nested `expr` calls (one per binary precedence level plus the
/// bracket itself); each prefix operator adds one. Left-associative chains
/// are parsed iteratively and do not count.
fn recursion_bound(tokens: &[Token]) -> usize {
    let mut nesting = 0usize;
    let mut max_nesting = 0usize;
    let mut unary = 0usize;
    for t in tokens {
        match t.kind {
            TokenKind::LParen | TokenKind::LBracket => {
                nesting += 1;
                max_nesting = max_nesting.max(nesting);
            }
            TokenKind::RParen | TokenKind::RBracket => nesting = nesting.saturating_sub(1),
            TokenKind::Minus | TokenKind::Bang | TokenKind::Tilde => unary += 1,
            _ => {}
        }
    }
    (max_nesting + 1) * 13 + unary
}

/// Depth of the tree, computed without recursion.
fn tree_depth(expr: &Expr) -> usize {
    let mut max = 0;
    let mut stack = vec![(expr, 1usize)];
    while let Some((e, d)) = stack.pop() {
        max = max.max(d);
        for c in e.children() {
            stack.push((c, d + 1));
        }
    }
    max
}

/// Drops a tree without recursion.
fn dismantle(expr: Expr) {
    let mut stack = vec![expr];
    while let Some(e) = stack.pop() {
        match e.kind {
            ExprKind::Unary(u) => stack.push(u.operand),
            ExprKind::Binary(b) => {
                let b = *b;
                stack.push(b.left);
                stack.push(b.right);
            }
            ExprKind::Call(c) => stack.extend(c.args),
            ExprKind::Subscript(s) => {
                let s = *s;
                stack.push(s.target);
                stack.push(s.index);
                stack.extend(s.column);
            }
            ExprKind::Paren(inner) => stack.push(*inner),
            ExprKind::Error(items) => stack.extend(items),
            _ => {}
        }
    }
}

fn parse_lexed(lexed: crate::lexer::Lexed) -> Parsed {
    // (anchor, priority, error): lexical errors surface when the parser
    // requests the token they occur in, before a syntax error at that token.
    let mut ordered: Vec<(TextSize, u8, ParseError)> = lexed
        .errors
        .iter()
        .map(|e| {
            let err = ParseError {
                range: e.range,
                kind: ParseErrorKind::Lex(e.kind.clone()),
            };
            (e.token_start, 0, err)
        })
        .collect();
    let mut p = Parser {
        tokens: lexed.tokens,
        pos: 0,
        errors: Vec::new(),
        depth: 0,
        max_depth: RECURSION_LIMIT,
    };
    let mut expr = p.expr(0);
    if p.peek() != TokenKind::Eof {
        let mut items = vec![expr];
        while p.peek() != TokenKind::Eof {
            p.unexpected("an operator or end of expression");
            if starts_expr(p.peek()) {
                items.push(p.expr(0));
            } else {
                p.bump();
            }
        }
        let range = items[0].range.cover(items[items.len() - 1].range);
        expr = Expr {
            range,
            kind: ExprKind::Error(items),
        };
    }
    if p.errors.iter().all(|e| e.kind != ParseErrorKind::TooDeep) && tree_depth(&expr) > MAX_DEPTH {
        p.errors.push(ParseError {
            range: TextRange::empty(expr.range.start()),
            kind: ParseErrorKind::TooDeep,
        });
    }
    if p.errors.iter().any(|e| e.kind == ParseErrorKind::TooDeep) {
        let range = expr.range;
        dismantle(expr);
        expr = Expr {
            range,
            kind: ExprKind::Error(Vec::new()),
        };
    }
    ordered.extend(p.errors.into_iter().map(|e| (e.range.start(), 1, e)));
    ordered.sort_by_key(|(anchor, prio, e)| (*anchor, *prio, e.range.start()));
    let mut errors: Vec<ParseError> = Vec::with_capacity(ordered.len());
    for (_, _, e) in ordered {
        if !errors.iter().any(|x| x.range.start() == e.range.start()) {
            errors.push(e);
        }
    }
    Parsed {
        expr,
        errors,
        comments: lexed.comments,
        dropped_tail: lexed.dropped_tail,
    }
}

fn starts_expr(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LParen
            | TokenKind::Minus
            | TokenKind::Bang
            | TokenKind::Tilde
            | TokenKind::Bool
            | TokenKind::Null
            | TokenKind::Ident
            | TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::Reference
    )
}

fn binary_op(kind: TokenKind) -> Option<BinaryOp> {
    Some(match kind {
        TokenKind::PipePipe => BinaryOp::Or,
        TokenKind::AmpAmp => BinaryOp::And,
        TokenKind::Xor => BinaryOp::BitXor,
        TokenKind::Pipe => BinaryOp::BitOr,
        TokenKind::Amp => BinaryOp::BitAnd,
        TokenKind::Eq => BinaryOp::Eq,
        TokenKind::NotEq => BinaryOp::NotEq,
        TokenKind::Like => BinaryOp::Like,
        TokenKind::Lt => BinaryOp::Lt,
        TokenKind::LtEq => BinaryOp::LtEq,
        TokenKind::Gt => BinaryOp::Gt,
        TokenKind::GtEq => BinaryOp::GtEq,
        TokenKind::Shl => BinaryOp::Shl,
        TokenKind::Shr => BinaryOp::Shr,
        TokenKind::Plus => BinaryOp::Add,
        TokenKind::Minus => BinaryOp::Sub,
        TokenKind::Star => BinaryOp::Mul,
        TokenKind::Slash => BinaryOp::Div,
        TokenKind::Percent => BinaryOp::Mod,
        TokenKind::Caret => BinaryOp::Pow,
        _ => return None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    depth: usize,
    max_depth: usize,
}

impl Parser {
    fn peek(&self) -> TokenKind {
        self.tokens[self.pos].kind
    }

    fn token(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if t.kind != TokenKind::Eof {
            self.pos += 1;
        }
        t
    }

    fn unexpected(&mut self, expected: &'static str) {
        let t = self.token();
        let err = ParseError {
            range: t.range,
            kind: ParseErrorKind::UnexpectedToken {
                found: t.kind,
                expected,
            },
        };
        self.push_error(err);
    }

    fn push_error(&mut self, err: ParseError) {
        if !self
            .errors
            .iter()
            .any(|e| e.range.start() == err.range.start())
        {
            self.errors.push(err);
        }
    }

    fn error_node(&self) -> Expr {
        Expr {
            range: TextRange::empty(self.token().range.start()),
            kind: ExprKind::Error(Vec::new()),
        }
    }

    /// Binary operators with precedence `>= min`.
    fn expr(&mut self, min: u8) -> Expr {
        self.depth += 1;
        if self.depth > self.max_depth {
            let range = self.token().range;
            self.push_error(ParseError {
                range,
                kind: ParseErrorKind::TooDeep,
            });
            // Skip to the end so recursion unwinds without more errors.
            self.pos = self.tokens.len() - 1;
            self.depth -= 1;
            return self.error_node();
        }
        let mut left = self.unary();
        while let Some(op) = binary_op(self.peek()) {
            let prec = op.precedence();
            if prec < min {
                break;
            }
            let op_range = self.bump().range;
            let right = self.expr(prec + 1);
            let range = left.range.cover(right.range).cover(op_range);
            left = Expr {
                range,
                kind: ExprKind::Binary(Box::new(Binary {
                    op,
                    op_range,
                    left,
                    right,
                })),
            };
        }
        self.depth -= 1;
        left
    }

    fn unary(&mut self) -> Expr {
        let op = match self.peek() {
            TokenKind::Minus => UnaryOp::Neg,
            TokenKind::Bang => UnaryOp::Not,
            TokenKind::Tilde => UnaryOp::BitNot,
            _ => return self.postfix(),
        };
        self.depth += 1;
        if self.depth > self.max_depth {
            let range = self.token().range;
            self.push_error(ParseError {
                range,
                kind: ParseErrorKind::TooDeep,
            });
            self.pos = self.tokens.len() - 1;
            self.depth -= 1;
            return self.error_node();
        }
        let op_range = self.bump().range;
        let operand = self.unary();
        self.depth -= 1;
        Expr {
            range: op_range.cover(operand.range),
            kind: ExprKind::Unary(Box::new(Unary {
                op,
                op_range,
                operand,
            })),
        }
    }

    fn postfix(&mut self) -> Expr {
        let mut target = self.primary();
        while self.peek() == TokenKind::LBracket {
            self.bump();
            let index = self.expr(0);
            let column = if self.peek() == TokenKind::Comma {
                self.bump();
                Some(self.expr(0))
            } else {
                None
            };
            let mut end = column.as_ref().unwrap_or(&index).range.end();
            if self.peek() == TokenKind::RBracket {
                end = self.bump().range.end();
            } else {
                self.unexpected(if column.is_some() {
                    "`]`"
                } else {
                    "`,` or `]`"
                });
            }
            target = Expr {
                range: TextRange::new(target.range.start(), end.max(target.range.end())),
                kind: ExprKind::Subscript(Box::new(Subscript {
                    target,
                    index,
                    column,
                })),
            };
        }
        target
    }

    fn primary(&mut self) -> Expr {
        let t = self.token().clone();
        let kind = match t.kind {
            TokenKind::Integer => match t.value {
                TokenValue::Integer(v) => ExprKind::Integer(v),
                _ => ExprKind::Integer(0),
            },
            TokenKind::Float => match t.value {
                TokenValue::Float(v) => ExprKind::Float(v),
                _ => ExprKind::Float(0.0),
            },
            TokenKind::String => match t.value {
                TokenValue::Text(s) => ExprKind::String(s),
                _ => ExprKind::String(String::new()),
            },
            TokenKind::Bool => ExprKind::Bool(t.value == TokenValue::Bool(true)),
            TokenKind::Null => ExprKind::Null,
            TokenKind::Reference => {
                let path = match t.value {
                    TokenValue::Text(s) => s,
                    _ => String::new(),
                };
                let start = t.range.start() + TextSize::new(1);
                let path_range = TextRange::at(start, TextSize::of(path.as_str()));
                ExprKind::Reference(BoundRef { path, path_range })
            }
            TokenKind::LParen => return self.paren(),
            TokenKind::Ident => return self.call(),
            _ => {
                self.unexpected("an expression");
                return self.error_node();
            }
        };
        self.bump();
        Expr {
            range: t.range,
            kind,
        }
    }

    fn paren(&mut self) -> Expr {
        let open = self.bump().range;
        let inner = self.expr(0);
        let end = if self.peek() == TokenKind::RParen {
            self.bump().range.end()
        } else {
            self.unexpected("`)`");
            inner.range.end()
        };
        Expr {
            range: TextRange::new(open.start(), end.max(open.end())),
            kind: ExprKind::Paren(Box::new(inner)),
        }
    }

    fn call(&mut self) -> Expr {
        let name_tok = self.bump();
        let name = match &name_tok.value {
            TokenValue::Text(s) => s.clone(),
            _ => String::new(),
        };
        if self.peek() != TokenKind::LParen {
            let range = self.token().range;
            self.push_error(ParseError {
                range,
                kind: ParseErrorKind::BareIdentifier,
            });
            return Expr {
                range: name_tok.range,
                kind: ExprKind::Error(Vec::new()),
            };
        }
        self.bump();
        let mut args = Vec::new();
        let mut end = None;
        if self.peek() == TokenKind::RParen {
            end = Some(self.bump().range.end());
        } else {
            loop {
                args.push(self.expr(0));
                match self.peek() {
                    TokenKind::Comma => {
                        self.bump();
                    }
                    TokenKind::RParen => {
                        end = Some(self.bump().range.end());
                        break;
                    }
                    k => {
                        self.unexpected("`,` or `)`");
                        if !starts_expr(k) {
                            break;
                        }
                    }
                }
            }
        }
        let end = end.unwrap_or_else(|| {
            args.last()
                .map_or(name_tok.range.end(), |a| a.range.end())
                .max(name_tok.range.end())
        });
        Expr {
            range: TextRange::new(name_tok.range.start(), end),
            kind: ExprKind::Call(Call {
                name,
                name_range: name_tok.range,
                args,
            }),
        }
    }
}
