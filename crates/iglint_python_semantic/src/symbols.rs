//! First pass: the scope tree and the names each scope binds.
//!
//! Python decides at compile time whether a name is local to a function: it
//! is if the function binds it anywhere (assignment, import, `def`, `for`,
//! `except ..., x`, `with ... as x`, `del`, a parameter, a list
//! comprehension variable) and does not declare it `global`. This pass
//! computes that for every scope before the flow pass resolves references.

use iglint_python::ast::*;
use iglint_text::{Ranged, TextRange};
use rustc_hash::FxHashMap;

use crate::model::{Scope, ScopeId, ScopeKind, ScopeNode};

pub(crate) struct SymbolTable<'a> {
    pub scopes: Vec<Scope<'a>>,
    pub scope_of_node: FxHashMap<usize, ScopeId>,
    pub dunder_all: Vec<String>,
    pub dunder_all_dynamic: bool,
}

struct Builder<'a> {
    scopes: Vec<Scope<'a>>,
    scope_of_node: FxHashMap<usize, ScopeId>,
    current: ScopeId,
    /// Per scope: every name used (any context) with its first range.
    uses: Vec<Vec<(&'a str, TextRange)>>,
    dunder_all: Vec<String>,
    dunder_all_dynamic: bool,
}

pub(crate) fn build(module: &Module) -> SymbolTable<'_> {
    let mut b = Builder {
        scopes: Vec::new(),
        scope_of_node: FxHashMap::default(),
        current: ScopeId::MODULE,
        uses: Vec::new(),
        dunder_all: Vec::new(),
        dunder_all_dynamic: false,
    };
    b.open(ScopeKind::Module, ScopeNode::Module(module), "<module>");
    for stmt in &module.body {
        b.stmt(stmt);
    }
    b.close();
    // Names functions bind through `global` belong to the module.
    let mut moved: Vec<&str> = Vec::new();
    for scope in &mut b.scopes[1..] {
        if scope.globals.is_empty() {
            continue;
        }
        let globals = &scope.globals;
        let mut kept = Vec::with_capacity(scope.locals.len());
        for name in scope.locals.drain(..) {
            if globals.contains_key(name) {
                moved.push(name);
            } else {
                kept.push(name);
            }
        }
        scope.locals = kept;
        scope.local_index = index(&scope.locals);
    }
    let module_scope = &mut b.scopes[0];
    for name in moved {
        if !module_scope.local_index.contains_key(name) {
            module_scope
                .local_index
                .insert(name, module_scope.locals.len() as u32);
            module_scope.locals.push(name);
        }
    }
    // Free names (Jython's definition, used for compile checks).
    for (i, uses) in b.uses.iter().enumerate() {
        let scope = &b.scopes[i];
        if scope.kind == ScopeKind::Module {
            continue;
        }
        let mut seen: FxHashMap<&str, ()> = FxHashMap::default();
        let mut free = Vec::new();
        for (name, range) in uses {
            if scope.local_index.contains_key(name) || scope.globals.contains_key(name) {
                continue;
            }
            if seen.insert(name, ()).is_none() {
                free.push((*name, *range));
            }
        }
        b.scopes[i].free_names = free;
    }
    SymbolTable {
        scopes: b.scopes,
        scope_of_node: b.scope_of_node,
        dunder_all: b.dunder_all,
        dunder_all_dynamic: b.dunder_all_dynamic,
    }
}

fn index<'a>(names: &[&'a str]) -> FxHashMap<&'a str, u32> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, i as u32))
        .collect()
}

/// Whether an exception type expression names `ImportError` (or a tuple
/// containing it), or is a bare `except:`.
pub(crate) fn catches(handler: &ExceptHandler, names: &[&str]) -> bool {
    fn names_one(e: &Expr, names: &[&str]) -> bool {
        match e {
            Expr::Name(n) => names.contains(&&*n.id),
            Expr::Attribute(a) => names.contains(&&*a.attr.id),
            Expr::Tuple(t) => t.elts.iter().any(|e| names_one(e, names)),
            _ => false,
        }
    }
    match &handler.type_ {
        None => true,
        Some(t) => names_one(t, names),
    }
}

impl<'a> Builder<'a> {
    fn open(&mut self, kind: ScopeKind, node: ScopeNode<'a>, name: &'a str) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        let parent = (!self.scopes.is_empty()).then_some(self.current);
        if let Some(p) = parent {
            self.scopes[p.index()].children.push(id);
        }
        self.scope_of_node.insert(node.key(), id);
        self.scopes.push(Scope {
            kind,
            node,
            parent,
            children: Vec::new(),
            name,
            locals: Vec::new(),
            local_index: FxHashMap::default(),
            globals: FxHashMap::default(),
            bindings: FxHashMap::default(),
            global_declarations: Vec::new(),
            star_import: None,
            unqualified_exec: None,
            reads_locals: false,
            writes_globals: false,
            is_generator: false,
            can_fall_through: false,
            free_names: Vec::new(),
        });
        self.uses.push(Vec::new());
        self.current = id;
        id
    }

    fn close(&mut self) {
        if let Some(parent) = self.scopes[self.current.index()].parent {
            self.current = parent;
        }
    }

    fn scope(&mut self) -> &mut Scope<'a> {
        &mut self.scopes[self.current.index()]
    }

    fn bind(&mut self, name: &'a str, range: TextRange) {
        self.uses[self.current.index()].push((name, range));
        let scope = self.scope();
        if !scope.local_index.contains_key(name) {
            scope.local_index.insert(name, scope.locals.len() as u32);
            scope.locals.push(name);
        }
    }

    fn use_name(&mut self, name: &'a str, range: TextRange) {
        self.uses[self.current.index()].push((name, range));
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
                self.bind(&f.name.id, f.name.range);
                self.open(ScopeKind::Function, ScopeNode::Function(f), &f.name.id);
                self.params(&f.args);
                self.body(&f.body);
                self.close();
            }
            Stmt::ClassDef(c) => {
                self.exprs(&c.decorator_list);
                self.exprs(&c.bases);
                self.open(ScopeKind::Class, ScopeNode::Class(c), &c.name.id);
                self.body(&c.body);
                self.close();
                self.bind(&c.name.id, c.name.range);
            }
            Stmt::Return(s) => self.opt_expr(s.value.as_deref()),
            Stmt::Delete(s) => {
                for t in &s.targets {
                    self.target(t);
                }
            }
            Stmt::Assign(s) => {
                self.expr(&s.value);
                for t in &s.targets {
                    self.target(t);
                }
                if self.current == ScopeId::MODULE {
                    self.dunder_all_assign(&s.targets, &s.value);
                }
                self.globals_write(&s.targets);
            }
            Stmt::AugAssign(s) => {
                self.expr(&s.value);
                self.target(&s.target);
                if self.current == ScopeId::MODULE
                    && matches!(&*s.target, Expr::Name(n) if &*n.id == "__all__")
                {
                    if s.op == Operator::Add {
                        self.dunder_all_extend(&s.value);
                    } else {
                        self.dunder_all_dynamic = true;
                    }
                }
            }
            Stmt::Print(s) => {
                self.opt_expr(s.dest.as_deref());
                self.exprs(&s.values);
            }
            Stmt::For(s) => {
                self.expr(&s.iter);
                self.target(&s.target);
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
                if let Some(v) = &s.optional_vars {
                    self.target(v);
                }
                self.body(&s.body);
            }
            Stmt::Raise(s) => {
                self.opt_expr(s.type_.as_deref());
                self.opt_expr(s.inst.as_deref());
                self.opt_expr(s.tback.as_deref());
            }
            Stmt::TryExcept(s) => {
                self.body(&s.body);
                for h in &s.handlers {
                    self.opt_expr(h.type_.as_deref());
                    if let Some(n) = &h.name {
                        self.target(n);
                    }
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
                self.opt_expr(s.msg.as_deref());
            }
            Stmt::Import(s) => {
                for alias in &s.names {
                    let (name, range) = import_binding(alias);
                    self.bind(name, range);
                }
            }
            Stmt::ImportFrom(s) => {
                for alias in &s.names {
                    if &*alias.name.id == "*" {
                        let scope = self.scope();
                        if scope.star_import.is_none() {
                            scope.star_import = Some(s.range);
                        }
                        continue;
                    }
                    let (name, range) = import_binding(alias);
                    self.bind(name, range);
                }
            }
            Stmt::Exec(s) => {
                self.expr(&s.body);
                self.opt_expr(s.globals.as_deref());
                self.opt_expr(s.locals.as_deref());
                if s.globals.is_none() {
                    let scope = self.scope();
                    if scope.unqualified_exec.is_none() {
                        scope.unqualified_exec = Some(s.range);
                    }
                    scope.reads_locals = true;
                } else if s.locals.is_none() || is_call_to(s.locals.as_deref(), "locals") {
                    // `exec code in globals()` / `in d, locals()` may read locals.
                    self.scope().reads_locals = true;
                }
            }
            Stmt::Global(s) => {
                for name in &s.names {
                    self.scope().globals.entry(&name.id).or_insert(name.range);
                }
            }
            Stmt::Expr(s) => {
                self.expr(&s.value);
                if self.current == ScopeId::MODULE {
                    self.dunder_all_call(&s.value);
                }
                self.globals_call(&s.value);
            }
            Stmt::Pass(_) | Stmt::Break(_) | Stmt::Continue(_) => {}
        }
    }

    fn params(&mut self, args: &'a Arguments) {
        for arg in &args.args {
            self.param(arg);
        }
        if let Some(v) = &args.vararg {
            self.bind(&v.id, v.range);
        }
        if let Some(k) = &args.kwarg {
            self.bind(&k.id, k.range);
        }
    }

    fn param(&mut self, arg: &'a Expr) {
        match arg {
            Expr::Name(n) => self.bind(&n.id, n.range),
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.param(e);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.param(e);
                }
            }
            other => self.expr(other),
        }
    }

    /// An assignment target (store or delete context).
    fn target(&mut self, target: &'a Expr) {
        match target {
            Expr::Name(n) => self.bind(&n.id, n.range),
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.target(e);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.target(e);
                }
            }
            other => self.expr(other),
        }
    }

    fn exprs(&mut self, exprs: &'a [Expr]) {
        for e in exprs {
            self.expr(e);
        }
    }

    fn opt_expr(&mut self, expr: Option<&'a Expr>) {
        if let Some(e) = expr {
            self.expr(e);
        }
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(n) => match n.ctx {
                ExprContext::Store | ExprContext::Del | ExprContext::AugStore => {
                    self.bind(&n.id, n.range)
                }
                _ => self.use_name(&n.id, n.range),
            },
            Expr::BoolOp(e) => self.exprs(&e.values),
            Expr::BinOp(e) => {
                self.expr(&e.left);
                self.expr(&e.right);
            }
            Expr::UnaryOp(e) => self.expr(&e.operand),
            Expr::Lambda(e) => {
                self.exprs(&e.args.defaults);
                self.open(ScopeKind::Lambda, ScopeNode::Lambda(e), "<lambda>");
                self.params(&e.args);
                self.expr(&e.body);
                self.close();
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
                for g in &e.generators {
                    self.expr(&g.iter);
                    self.target(&g.target);
                    self.exprs(&g.ifs);
                }
                self.expr(&e.elt);
            }
            Expr::SetComp(c) => self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::DictComp(c) => self.comprehension(expr, &c.generators, &[&c.key, &c.value]),
            Expr::GeneratorExp(c) => self.comprehension(expr, &c.generators, &[&c.elt]),
            Expr::Yield(e) => {
                self.opt_expr(e.value.as_deref());
                self.scope().is_generator = true;
            }
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
                self.opt_expr(e.starargs.as_deref());
                self.opt_expr(e.kwargs.as_deref());
                if let Expr::Name(f) = &*e.func {
                    let no_args = e.args.is_empty() && e.keywords.is_empty();
                    match &*f.id {
                        "locals" => self.scope().reads_locals = true,
                        "vars" | "dir" if no_args => self.scope().reads_locals = true,
                        "eval" | "execfile" if e.args.len() < 2 => self.scope().reads_locals = true,
                        _ => {}
                    }
                }
            }
            Expr::Repr(e) => self.expr(&e.value),
            Expr::Num(_) | Expr::Str(_) => {}
            Expr::Attribute(e) => self.expr(&e.value),
            Expr::Subscript(e) => {
                self.expr(&e.value);
                self.slice(&e.slice);
            }
            Expr::List(e) => {
                if matches!(e.ctx, ExprContext::Store | ExprContext::Del) {
                    for elt in &e.elts {
                        self.target(elt);
                    }
                } else {
                    self.exprs(&e.elts);
                }
            }
            Expr::Tuple(e) => {
                if matches!(e.ctx, ExprContext::Store | ExprContext::Del) {
                    for elt in &e.elts {
                        self.target(elt);
                    }
                } else {
                    self.exprs(&e.elts);
                }
            }
        }
    }

    fn slice(&mut self, slice: &'a Slice) {
        match slice {
            Slice::Ellipsis(_) => {}
            Slice::Slice(s) => {
                self.opt_expr(s.lower.as_deref());
                self.opt_expr(s.upper.as_deref());
                self.opt_expr(s.step.as_deref());
            }
            Slice::ExtSlice(s) => {
                for d in &s.dims {
                    self.slice(d);
                }
            }
            Slice::Index(s) => self.expr(&s.value),
        }
    }

    /// A generator expression or set/dict comprehension: the first iterable
    /// is evaluated in the enclosing scope, the rest in the new scope.
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
        let name = match node {
            Expr::SetComp(_) => "<setcomp>",
            Expr::DictComp(_) => "<dictcomp>",
            _ => "<genexpr>",
        };
        self.open(ScopeKind::Generator, ScopeNode::Generator(node), name);
        for (i, g) in generators.iter().enumerate() {
            if i > 0 {
                self.expr(&g.iter);
            }
            self.target(&g.target);
            self.exprs(&g.ifs);
        }
        for e in elts {
            self.expr(e);
        }
        self.close();
    }

    // ---- `__all__` ----

    fn dunder_all_assign(&mut self, targets: &[Expr], value: &Expr) {
        if !targets
            .iter()
            .any(|t| matches!(t, Expr::Name(n) if &*n.id == "__all__"))
        {
            return;
        }
        self.dunder_all.clear();
        self.dunder_all_extend(value);
    }

    fn dunder_all_extend(&mut self, value: &Expr) {
        let elts = match value {
            Expr::List(l) => &l.elts,
            Expr::Tuple(t) => &t.elts,
            _ => {
                self.dunder_all_dynamic = true;
                return;
            }
        };
        for e in elts {
            match e {
                Expr::Str(s) => self.dunder_all.push(s.s.to_string_lossy().into_owned()),
                _ => self.dunder_all_dynamic = true,
            }
        }
    }

    fn dunder_all_call(&mut self, value: &Expr) {
        let Expr::Call(call) = value else { return };
        let Expr::Attribute(a) = &*call.func else {
            return;
        };
        if !matches!(&*a.value, Expr::Name(n) if &*n.id == "__all__") {
            return;
        }
        match (&*a.attr.id, call.args.as_slice()) {
            ("append", [Expr::Str(s)]) => {
                self.dunder_all.push(s.s.to_string_lossy().into_owned());
            }
            ("extend", [arg]) => self.dunder_all_extend(arg),
            _ => self.dunder_all_dynamic = true,
        }
    }

    // ---- writes through globals() ----

    /// `globals()['x'] = ...`, `vars()['x'] = ...`, `locals()['x'] = ...`.
    fn globals_write(&mut self, targets: &[Expr]) {
        for t in targets {
            if let Expr::Subscript(s) = t
                && namespace_call(&s.value).is_some()
            {
                self.mark_namespace_write(namespace_call(&s.value));
            }
        }
    }

    /// `globals().update(...)`, `globals().setdefault(...)`, ...
    fn globals_call(&mut self, value: &Expr) {
        let Expr::Call(call) = value else { return };
        if let Expr::Attribute(a) = &*call.func
            && matches!(
                &*a.attr.id,
                "update" | "setdefault" | "__setitem__" | "pop" | "__delitem__"
            )
            && namespace_call(&a.value).is_some()
        {
            self.mark_namespace_write(namespace_call(&a.value));
        }
    }

    fn mark_namespace_write(&mut self, function: Option<&str>) {
        match function {
            Some("globals") => self.scopes[0].writes_globals = true,
            Some(_) => {
                // locals()/vars() in a function cannot create variables, but
                // at module level they are the module namespace.
                self.scope().writes_globals = true;
                if self.current == ScopeId::MODULE {
                    self.scopes[0].writes_globals = true;
                }
            }
            None => {}
        }
    }
}

/// `globals()`, `locals()` or `vars()`: the function name.
fn namespace_call(e: &Expr) -> Option<&str> {
    let Expr::Call(call) = e else { return None };
    let Expr::Name(n) = &*call.func else {
        return None;
    };
    (call.args.is_empty() && matches!(&*n.id, "globals" | "locals" | "vars")).then_some(&*n.id)
}

fn is_call_to(e: Option<&Expr>, name: &str) -> bool {
    e.and_then(namespace_call) == Some(name)
}

/// The name an import alias binds: `asname`, or the first component of the
/// dotted name.
pub(crate) fn import_binding(alias: &Alias) -> (&str, TextRange) {
    match &alias.asname {
        Some(asname) => (&asname.id, asname.range),
        None => {
            let full = &*alias.name.id;
            let first = full.split('.').next().unwrap_or(full);
            let range = alias.name.range();
            let range = TextRange::at(range.start(), iglint_text::TextSize::of(first));
            (first, range)
        }
    }
}
