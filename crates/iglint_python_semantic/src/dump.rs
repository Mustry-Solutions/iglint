//! The model's view of how compiled code reaches every name, in the format
//! of Jython's own (`oracle/scope/jython_scope_oracle.py`), for the
//! differential `scope_oracle` test.
//!
//! Per code object (module, class body, function, lambda, generator
//! expression): every read, store and delete of a name with its
//! [`NameAccess`] and run-time spelling (mangled in class bodies), and every
//! attribute name (mangled in scopes nested in a class), each once, in
//! the code the bytecode can reach. Everything comes
//! from the model's references and bindings; only attribute names and the
//! names Jython's class prologue and docstrings store come from the tree.

use std::collections::BTreeMap;
use std::fmt::Write;

use iglint_python::ast::*;
use iglint_text::{LineIndex, Ranged};

use crate::emitted::Emission;
use crate::model::{
    BindingKind, ImplicitRead, NameAccess, ReferenceKind, ScopeId, ScopeKind, ScopeNode,
    SemanticModel,
};

type Ops = std::collections::BTreeSet<(String, String, String)>;

/// The dump of `model` (built from `source`).
pub fn scope_dump(model: &SemanticModel<'_>, source: &str) -> String {
    let lines = LineIndex::new(source);
    let emission = Emission::of(model);
    let mut scopes: BTreeMap<(usize, &'static str, String), Ops> = BTreeMap::new();
    let mut key_of: Vec<(usize, &'static str, String)> = Vec::new();
    for (id, scope) in model.scopes() {
        let (kind, line) = match scope.node {
            ScopeNode::Module(_) => ("module", 1),
            ScopeNode::Class(c) => ("class", lines.line_of(c.range.start()) + 1),
            ScopeNode::Function(f) => ("function", lines.line_of(f.range.start()) + 1),
            ScopeNode::Lambda(l) => ("function", lines.line_of(l.range.start()) + 1),
            ScopeNode::Generator(e) => ("function", lines.line_of(e.range().start()) + 1),
        };
        let name = match scope.kind {
            ScopeKind::Class | ScopeKind::Function => match scope.parent {
                Some(p) => model.scope(p).runtime_attribute(scope.name).to_owned(),
                None => scope.name.to_owned(),
            },
            ScopeKind::Generator => "<genexpr>".to_owned(),
            _ => scope.name.to_owned(),
        };
        let key = (line, kind, name);
        if !emission.dead_scopes.contains(&id) {
            scopes.entry(key.clone()).or_default();
        }
        key_of.push(key);
        if emission.dead_scopes.contains(&id) {
            continue;
        }
        let ops = scopes.get_mut(&key_of[id.index()]).unwrap();
        match scope.node {
            ScopeNode::Class(c) => {
                add(ops, "store", "name", "__module__");
                if has_docstring(&c.body) {
                    add(ops, "store", "name", "__doc__");
                }
            }
            ScopeNode::Module(m) if has_docstring(&m.body) => {
                add(ops, "store", "global", "__doc__");
            }
            _ => {}
        }
    }
    for (_, r) in model.references() {
        if !emission.runs(r.scope, r.range) {
            continue;
        }
        let op = match r.kind {
            ReferenceKind::Load | ReferenceKind::AugLoad => "load",
            ReferenceKind::Del => "del",
            ReferenceKind::Implicit(ImplicitRead::ClassModule | ImplicitRead::Assert) => "load",
            ReferenceKind::Implicit(_) => continue,
        };
        let access = access_name(model, r.scope, r.access, r.name);
        add(
            scopes.get_mut(&key_of[r.scope.index()]).unwrap(),
            op,
            access,
            r.name,
        );
    }
    for (_, b) in model.bindings() {
        let stores = matches!(
            b.kind,
            BindingKind::Import
                | BindingKind::FromImport
                | BindingKind::FutureImport
                | BindingKind::Assignment
                | BindingKind::AugAssignment
                | BindingKind::FunctionDef
                | BindingKind::ClassDef
                | BindingKind::LoopVar
                | BindingKind::ComprehensionVar
                | BindingKind::ExceptVar
                | BindingKind::WithVar
        );
        if !stores || !emission.runs(b.site, b.range) {
            continue;
        }
        let access = access_name(model, b.site, b.access, b.name);
        add(
            scopes.get_mut(&key_of[b.site.index()]).unwrap(),
            "store",
            access,
            b.name,
        );
    }
    for (scope, op, name, range) in attributes(model) {
        if !emission.runs(scope, range) {
            continue;
        }
        let spelled = model.scope(scope).runtime_attribute(name);
        add(
            scopes.get_mut(&key_of[scope.index()]).unwrap(),
            op,
            "-",
            spelled,
        );
    }
    let mut out = String::new();
    for ((line, kind, name), ops) in &scopes {
        let _ = writeln!(out, "scope {kind} {name} {line}");
        for (op, access, name) in ops {
            let _ = writeln!(out, "{op} {access} {name}");
        }
    }
    out
}

fn add(ops: &mut Ops, op: &str, access: &str, name: &str) {
    ops.insert((op.to_owned(), access.to_owned(), name.to_owned()));
}

/// The oracle's spelling of an access: `global!` for a name the scope
/// itself declares `global`.
fn access_name(
    model: &SemanticModel<'_>,
    scope: ScopeId,
    access: NameAccess,
    name: &str,
) -> &'static str {
    if access == NameAccess::Global && model.scope(scope).declares_global(name) {
        "global!"
    } else {
        access.name()
    }
}

fn has_docstring(body: &[Stmt]) -> bool {
    matches!(body.first(), Some(Stmt::Expr(e)) if matches!(&*e.value, Expr::Str(_)))
}

/// Every attribute name the code uses, as spelled in the source: the scope
/// whose code uses it, `attr-load`, `attr-store` or `attr-del`, the name and
/// its range (Jython's list comprehension accumulator `append` included).
pub(crate) fn attributes<'a>(
    model: &SemanticModel<'a>,
) -> Vec<(ScopeId, &'static str, &'a str, iglint_text::TextRange)> {
    let mut attrs = Attributes {
        model,
        ops: Vec::new(),
        scope: ScopeId::MODULE,
    };
    attrs.body(&model.module.body);
    attrs.ops
}

struct Attributes<'m, 'a> {
    model: &'m SemanticModel<'a>,
    ops: Vec<(ScopeId, &'static str, &'a str, iglint_text::TextRange)>,
    scope: ScopeId,
}

impl<'a> Attributes<'_, 'a> {
    fn attr(&mut self, op: &'static str, name: &'a Identifier) {
        self.ops.push((self.scope, op, &name.id, name.range));
    }

    fn within(&mut self, scope: Option<ScopeId>, f: impl FnOnce(&mut Self)) {
        let saved = self.scope;
        if let Some(scope) = scope {
            self.scope = scope;
        }
        f(self);
        self.scope = saved;
    }

    fn body(&mut self, body: &'a [Stmt]) {
        for stmt in body {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(f) => {
                self.exprs(&f.decorator_list);
                self.exprs(&f.args.defaults);
                let scope = self.model.function_scope(f);
                self.within(scope, |s| {
                    s.exprs(&f.args.args);
                    s.body(&f.body);
                });
            }
            Stmt::ClassDef(c) => {
                self.exprs(&c.decorator_list);
                self.exprs(&c.bases);
                let scope = self.model.class_scope(c);
                self.within(scope, |s| s.body(&c.body));
            }
            Stmt::Return(s) => self.opt(s.value.as_deref()),
            Stmt::Delete(s) => self.exprs(&s.targets),
            Stmt::Assign(s) => {
                self.expr(&s.value);
                self.exprs(&s.targets);
            }
            Stmt::AugAssign(s) => {
                if let Expr::Attribute(a) = &*s.target {
                    self.expr(&a.value);
                    self.attr("attr-load", &a.attr);
                    self.attr("attr-store", &a.attr);
                } else {
                    self.expr(&s.target);
                }
                self.expr(&s.value);
            }
            Stmt::Print(s) => {
                self.opt(s.dest.as_deref());
                self.exprs(&s.values);
            }
            Stmt::For(s) => {
                self.expr(&s.iter);
                self.expr(&s.target);
                self.body(&s.body);
                self.body(&s.orelse);
            }
            Stmt::While(s) => {
                self.expr(&s.test);
                self.body(&s.body);
                self.body(&s.orelse);
            }
            Stmt::If(s) => {
                self.expr(&s.test);
                self.body(&s.body);
                self.body(&s.orelse);
            }
            Stmt::With(s) => {
                self.expr(&s.context_expr);
                self.opt(s.optional_vars.as_deref());
                self.body(&s.body);
            }
            Stmt::Raise(s) => {
                self.opt(s.type_.as_deref());
                self.opt(s.inst.as_deref());
                self.opt(s.tback.as_deref());
            }
            Stmt::TryExcept(s) => {
                self.body(&s.body);
                for h in &s.handlers {
                    self.opt(h.type_.as_deref());
                    self.opt(h.name.as_deref());
                    self.body(&h.body);
                }
                self.body(&s.orelse);
            }
            Stmt::TryFinally(s) => {
                self.body(&s.body);
                self.body(&s.finalbody);
            }
            Stmt::Assert(s) => {
                self.expr(&s.test);
                self.opt(s.msg.as_deref());
            }
            Stmt::Exec(s) => {
                self.expr(&s.body);
                self.opt(s.globals.as_deref());
                self.opt(s.locals.as_deref());
            }
            Stmt::Expr(s) => self.expr(&s.value),
            Stmt::Import(_)
            | Stmt::ImportFrom(_)
            | Stmt::Global(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_) => {}
        }
    }

    fn opt(&mut self, e: Option<&'a Expr>) {
        if let Some(e) = e {
            self.expr(e);
        }
    }

    fn exprs(&mut self, exprs: &'a [Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    fn comprehension(
        &mut self,
        node: &'a Expr,
        generators: &'a [Comprehension],
        elts: &[&'a Expr],
    ) {
        let Some(first) = generators.first() else {
            return;
        };
        self.expr(&first.iter);
        let scope = self.model.generator_scope(node);
        self.within(scope, |s| {
            for (i, g) in generators.iter().enumerate() {
                if i > 0 {
                    s.expr(&g.iter);
                }
                s.expr(&g.target);
                s.exprs(&g.ifs);
            }
            for e in elts {
                s.expr(e);
            }
        });
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Attribute(a) => {
                self.expr(&a.value);
                let op = match a.ctx {
                    ExprContext::Store | ExprContext::AugStore => "attr-store",
                    ExprContext::Del => "attr-del",
                    _ => "attr-load",
                };
                self.attr(op, &a.attr);
            }
            Expr::Name(_) | Expr::Num(_) | Expr::Str(_) => {}
            Expr::BoolOp(e) => self.exprs(&e.values),
            Expr::BinOp(e) => {
                self.expr(&e.left);
                self.expr(&e.right);
            }
            Expr::UnaryOp(e) => self.expr(&e.operand),
            Expr::Lambda(e) => {
                self.exprs(&e.args.defaults);
                let scope = self.model.lambda_scope(e);
                self.within(scope, |s| {
                    s.exprs(&e.args.args);
                    s.expr(&e.body);
                });
            }
            Expr::IfExp(e) => {
                self.expr(&e.test);
                self.expr(&e.body);
                self.expr(&e.orelse);
            }
            Expr::Dict(e) => {
                self.exprs(&e.keys);
                self.exprs(&e.values);
            }
            Expr::Set(e) => self.exprs(&e.elts),
            Expr::ListComp(e) => {
                // The accumulator's `append` (Jython's temporary list).
                let at = iglint_text::TextRange::empty(e.range.start());
                self.ops.push((self.scope, "attr-load", "append", at));
                for g in &e.generators {
                    self.expr(&g.iter);
                    self.expr(&g.target);
                    self.exprs(&g.ifs);
                }
                self.expr(&e.elt);
            }
            Expr::SetComp(c) => self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::DictComp(c) => self.comprehension(expr, &c.generators, &[&c.key, &c.value]),
            Expr::GeneratorExp(c) => self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::Yield(e) => self.opt(e.value.as_deref()),
            Expr::Compare(e) => {
                self.expr(&e.left);
                self.exprs(&e.comparators);
            }
            Expr::Call(e) => {
                self.expr(&e.func);
                self.exprs(&e.args);
                for k in &e.keywords {
                    self.expr(&k.value);
                }
                self.opt(e.starargs.as_deref());
                self.opt(e.kwargs.as_deref());
            }
            Expr::Repr(e) => self.expr(&e.value),
            Expr::Subscript(e) => {
                self.expr(&e.value);
                self.slice(&e.slice);
            }
            Expr::List(e) => self.exprs(&e.elts),
            Expr::Tuple(e) => self.exprs(&e.elts),
        }
    }

    fn slice(&mut self, slice: &'a Slice) {
        match slice {
            Slice::Ellipsis(_) => {}
            Slice::Slice(s) => {
                self.opt(s.lower.as_deref());
                self.opt(s.upper.as_deref());
                self.opt(s.step.as_deref());
            }
            Slice::ExtSlice(s) => {
                for d in &s.dims {
                    self.slice(d);
                }
            }
            Slice::Index(s) => self.expr(&s.value),
        }
    }
}
