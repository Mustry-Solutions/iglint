//! Statements, function parameters and the module level.

use iglint_text::{Ranged, TextRange};

use super::expression::{Operand, starts_test_loosely};
use super::target::{apply_context, check_assign, check_aug_assign, check_delete};
use super::{Failure, PResult, Parser};
use crate::anchor::Anchor;
use crate::ast::{
    Alias, Arguments, ExceptHandler, Expr, ExprAttribute, ExprCall, ExprContext, ExprName,
    ExprTuple, Identifier, Operator, Stmt, StmtAssert, StmtAssign, StmtAugAssign, StmtBreak,
    StmtClassDef, StmtContinue, StmtDelete, StmtExec, StmtExpr, StmtFor, StmtFunctionDef,
    StmtGlobal, StmtIf, StmtImport, StmtImportFrom, StmtPass, StmtPrint, StmtRaise, StmtReturn,
    StmtTryExcept, StmtTryFinally, StmtWhile, StmtWith,
};
use crate::error::ParseError;
use crate::token::{Token, TokenKind};

/// Tokens that start a simple statement (Jython's prediction set).
fn starts_simple_statement(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(
        kind,
        Backquote
            | Lsqb
            | Lbrace
            | Lpar
            | Minus
            | Name
            | Not
            | Plus
            | Tilde
            | Assert
            | Break
            | Complex
            | Continue
            | Del
            | Exec
            | Float
            | From
            | Global
            | Import
            | Int
            | Lambda
            | LongInt
            | Pass
            | Print
            | Raise
            | Return
            | String
            | Yield
    )
}

fn starts_compound_statement(kind: TokenKind) -> bool {
    use TokenKind::*;
    matches!(kind, At | Class | Def | For | If | Try | While | With)
}

fn augmented_operator(kind: TokenKind) -> Option<Operator> {
    use TokenKind::*;
    Some(match kind {
        PlusEqual => Operator::Add,
        MinusEqual => Operator::Sub,
        StarEqual => Operator::Mult,
        SlashEqual => Operator::Div,
        PercentEqual => Operator::Mod,
        AmperEqual => Operator::BitAnd,
        VbarEqual => Operator::BitOr,
        CircumflexEqual => Operator::BitXor,
        LeftShiftEqual => Operator::LShift,
        RightShiftEqual => Operator::RShift,
        DoubleStarEqual => Operator::Pow,
        DoubleSlashEqual => Operator::FloorDiv,
        _ => return None,
    })
}

impl Parser<'_> {
    // -- Module and blocks --------------------------------------------------

    /// `file_input: (NEWLINE | stmt)* ENDMARKER`
    pub(super) fn file_input(&mut self) -> Vec<Stmt> {
        let mut body = Vec::new();
        loop {
            match self.kind() {
                TokenKind::EndOfFile => break,
                TokenKind::Newline => {
                    self.bump();
                }
                TokenKind::Indent => {
                    let _ = self.invalid_syntax();
                    self.bump();
                    self.statements(&mut body, true);
                    self.eat(TokenKind::Dedent);
                }
                TokenKind::Dedent => {
                    let _ = self.invalid_syntax();
                    self.bump();
                }
                _ => self.statement_with_recovery(&mut body),
            }
        }
        body
    }

    /// `eval_input: LEADING_WS? NEWLINE* testlist NEWLINE* ENDMARKER`
    pub(super) fn eval_input(&mut self) -> Option<Expr> {
        self.eat(TokenKind::LeadingWs);
        while self.eat(TokenKind::Newline).is_some() {}
        let expr = self.testlist().ok().map(|operand| operand.expr);
        while self.eat(TokenKind::Newline).is_some() {}
        if !self.at(TokenKind::EndOfFile) {
            let _ = self.invalid_syntax();
        }
        expr
    }

    /// Statements of an indented block until its `DEDENT` (not consumed).
    fn statements(&mut self, body: &mut Vec<Stmt>, nested: bool) {
        loop {
            match self.kind() {
                TokenKind::EndOfFile => return,
                TokenKind::Dedent if nested => return,
                TokenKind::Indent => {
                    // An unexpected indent: report it and keep the nested
                    // statements.
                    let _ = self.invalid_syntax();
                    self.bump();
                    self.statements(body, true);
                    self.eat(TokenKind::Dedent);
                }
                TokenKind::Newline | TokenKind::Dedent => {
                    let _ = self.invalid_syntax();
                    self.bump();
                }
                _ => self.statement_with_recovery(body),
            }
        }
    }

    /// Parses one statement; on a syntax error skips to the end of the line.
    fn statement_with_recovery(&mut self, body: &mut Vec<Stmt>) {
        let before = self.stream.position;
        if self.statement(body).is_err() {
            self.recover_to_line_end();
            if self.stream.position == before && !self.at(TokenKind::EndOfFile) {
                self.bump();
            }
            // A block that belonged to a statement with a broken header:
            // keep its statements.
            if self.at(TokenKind::Indent) {
                self.bump();
                self.statements(body, true);
                self.eat(TokenKind::Dedent);
            }
        }
    }

    /// `stmt: simple_stmt | compound_stmt`
    fn statement(&mut self, body: &mut Vec<Stmt>) -> PResult<()> {
        let kind = self.kind();
        if starts_compound_statement(kind) {
            let statement = self.compound_statement()?;
            body.push(statement);
            Ok(())
        } else if starts_simple_statement(kind) {
            self.simple_statement(body)
        } else {
            Err(self.invalid_syntax())
        }
    }

    /// `simple_stmt: small_stmt (';' small_stmt)* [';'] NEWLINE`
    fn simple_statement(&mut self, body: &mut Vec<Stmt>) -> PResult<()> {
        loop {
            let statement = self.small_statement()?;
            body.push(statement);
            if self.at(TokenKind::Semi) && starts_simple_statement(self.peek_kind(2)) {
                self.bump();
                continue;
            }
            break;
        }
        self.eat(TokenKind::Semi);
        self.expect(TokenKind::Newline)?;
        Ok(())
    }

    /// `suite: simple_stmt | NEWLINE INDENT stmt+ DEDENT`. Recovers from
    /// errors inside the suite itself.
    fn suite(&mut self, from_finally: bool) -> PResult<Vec<Stmt>> {
        let mut body = Vec::new();
        let kind = self.kind();
        if starts_simple_statement(kind) {
            self.suites.push(from_finally);
            if self.simple_statement(&mut body).is_err() {
                self.recover_to_line_end();
            }
            self.suites.pop();
            return Ok(body);
        }
        if kind != TokenKind::Newline {
            return Err(self.invalid_syntax());
        }
        self.bump();
        if !self.at(TokenKind::Indent) {
            // "expected an indented block"
            let _ = self.expected("an indented block");
            return Ok(body);
        }
        self.bump();
        self.suites.push(from_finally);
        self.statements(&mut body, true);
        self.suites.pop();
        if self.at(TokenKind::Dedent) {
            self.bump();
        }
        Ok(body)
    }

    /// Whether `continue` is illegal in the current suite.
    fn continue_illegal(&self) -> bool {
        self.suites.last().copied().unwrap_or(false)
    }

    // -- Simple statements --------------------------------------------------

    fn small_statement(&mut self) -> PResult<Stmt> {
        let token = self.lt1();
        match token.kind {
            TokenKind::Print if !self.print_function => self.print_statement(),
            TokenKind::Del => self.del_statement(),
            TokenKind::Pass => {
                self.bump();
                Ok(Stmt::Pass(StmtPass {
                    range: token.range,
                    anchor: token.anchor,
                }))
            }
            TokenKind::Break => {
                self.bump();
                Ok(Stmt::Break(StmtBreak {
                    range: token.range,
                    anchor: token.anchor,
                }))
            }
            TokenKind::Continue => {
                self.bump();
                if self.continue_illegal() {
                    self.report(ParseError::syntax(
                        "'continue' not supported inside 'finally' clause",
                        token.range,
                        token.anchor,
                    ));
                }
                Ok(Stmt::Continue(StmtContinue {
                    range: token.range,
                    anchor: token.anchor,
                }))
            }
            TokenKind::Return => {
                self.bump();
                let value = match self.kind() {
                    TokenKind::Newline | TokenKind::Semi => None,
                    _ if self.starts_test() => Some(Box::new(self.testlist()?.expr)),
                    _ => return Err(self.invalid_syntax()),
                };
                Ok(Stmt::Return(StmtReturn {
                    range: self.range_from(token.range.start()),
                    anchor: token.anchor,
                    value,
                }))
            }
            TokenKind::Raise => self.raise_statement(),
            TokenKind::Yield => {
                let value = self.yield_expr()?;
                Ok(Stmt::Expr(StmtExpr {
                    range: value.range(),
                    anchor: token.anchor,
                    value: Box::new(value),
                }))
            }
            TokenKind::Import => self.import_name(),
            TokenKind::From => self.import_from(),
            TokenKind::Global => self.global_statement(),
            TokenKind::Exec => self.exec_statement(),
            TokenKind::Assert => {
                self.bump();
                let test = self.test()?.expr;
                let msg = if self.eat(TokenKind::Comma).is_some() {
                    Some(Box::new(self.test()?.expr))
                } else {
                    None
                };
                Ok(Stmt::Assert(StmtAssert {
                    range: self.range_from(token.range.start()),
                    anchor: token.anchor,
                    test: Box::new(test),
                    msg,
                }))
            }
            _ if self.starts_test() => self.expression_statement(),
            _ => Err(self.invalid_syntax()),
        }
    }

    /// `expr_stmt`: an expression, an assignment or an augmented assignment.
    fn expression_statement(&mut self) -> PResult<Stmt> {
        // Jython's predicates `(testlist augassign)=>` and
        // `(testlist ASSIGN)=>` scan the left-hand side and the operator.
        let (lhs, operator) = self.region(|parser| {
            let lhs = parser.testlist()?;
            let kind = parser.kind();
            let operator = if kind == TokenKind::Equal || augmented_operator(kind).is_some() {
                Some(parser.bump())
            } else {
                None
            };
            Ok((lhs, operator))
        })?;
        let Some(operator) = operator else {
            return Ok(Stmt::Expr(StmtExpr {
                range: self.range_from(lhs.start),
                anchor: lhs.first,
                value: Box::new(lhs.expr),
            }));
        };
        if let Some(op) = augmented_operator(operator.kind) {
            let value = if self.at(TokenKind::Yield) {
                self.yield_expr()?
            } else if self.starts_test() {
                self.testlist()?.expr
            } else {
                return Err(self.invalid_syntax());
            };
            let mut target = lhs.expr;
            apply_context(&mut target, ExprContext::AugStore);
            if let Some(error) = check_aug_assign(&target) {
                self.report(error);
            }
            return Ok(Stmt::AugAssign(StmtAugAssign {
                range: self.range_from(lhs.start),
                anchor: target.anchor(),
                target: Box::new(target),
                op,
                value: Box::new(value),
            }));
        }
        let yields = self.at(TokenKind::Yield);
        let lhs_node_anchor = lhs.expr.anchor();
        let mut parts = vec![lhs.expr];
        loop {
            let part = if yields {
                self.yield_expr()?
            } else {
                self.testlist()?.expr
            };
            parts.push(part);
            if self.eat(TokenKind::Equal).is_none() {
                break;
            }
        }
        let value = parts.pop().expect("an assignment has a value");
        let mut first_error = None;
        for target in &mut parts {
            apply_context(target, ExprContext::Store);
            if first_error.is_none() {
                first_error = check_assign(target);
            }
        }
        if let Some(error) = first_error {
            self.report(error);
        }
        let anchor = if yields { lhs.first } else { lhs_node_anchor };
        Ok(Stmt::Assign(StmtAssign {
            range: self.range_from(lhs.start),
            anchor,
            targets: parts,
            value: Box::new(value),
        }))
    }

    /// `print_stmt`
    fn print_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut dest = None;
        let mut values = Vec::new();
        let mut nl = true;
        match self.kind() {
            TokenKind::RightShift => {
                self.bump();
                // Syntactic predicate `(test COMMA test)=>`: without a
                // second expression the comma is left for the caller.
                let second = self.region(|parser| {
                    let dest = parser.test()?.expr;
                    let second = if parser.at(TokenKind::Comma) {
                        parser.speculate(|parser| {
                            parser.bump();
                            parser.test()
                        })
                    } else {
                        None
                    };
                    Ok((dest, second))
                })?;
                dest = Some(Box::new(second.0));
                if let Some(second) = second.1 {
                    values.push(second.expr);
                    nl = self.print_values(&mut values)?;
                }
            }
            TokenKind::Newline | TokenKind::Semi => {}
            _ if self.starts_test() => {
                // Syntactic predicate `(test COMMA)=>`.
                let (first, comma) = self.region(|parser| {
                    let first = parser.test()?.expr;
                    let comma = parser.eat(TokenKind::Comma).is_some();
                    Ok((first, comma))
                })?;
                values.push(first);
                if comma {
                    nl = false;
                    while starts_test_loosely(self.kind()) {
                        values.push(self.test()?.expr);
                        nl = true;
                        if self.eat(TokenKind::Comma).is_none() {
                            break;
                        }
                        nl = false;
                    }
                }
            }
            _ => return Err(self.invalid_syntax()),
        }
        Ok(Stmt::Print(StmtPrint {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            dest,
            values,
            nl,
        }))
    }

    /// `(',' test)* [',']` of a print statement; returns whether the
    /// statement prints a newline (no trailing comma).
    fn print_values(&mut self, values: &mut Vec<Expr>) -> PResult<bool> {
        while self.at(TokenKind::Comma) && starts_test_loosely(self.peek_kind(2)) {
            self.bump();
            values.push(self.test()?.expr);
        }
        Ok(self.eat(TokenKind::Comma).is_none())
    }

    /// `del_stmt: 'del' exprlist`
    fn del_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut targets = vec![self.expr()?.expr];
        while self.at(TokenKind::Comma)
            && starts_test_loosely(self.peek_kind(2))
            && !matches!(self.peek_kind(2), TokenKind::Not | TokenKind::Lambda)
        {
            self.bump();
            targets.push(self.expr()?.expr);
        }
        self.eat(TokenKind::Comma);
        let mut first_error = None;
        for target in &mut targets {
            apply_context(target, ExprContext::Del);
            if first_error.is_none() {
                first_error = check_delete(target);
            }
        }
        if let Some(error) = first_error {
            self.report(error);
        }
        Ok(Stmt::Delete(StmtDelete {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            targets,
        }))
    }

    /// `raise_stmt: 'raise' [test [',' test [',' test]]]`
    fn raise_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut parts: [Option<Box<Expr>>; 3] = [None, None, None];
        if self.starts_test() {
            parts[0] = Some(Box::new(self.test()?.expr));
            if self.eat(TokenKind::Comma).is_some() {
                parts[1] = Some(Box::new(self.test()?.expr));
                if self.eat(TokenKind::Comma).is_some() {
                    parts[2] = Some(Box::new(self.test()?.expr));
                }
            }
        }
        let [type_, inst, tback] = parts;
        Ok(Stmt::Raise(StmtRaise {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            type_,
            inst,
            tback,
        }))
    }

    /// `global_stmt: 'global' NAME (',' NAME)*`
    fn global_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut names = vec![self.name_identifier()?];
        while self.eat(TokenKind::Comma).is_some() {
            names.push(self.name_identifier()?);
        }
        Ok(Stmt::Global(StmtGlobal {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            names,
        }))
    }

    /// `exec_stmt: 'exec' expr ['in' test [',' test]]`
    fn exec_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let body = self.expr()?.expr;
        let mut globals = None;
        let mut locals = None;
        if self.eat(TokenKind::In).is_some() {
            globals = Some(Box::new(self.test()?.expr));
            if self.eat(TokenKind::Comma).is_some() {
                locals = Some(Box::new(self.test()?.expr));
            }
        }
        Ok(Stmt::Exec(StmtExec {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            body: Box::new(body),
            globals,
            locals,
        }))
    }

    /// A `NAME` token as an identifier.
    fn name_identifier(&mut self) -> PResult<Identifier> {
        let token = self.expect(TokenKind::Name)?;
        Ok(self.identifier(&token))
    }

    // -- Imports ------------------------------------------------------------

    /// `import_name: 'import' dotted_as_names`
    fn import_name(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut names = vec![self.dotted_as_name()?];
        while self.eat(TokenKind::Comma).is_some() {
            names.push(self.dotted_as_name()?);
        }
        Ok(Stmt::Import(StmtImport {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            names,
        }))
    }

    /// `dotted_as_name: dotted_name ['as' NAME]`
    fn dotted_as_name(&mut self) -> PResult<Alias> {
        let name = self.dotted_name()?;
        let asname = if self.eat(TokenKind::As).is_some() {
            Some(self.name_identifier()?)
        } else {
            None
        };
        Ok(Alias {
            range: self.range_from(name.range.start()),
            name,
            asname,
        })
    }

    /// `dotted_name: NAME ('.' attr)*`, joined with dots. Jython accepts
    /// keywords after the first dot.
    fn dotted_name(&mut self) -> PResult<Identifier> {
        let first = self.expect(TokenKind::Name)?;
        let mut id = self.text(&first).to_owned();
        while self.eat(TokenKind::Dot).is_some() {
            let part = self.attribute_name()?;
            id.push('.');
            id.push_str(&part.id);
        }
        Ok(Identifier {
            id: id.into(),
            range: self.range_from(first.range.start()),
        })
    }

    /// `import_from`
    fn import_from(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut level = 0;
        while self.eat(TokenKind::Dot).is_some() {
            level += 1;
        }
        let module = if level == 0 || self.at(TokenKind::Name) {
            Some(self.dotted_name()?)
        } else {
            None
        };
        self.expect(TokenKind::Import)?;
        let names = match self.kind() {
            TokenKind::Star => {
                let star = self.bump();
                vec![Alias {
                    range: star.range,
                    name: Identifier {
                        id: "*".into(),
                        range: star.range,
                    },
                    asname: None,
                }]
            }
            TokenKind::Lpar => {
                self.bump();
                let mut names = vec![self.import_as_name()?];
                while self.at(TokenKind::Comma) && self.peek_kind(2) == TokenKind::Name {
                    self.bump();
                    names.push(self.import_as_name()?);
                }
                self.eat(TokenKind::Comma);
                self.expect(TokenKind::Rpar)?;
                self.note_future(module.as_ref(), &names);
                names
            }
            TokenKind::Name => {
                let mut names = vec![self.import_as_name()?];
                while self.at(TokenKind::Comma) && self.peek_kind(2) == TokenKind::Name {
                    self.bump();
                    names.push(self.import_as_name()?);
                }
                self.note_future(module.as_ref(), &names);
                names
            }
            _ => return Err(self.invalid_syntax()),
        };
        Ok(Stmt::ImportFrom(StmtImportFrom {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            module,
            names,
            level,
        }))
    }

    /// `import_as_name: NAME ['as' NAME]`
    fn import_as_name(&mut self) -> PResult<Alias> {
        let name = self.name_identifier()?;
        let asname = if self.eat(TokenKind::As).is_some() {
            Some(self.name_identifier()?)
        } else {
            None
        };
        Ok(Alias {
            range: self.range_from(name.range.start()),
            name,
            asname,
        })
    }

    /// `from __future__ import ...` switches parser features immediately.
    /// Jython compares the source text of the module name, so
    /// `from . __future__` counts too.
    fn note_future(&mut self, module: Option<&Identifier>, names: &[Alias]) {
        let Some(module) = module else { return };
        if module.range.slice(self.source) != "__future__" {
            return;
        }
        for alias in names {
            match &*alias.name.id {
                "print_function" => self.print_function = true,
                "unicode_literals" => self.unicode_literals = true,
                _ => {}
            }
        }
    }

    // -- Compound statements ------------------------------------------------

    fn compound_statement(&mut self) -> PResult<Stmt> {
        match self.kind() {
            TokenKind::If => self.if_statement(),
            TokenKind::While => self.while_statement(),
            TokenKind::For => self.for_statement(),
            TokenKind::Try => self.try_statement(),
            TokenKind::With => self.with_statement(),
            TokenKind::At => {
                let start = self.lt1();
                // Syntactic predicate `(decorators? DEF)=>`.
                let (decorators, def) = self.region(|parser| {
                    let decorators = parser.decorators()?;
                    let def = parser.eat(TokenKind::Def);
                    Ok((decorators, def))
                })?;
                if def.is_some() {
                    self.function_definition(&start, decorators, true)
                } else if self.at(TokenKind::Class) {
                    self.class_definition(&start, decorators)
                } else {
                    Err(self.expected("'def' or 'class'"))
                }
            }
            TokenKind::Def => {
                let start = self.lt1();
                self.function_definition(&start, Vec::new(), false)
            }
            TokenKind::Class => {
                let start = self.lt1();
                self.class_definition(&start, Vec::new())
            }
            _ => Err(self.invalid_syntax()),
        }
    }

    /// The suite after `:` of a clause.
    fn clause_body(&mut self, from_finally: bool) -> PResult<Vec<Stmt>> {
        self.expect(TokenKind::Colon)?;
        self.suite(from_finally)
    }

    /// `if_stmt`; `elif` clauses become nested `If` nodes anchored at their
    /// condition.
    fn if_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let test = self.test()?.expr;
        let body = self.clause_body(false)?;
        let orelse = self.elif_or_else()?;
        Ok(Stmt::If(StmtIf {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            test: Box::new(test),
            body,
            orelse,
        }))
    }

    fn elif_or_else(&mut self) -> PResult<Vec<Stmt>> {
        match self.kind() {
            TokenKind::Else => {
                self.bump();
                self.clause_body(false)
            }
            TokenKind::Elif => {
                let elif = self.bump();
                let test = self.test()?;
                let body = self.clause_body(false)?;
                let orelse = self.elif_or_else()?;
                Ok(vec![Stmt::If(StmtIf {
                    range: self.range_from(elif.range.start()),
                    anchor: test.first,
                    test: Box::new(test.expr),
                    body,
                    orelse,
                })])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn while_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let test = self.test()?.expr;
        let body = self.clause_body(false)?;
        let orelse = if self.eat(TokenKind::Else).is_some() {
            self.clause_body(false)?
        } else {
            Vec::new()
        };
        Ok(Stmt::While(StmtWhile {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            test: Box::new(test),
            body,
            orelse,
        }))
    }

    fn for_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut target = self.exprlist()?;
        apply_context(&mut target, ExprContext::Store);
        self.expect(TokenKind::In)?;
        let iter = self.testlist()?.expr;
        let body = self.clause_body(false)?;
        let orelse = if self.eat(TokenKind::Else).is_some() {
            self.clause_body(false)?
        } else {
            Vec::new()
        };
        // Jython's `cantBeNone` looks at the text of the target's anchor
        // token.
        if self.anchor_text_is_none(target.anchor()) {
            self.report(ParseError::syntax(
                "can't be None",
                target.range(),
                target.anchor(),
            ));
        }
        Ok(Stmt::For(StmtFor {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            target: Box::new(target),
            iter: Box::new(iter),
            body,
            orelse,
        }))
    }

    /// Whether the token at `anchor` is the name `None`.
    fn anchor_text_is_none(&self, anchor: Anchor) -> bool {
        let Some(offset) = anchor.offset() else {
            return false;
        };
        let rest = self.source.get(offset.to_usize()..).unwrap_or("");
        rest.starts_with("None")
            && !rest[4..]
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    /// `try_stmt`
    fn try_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let inherited = self.continue_illegal();
        let body = self.clause_body(inherited)?;
        let start = token.range.start();
        match self.kind() {
            TokenKind::Except => {
                let mut handlers = Vec::new();
                while self.at(TokenKind::Except) {
                    handlers.push(self.except_clause(inherited)?);
                }
                let orelse = if self.eat(TokenKind::Else).is_some() {
                    self.clause_body(inherited)?
                } else {
                    Vec::new()
                };
                let try_except_end = self.previous_end;
                let finalbody = if self.eat(TokenKind::Finally).is_some() {
                    Some(self.clause_body(true)?)
                } else {
                    None
                };
                let try_except = Stmt::TryExcept(StmtTryExcept {
                    range: TextRange::new(start, try_except_end),
                    anchor: token.anchor,
                    body,
                    handlers,
                    orelse,
                });
                Ok(match finalbody {
                    Some(finalbody) => Stmt::TryFinally(StmtTryFinally {
                        range: self.range_from(start),
                        anchor: token.anchor,
                        body: vec![try_except],
                        finalbody,
                    }),
                    None => try_except,
                })
            }
            TokenKind::Finally => {
                self.bump();
                let finalbody = self.clause_body(true)?;
                Ok(Stmt::TryFinally(StmtTryFinally {
                    range: self.range_from(start),
                    anchor: token.anchor,
                    body,
                    finalbody,
                }))
            }
            _ => Err(self.invalid_syntax()),
        }
    }

    /// `except_clause: 'except' [test [('as' | ',') test]]` and its suite.
    fn except_clause(&mut self, continue_illegal: bool) -> PResult<ExceptHandler> {
        let token = self.bump();
        let mut type_ = None;
        let mut name = None;
        if self.starts_test() {
            type_ = Some(Box::new(self.test()?.expr));
            if matches!(self.kind(), TokenKind::Comma | TokenKind::As) {
                self.bump();
                let mut target = self.test()?.expr;
                apply_context(&mut target, ExprContext::Store);
                name = Some(Box::new(target));
            }
        }
        let body = self.clause_body(continue_illegal)?;
        Ok(ExceptHandler {
            range: self.range_from(token.range.start()),
            anchor: token.anchor,
            type_,
            name,
            body,
        })
    }

    /// `with_stmt: 'with' with_item (',' with_item)* ':' suite`. Several
    /// items nest; each `With` is anchored at its context expression.
    fn with_statement(&mut self) -> PResult<Stmt> {
        let token = self.bump();
        let mut items = vec![self.with_item()?];
        while self.eat(TokenKind::Comma).is_some() {
            items.push(self.with_item()?);
        }
        let mut body = self.clause_body(false)?;
        let end = self.previous_end;
        let mut statement = None;
        let count = items.len();
        for (index, (context, vars)) in items.into_iter().enumerate().rev() {
            if let Some(inner) = statement.take() {
                body = vec![inner];
            }
            let start = if index == 0 {
                token.range.start()
            } else {
                context.start
            };
            statement = Some(Stmt::With(StmtWith {
                range: TextRange::new(start, end),
                anchor: context.first,
                context_expr: Box::new(context.expr),
                optional_vars: vars.map(Box::new),
                body: std::mem::take(&mut body),
            }));
        }
        debug_assert!(count > 0);
        statement.ok_or(Failure)
    }

    /// `with_item: test ['as' expr]`
    fn with_item(&mut self) -> PResult<(Operand, Option<Expr>)> {
        let context = self.test()?;
        let vars = if self.eat(TokenKind::As).is_some() {
            let mut target = self.expr()?.expr;
            apply_context(&mut target, ExprContext::Store);
            if let Some(error) = check_assign(&target) {
                self.report(error);
            }
            Some(target)
        } else {
            None
        };
        Ok((context, vars))
    }

    // -- Definitions --------------------------------------------------------

    /// `decorators: decorator+`
    fn decorators(&mut self) -> PResult<Vec<Expr>> {
        let mut decorators = Vec::new();
        while self.at(TokenKind::At) {
            decorators.push(self.decorator()?);
        }
        Ok(decorators)
    }

    /// `decorator: '@' dotted_attr [ '(' [arglist] ')' ] NEWLINE`
    fn decorator(&mut self) -> PResult<Expr> {
        self.bump();
        let first = self.expect(TokenKind::Name)?;
        let start = first.range.start();
        let mut expr = self.name(&first);
        let mut attributes = Vec::new();
        if !matches!(
            self.kind(),
            TokenKind::Dot | TokenKind::Lpar | TokenKind::Newline
        ) {
            return Err(self.invalid_syntax());
        }
        while self.eat(TokenKind::Dot).is_some() {
            let attr = self.expect(TokenKind::Name)?;
            attributes.push(attr);
            expr = Expr::Attribute(ExprAttribute {
                range: self.range_from(start),
                anchor: attr.anchor,
                value: Box::new(expr),
                attr: self.identifier(&attr),
                ctx: ExprContext::Load,
            });
        }
        for attr in &attributes {
            if self.text(attr) == "None" {
                self.report(ParseError::syntax("can't be None", attr.range, attr.anchor));
            }
        }
        match self.kind() {
            TokenKind::Lpar => {
                let lpar = self.bump();
                let arguments = self.call_arguments()?;
                expr = Expr::Call(ExprCall {
                    range: self.range_from(start),
                    anchor: lpar.anchor,
                    func: Box::new(expr),
                    args: arguments.args,
                    keywords: arguments.keywords,
                    starargs: arguments.starargs,
                    kwargs: arguments.kwargs,
                });
            }
            TokenKind::Newline => {}
            _ => return Err(self.invalid_syntax()),
        }
        self.expect(TokenKind::Newline)?;
        Ok(expr)
    }

    /// `funcdef: [decorators] 'def' NAME parameters ':' suite`
    fn function_definition(
        &mut self,
        start: &Token,
        decorator_list: Vec<Expr>,
        def_consumed: bool,
    ) -> PResult<Stmt> {
        if !def_consumed {
            self.expect(TokenKind::Def)?;
        }
        let name_token = self.lt1();
        let name_ok = name_token.kind == TokenKind::Name
            || (name_token.kind == TokenKind::Print && self.print_function);
        if !name_ok {
            return Err(self.expected("function name"));
        }
        self.bump();
        let lpar = self.expect(TokenKind::Lpar)?;
        let args = match self.kind() {
            TokenKind::Rpar => Arguments {
                range: TextRange::empty(lpar.range.end()),
                ..Arguments::default()
            },
            TokenKind::Name | TokenKind::Lpar | TokenKind::Star | TokenKind::DoubleStar => {
                self.varargslist()?
            }
            _ => return Err(self.invalid_syntax()),
        };
        self.expect(TokenKind::Rpar)?;
        let body = self.clause_body(false)?;
        if self.text(&name_token) == "None" {
            self.report(ParseError::syntax(
                "can't be None",
                name_token.range,
                name_token.anchor,
            ));
        }
        Ok(Stmt::FunctionDef(StmtFunctionDef {
            range: self.range_from(start.range.start()),
            anchor: start.anchor,
            name: self.identifier(&name_token),
            args: Box::new(args),
            body,
            decorator_list,
        }))
    }

    /// `classdef: [decorators] 'class' NAME ['(' [testlist] ')'] ':' suite`
    fn class_definition(&mut self, start: &Token, decorator_list: Vec<Expr>) -> PResult<Stmt> {
        self.expect(TokenKind::Class)?;
        let name_token = self.expect(TokenKind::Name)?;
        let mut bases = Vec::new();
        if self.eat(TokenKind::Lpar).is_some() {
            if self.starts_test() {
                // Jython unpacks any tuple, even a parenthesized one.
                match self.testlist()?.expr {
                    Expr::Tuple(tuple) => bases = tuple.elts,
                    base => bases.push(base),
                }
            }
            self.expect(TokenKind::Rpar)?;
        }
        let body = self.clause_body(false)?;
        if self.text(&name_token) == "None" {
            self.report(ParseError::syntax(
                "can't be None",
                name_token.range,
                name_token.anchor,
            ));
        }
        Ok(Stmt::ClassDef(StmtClassDef {
            range: self.range_from(start.range.start()),
            anchor: start.anchor,
            name: self.identifier(&name_token),
            bases,
            body,
            decorator_list,
        }))
    }

    /// `varargslist` (parameters of `def` and `lambda`).
    pub(super) fn varargslist(&mut self) -> PResult<Arguments> {
        let start = self.lt1().range.start();
        let mut arguments = Arguments::default();
        match self.kind() {
            TokenKind::Star => {
                self.star_parameters(&mut arguments)?;
            }
            TokenKind::DoubleStar => {
                self.bump();
                arguments.kwarg = Some(self.parameter_name()?);
            }
            _ => {
                self.default_parameter(&mut arguments)?;
                while self.at(TokenKind::Comma)
                    && matches!(self.peek_kind(2), TokenKind::Name | TokenKind::Lpar)
                {
                    self.bump();
                    self.default_parameter(&mut arguments)?;
                }
                if self.eat(TokenKind::Comma).is_some() {
                    match self.kind() {
                        TokenKind::Star => self.star_parameters(&mut arguments)?,
                        TokenKind::DoubleStar => {
                            self.bump();
                            arguments.kwarg = Some(self.parameter_name()?);
                        }
                        _ => {}
                    }
                }
            }
        }
        arguments.range = self.range_from(start);
        // Jython checks the names of `*args` and `**kwargs` when it builds
        // the arguments, after the whole list.
        for name in [&arguments.vararg, &arguments.kwarg].into_iter().flatten() {
            if &*name.id == "None" {
                let error =
                    ParseError::syntax("can't be None", name.range, Anchor::at(name.range.start()));
                self.report(error);
            }
        }
        Ok(arguments)
    }

    /// `'*' NAME [',' '**' NAME]`
    fn star_parameters(&mut self, arguments: &mut Arguments) -> PResult<()> {
        self.bump();
        arguments.vararg = Some(self.parameter_name()?);
        if self.eat(TokenKind::Comma).is_some() {
            self.expect(TokenKind::DoubleStar)?;
            arguments.kwarg = Some(self.parameter_name()?);
        }
        Ok(())
    }

    /// The name after `*` or `**`.
    fn parameter_name(&mut self) -> PResult<Identifier> {
        let token = self.expect(TokenKind::Name)?;
        Ok(self.identifier(&token))
    }

    /// `defparameter: fpdef ['=' test]`
    fn default_parameter(&mut self, arguments: &mut Arguments) -> PResult<()> {
        let parameter = self.fpdef(ExprContext::Param)?;
        if self.eat(TokenKind::Equal).is_some() {
            arguments.defaults.push(self.test()?.expr);
        } else if !arguments.defaults.is_empty() {
            self.report(ParseError::syntax(
                "non-default argument follows default argument",
                parameter.range(),
                parameter.anchor(),
            ));
        }
        arguments.args.push(parameter);
        Ok(())
    }

    /// `fpdef: NAME | '(' fplist ')'`
    fn fpdef(&mut self, ctx: ExprContext) -> PResult<Expr> {
        let token = self.lt1();
        let parameter = match token.kind {
            TokenKind::Name => {
                self.bump();
                let parameter = Expr::Name(ExprName {
                    range: token.range,
                    anchor: token.anchor,
                    id: self.text(&token).into(),
                    ctx,
                });
                // Tuples need no check of their own: their elements have
                // been checked already.
                if let Some(error) = check_assign(&parameter) {
                    self.report(error);
                }
                parameter
            }
            TokenKind::Lpar => {
                // Syntactic predicate `(LPAREN fpdef COMMA)=>`.
                let (first_token, first, comma) = self.region(|parser| {
                    parser.bump();
                    let first_token = parser.lt1();
                    let first = parser.fpdef(ExprContext::Store)?;
                    let comma = parser.eat(TokenKind::Comma).is_some();
                    Ok((first_token, first, comma))
                })?;
                if comma {
                    let mut elts = vec![first];
                    while matches!(self.kind(), TokenKind::Name | TokenKind::Lpar) {
                        elts.push(self.fpdef(ExprContext::Store)?);
                        if self.eat(TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                    self.expect(TokenKind::Rpar)?;
                    Expr::Tuple(ExprTuple {
                        range: self.range_from(token.range.start()),
                        anchor: first_token.anchor,
                        elts,
                        ctx: ExprContext::Store,
                    })
                } else {
                    self.expect(TokenKind::Rpar)?;
                    first
                }
            }
            _ => return Err(self.invalid_syntax()),
        };
        Ok(parameter)
    }
}
