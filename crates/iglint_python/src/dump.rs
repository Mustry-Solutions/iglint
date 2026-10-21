//! A printer that reproduces Jython's `ast.dump(tree)` and
//! `ast.dump(tree, include_attributes=True)` byte for byte.
//!
//! Two Jython quirks are reproduced on purpose: a node without fields prints
//! its attributes after a space (`Pass( lineno=1, col_offset=0)`), and a
//! dictionary comprehension prints as the bare word `DictComp`, because
//! Jython's `DictComp` is not an `ast.AST` instance.

use crate::anchor::{Anchor, Locator};
use crate::ast::*;
use crate::repr::{number_repr, str_repr, string_repr};

/// `ast.dump(module)`.
pub fn dump(module: &Module) -> String {
    let mut printer = Printer {
        out: String::new(),
        locator: None,
    };
    printer.module(module);
    printer.out
}

/// `ast.dump(module, include_attributes=True)`; `source` is the text the
/// module was parsed from.
pub fn dump_with_attributes(module: &Module, source: &str) -> String {
    let locator = Locator::new(source);
    let mut printer = Printer {
        out: String::new(),
        locator: Some(&locator),
    };
    printer.module(module);
    printer.out
}

/// `ast.dump` of a single expression.
pub fn dump_expr(expr: &Expr) -> String {
    let mut printer = Printer {
        out: String::new(),
        locator: None,
    };
    printer.expr(expr);
    printer.out
}

struct Printer<'a> {
    out: String,
    locator: Option<&'a Locator<'a>>,
}

/// Starts a node; fields are written with [`Printer::field`].
struct Node {
    fields: usize,
}

impl Printer<'_> {
    fn open(&mut self, name: &str) -> Node {
        self.out.push_str(name);
        self.out.push('(');
        Node { fields: 0 }
    }

    fn field(&mut self, node: &mut Node, name: &str) {
        if node.fields > 0 {
            self.out.push_str(", ");
        }
        node.fields += 1;
        self.out.push_str(name);
        self.out.push('=');
    }

    /// Closes a node that has `lineno` / `col_offset` attributes.
    fn close_located(&mut self, node: Node, anchor: Anchor) {
        if let Some(locator) = self.locator {
            let location = locator.anchor_location(anchor);
            self.out.push_str(if node.fields > 0 { ", " } else { " " });
            self.out.push_str("lineno=");
            self.out.push_str(&location.line.to_string());
            self.out.push_str(", col_offset=");
            self.out.push_str(&location.column.to_string());
        }
        self.out.push(')');
    }

    fn close(&mut self, _node: Node) {
        self.out.push(')');
    }

    fn none(&mut self) {
        self.out.push_str("None");
    }

    fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut Self, &T)) {
        self.out.push('[');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.out.push_str(", ");
            }
            each(self, item);
        }
        self.out.push(']');
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        self.list(stmts, |printer, stmt| printer.stmt(stmt));
    }

    fn exprs(&mut self, exprs: &[Expr]) {
        self.list(exprs, |printer, expr| printer.expr(expr));
    }

    fn opt_expr(&mut self, expr: Option<&Expr>) {
        match expr {
            Some(expr) => self.expr(expr),
            None => self.none(),
        }
    }

    fn identifier(&mut self, identifier: &Identifier) {
        self.out.push_str(&str_repr(&identifier.id));
    }

    fn opt_identifier(&mut self, identifier: Option<&Identifier>) {
        match identifier {
            Some(identifier) => self.identifier(identifier),
            None => self.none(),
        }
    }

    fn simple(&mut self, name: &str) {
        self.out.push_str(name);
        self.out.push_str("()");
    }

    fn module(&mut self, module: &Module) {
        let mut node = self.open("Module");
        self.field(&mut node, "body");
        self.stmts(&module.body);
        self.close(node);
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::FunctionDef(s) => {
                let mut node = self.open("FunctionDef");
                self.field(&mut node, "name");
                self.identifier(&s.name);
                self.field(&mut node, "args");
                self.arguments(&s.args);
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "decorator_list");
                self.exprs(&s.decorator_list);
                self.close_located(node, s.anchor);
            }
            Stmt::ClassDef(s) => {
                let mut node = self.open("ClassDef");
                self.field(&mut node, "name");
                self.identifier(&s.name);
                self.field(&mut node, "bases");
                self.exprs(&s.bases);
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "decorator_list");
                self.exprs(&s.decorator_list);
                self.close_located(node, s.anchor);
            }
            Stmt::Return(s) => {
                let mut node = self.open("Return");
                self.field(&mut node, "value");
                self.opt_expr(s.value.as_deref());
                self.close_located(node, s.anchor);
            }
            Stmt::Delete(s) => {
                let mut node = self.open("Delete");
                self.field(&mut node, "targets");
                self.exprs(&s.targets);
                self.close_located(node, s.anchor);
            }
            Stmt::Assign(s) => {
                let mut node = self.open("Assign");
                self.field(&mut node, "targets");
                self.exprs(&s.targets);
                self.field(&mut node, "value");
                self.expr(&s.value);
                self.close_located(node, s.anchor);
            }
            Stmt::AugAssign(s) => {
                let mut node = self.open("AugAssign");
                self.field(&mut node, "target");
                self.expr(&s.target);
                self.field(&mut node, "op");
                self.simple(s.op.name());
                self.field(&mut node, "value");
                self.expr(&s.value);
                self.close_located(node, s.anchor);
            }
            Stmt::Print(s) => {
                let mut node = self.open("Print");
                self.field(&mut node, "dest");
                self.opt_expr(s.dest.as_deref());
                self.field(&mut node, "values");
                self.exprs(&s.values);
                self.field(&mut node, "nl");
                self.out.push_str(if s.nl { "True" } else { "False" });
                self.close_located(node, s.anchor);
            }
            Stmt::For(s) => {
                let mut node = self.open("For");
                self.field(&mut node, "target");
                self.expr(&s.target);
                self.field(&mut node, "iter");
                self.expr(&s.iter);
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "orelse");
                self.stmts(&s.orelse);
                self.close_located(node, s.anchor);
            }
            Stmt::While(s) => {
                let mut node = self.open("While");
                self.field(&mut node, "test");
                self.expr(&s.test);
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "orelse");
                self.stmts(&s.orelse);
                self.close_located(node, s.anchor);
            }
            Stmt::If(s) => {
                let mut node = self.open("If");
                self.field(&mut node, "test");
                self.expr(&s.test);
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "orelse");
                self.stmts(&s.orelse);
                self.close_located(node, s.anchor);
            }
            Stmt::With(s) => {
                let mut node = self.open("With");
                self.field(&mut node, "context_expr");
                self.expr(&s.context_expr);
                self.field(&mut node, "optional_vars");
                self.opt_expr(s.optional_vars.as_deref());
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.close_located(node, s.anchor);
            }
            Stmt::Raise(s) => {
                let mut node = self.open("Raise");
                self.field(&mut node, "type");
                self.opt_expr(s.type_.as_deref());
                self.field(&mut node, "inst");
                self.opt_expr(s.inst.as_deref());
                self.field(&mut node, "tback");
                self.opt_expr(s.tback.as_deref());
                self.close_located(node, s.anchor);
            }
            Stmt::TryExcept(s) => {
                let mut node = self.open("TryExcept");
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "handlers");
                self.list(&s.handlers, |printer, handler| printer.handler(handler));
                self.field(&mut node, "orelse");
                self.stmts(&s.orelse);
                self.close_located(node, s.anchor);
            }
            Stmt::TryFinally(s) => {
                let mut node = self.open("TryFinally");
                self.field(&mut node, "body");
                self.stmts(&s.body);
                self.field(&mut node, "finalbody");
                self.stmts(&s.finalbody);
                self.close_located(node, s.anchor);
            }
            Stmt::Assert(s) => {
                let mut node = self.open("Assert");
                self.field(&mut node, "test");
                self.expr(&s.test);
                self.field(&mut node, "msg");
                self.opt_expr(s.msg.as_deref());
                self.close_located(node, s.anchor);
            }
            Stmt::Import(s) => {
                let mut node = self.open("Import");
                self.field(&mut node, "names");
                self.list(&s.names, |printer, alias| printer.alias(alias));
                self.close_located(node, s.anchor);
            }
            Stmt::ImportFrom(s) => {
                let mut node = self.open("ImportFrom");
                self.field(&mut node, "module");
                match &s.module {
                    Some(module) => self.identifier(module),
                    None => self.out.push_str("''"),
                }
                self.field(&mut node, "names");
                self.list(&s.names, |printer, alias| printer.alias(alias));
                self.field(&mut node, "level");
                self.out.push_str(&s.level.to_string());
                self.close_located(node, s.anchor);
            }
            Stmt::Exec(s) => {
                let mut node = self.open("Exec");
                self.field(&mut node, "body");
                self.expr(&s.body);
                self.field(&mut node, "globals");
                self.opt_expr(s.globals.as_deref());
                self.field(&mut node, "locals");
                self.opt_expr(s.locals.as_deref());
                self.close_located(node, s.anchor);
            }
            Stmt::Global(s) => {
                let mut node = self.open("Global");
                self.field(&mut node, "names");
                self.list(&s.names, |printer, name| printer.identifier(name));
                self.close_located(node, s.anchor);
            }
            Stmt::Expr(s) => {
                let mut node = self.open("Expr");
                self.field(&mut node, "value");
                self.expr(&s.value);
                self.close_located(node, s.anchor);
            }
            Stmt::Pass(s) => {
                let node = self.open("Pass");
                self.close_located(node, s.anchor);
            }
            Stmt::Break(s) => {
                let node = self.open("Break");
                self.close_located(node, s.anchor);
            }
            Stmt::Continue(s) => {
                let node = self.open("Continue");
                self.close_located(node, s.anchor);
            }
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::BoolOp(e) => {
                let mut node = self.open("BoolOp");
                self.field(&mut node, "op");
                self.simple(e.op.name());
                self.field(&mut node, "values");
                self.exprs(&e.values);
                self.close_located(node, e.anchor);
            }
            Expr::BinOp(e) => {
                let mut node = self.open("BinOp");
                self.field(&mut node, "left");
                self.expr(&e.left);
                self.field(&mut node, "op");
                self.simple(e.op.name());
                self.field(&mut node, "right");
                self.expr(&e.right);
                self.close_located(node, e.anchor);
            }
            Expr::UnaryOp(e) => {
                let mut node = self.open("UnaryOp");
                self.field(&mut node, "op");
                self.simple(e.op.name());
                self.field(&mut node, "operand");
                self.expr(&e.operand);
                self.close_located(node, e.anchor);
            }
            Expr::Lambda(e) => {
                let mut node = self.open("Lambda");
                self.field(&mut node, "args");
                self.arguments(&e.args);
                self.field(&mut node, "body");
                self.expr(&e.body);
                self.close_located(node, e.anchor);
            }
            Expr::IfExp(e) => {
                let mut node = self.open("IfExp");
                self.field(&mut node, "test");
                self.expr(&e.test);
                self.field(&mut node, "body");
                self.expr(&e.body);
                self.field(&mut node, "orelse");
                self.expr(&e.orelse);
                self.close_located(node, e.anchor);
            }
            Expr::Dict(e) => {
                let mut node = self.open("Dict");
                self.field(&mut node, "keys");
                self.exprs(&e.keys);
                self.field(&mut node, "values");
                self.exprs(&e.values);
                self.close_located(node, e.anchor);
            }
            Expr::Set(e) => {
                let mut node = self.open("Set");
                self.field(&mut node, "elts");
                self.exprs(&e.elts);
                self.close_located(node, e.anchor);
            }
            Expr::ListComp(e) => {
                let mut node = self.open("ListComp");
                self.field(&mut node, "elt");
                self.expr(&e.elt);
                self.field(&mut node, "generators");
                self.comprehensions(&e.generators);
                self.close_located(node, e.anchor);
            }
            Expr::SetComp(e) => {
                let mut node = self.open("SetComp");
                self.field(&mut node, "elt");
                self.expr(&e.elt);
                self.field(&mut node, "generators");
                self.comprehensions(&e.generators);
                self.close_located(node, e.anchor);
            }
            // Jython's DictComp is not an `ast.AST`; `ast.dump` falls back to
            // its `repr()`.
            Expr::DictComp(_) => self.out.push_str("DictComp"),
            Expr::GeneratorExp(e) => {
                let mut node = self.open("GeneratorExp");
                self.field(&mut node, "elt");
                self.expr(&e.elt);
                self.field(&mut node, "generators");
                self.comprehensions(&e.generators);
                self.close_located(node, e.anchor);
            }
            Expr::Yield(e) => {
                let mut node = self.open("Yield");
                self.field(&mut node, "value");
                self.opt_expr(e.value.as_deref());
                self.close_located(node, e.anchor);
            }
            Expr::Compare(e) => {
                let mut node = self.open("Compare");
                self.field(&mut node, "left");
                self.expr(&e.left);
                self.field(&mut node, "ops");
                self.list(&e.ops, |printer, op| printer.simple(op.name()));
                self.field(&mut node, "comparators");
                self.exprs(&e.comparators);
                self.close_located(node, e.anchor);
            }
            Expr::Call(e) => {
                let mut node = self.open("Call");
                self.field(&mut node, "func");
                self.expr(&e.func);
                self.field(&mut node, "args");
                self.exprs(&e.args);
                self.field(&mut node, "keywords");
                self.list(&e.keywords, |printer, keyword| printer.keyword(keyword));
                self.field(&mut node, "starargs");
                self.opt_expr(e.starargs.as_deref());
                self.field(&mut node, "kwargs");
                self.opt_expr(e.kwargs.as_deref());
                self.close_located(node, e.anchor);
            }
            Expr::Repr(e) => {
                let mut node = self.open("Repr");
                self.field(&mut node, "value");
                self.expr(&e.value);
                self.close_located(node, e.anchor);
            }
            Expr::Num(e) => {
                let mut node = self.open("Num");
                self.field(&mut node, "n");
                self.out.push_str(&number_repr(&e.n));
                self.close_located(node, e.anchor);
            }
            Expr::Str(e) => {
                let mut node = self.open("Str");
                self.field(&mut node, "s");
                self.out.push_str(&string_repr(&e.s));
                self.close_located(node, e.anchor);
            }
            Expr::Attribute(e) => {
                let mut node = self.open("Attribute");
                self.field(&mut node, "value");
                self.expr(&e.value);
                self.field(&mut node, "attr");
                self.identifier(&e.attr);
                self.field(&mut node, "ctx");
                self.simple(e.ctx.name());
                self.close_located(node, e.anchor);
            }
            Expr::Subscript(e) => {
                let mut node = self.open("Subscript");
                self.field(&mut node, "value");
                self.expr(&e.value);
                self.field(&mut node, "slice");
                self.slice(&e.slice);
                self.field(&mut node, "ctx");
                self.simple(e.ctx.name());
                self.close_located(node, e.anchor);
            }
            Expr::Name(e) => {
                let mut node = self.open("Name");
                self.field(&mut node, "id");
                self.out.push_str(&str_repr(&e.id));
                self.field(&mut node, "ctx");
                self.simple(e.ctx.name());
                self.close_located(node, e.anchor);
            }
            Expr::List(e) => {
                let mut node = self.open("List");
                self.field(&mut node, "elts");
                self.exprs(&e.elts);
                self.field(&mut node, "ctx");
                self.simple(e.ctx.name());
                self.close_located(node, e.anchor);
            }
            Expr::Tuple(e) => {
                let mut node = self.open("Tuple");
                self.field(&mut node, "elts");
                self.exprs(&e.elts);
                self.field(&mut node, "ctx");
                self.simple(e.ctx.name());
                self.close_located(node, e.anchor);
            }
        }
    }

    fn slice(&mut self, slice: &Slice) {
        match slice {
            Slice::Ellipsis(_) => self.simple("Ellipsis"),
            Slice::Slice(s) => {
                let mut node = self.open("Slice");
                self.field(&mut node, "lower");
                self.opt_expr(s.lower.as_deref());
                self.field(&mut node, "upper");
                self.opt_expr(s.upper.as_deref());
                self.field(&mut node, "step");
                self.opt_expr(s.step.as_deref());
                self.close(node);
            }
            Slice::ExtSlice(s) => {
                let mut node = self.open("ExtSlice");
                self.field(&mut node, "dims");
                self.list(&s.dims, |printer, dim| printer.slice(dim));
                self.close(node);
            }
            Slice::Index(s) => {
                let mut node = self.open("Index");
                self.field(&mut node, "value");
                self.expr(&s.value);
                self.close(node);
            }
        }
    }

    fn handler(&mut self, handler: &ExceptHandler) {
        let mut node = self.open("ExceptHandler");
        self.field(&mut node, "type");
        self.opt_expr(handler.type_.as_deref());
        self.field(&mut node, "name");
        self.opt_expr(handler.name.as_deref());
        self.field(&mut node, "body");
        self.stmts(&handler.body);
        self.close_located(node, handler.anchor);
    }

    fn arguments(&mut self, arguments: &Arguments) {
        let mut node = self.open("arguments");
        self.field(&mut node, "args");
        self.exprs(&arguments.args);
        self.field(&mut node, "vararg");
        self.opt_identifier(arguments.vararg.as_ref());
        self.field(&mut node, "kwarg");
        self.opt_identifier(arguments.kwarg.as_ref());
        self.field(&mut node, "defaults");
        self.exprs(&arguments.defaults);
        self.close(node);
    }

    fn keyword(&mut self, keyword: &Keyword) {
        let mut node = self.open("keyword");
        self.field(&mut node, "arg");
        self.identifier(&keyword.arg);
        self.field(&mut node, "value");
        self.expr(&keyword.value);
        self.close(node);
    }

    fn alias(&mut self, alias: &Alias) {
        let mut node = self.open("alias");
        self.field(&mut node, "name");
        self.identifier(&alias.name);
        self.field(&mut node, "asname");
        self.opt_identifier(alias.asname.as_ref());
        self.close(node);
    }

    fn comprehensions(&mut self, generators: &[Comprehension]) {
        self.list(generators, |printer, generator| {
            let mut node = printer.open("comprehension");
            printer.field(&mut node, "target");
            printer.expr(&generator.target);
            printer.field(&mut node, "iter");
            printer.expr(&generator.iter);
            printer.field(&mut node, "ifs");
            printer.exprs(&generator.ifs);
            printer.close(node);
        });
    }
}
