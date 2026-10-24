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
    /// Which import statements bind [`PACKAGE`] in the module namespace.
    pub package: PackageImports,
    /// An import outside the module's own code (in a function or class
    /// body) binds [`PACKAGE`].
    pub package_bound_elsewhere: bool,
}

/// The flags of Jython's symbol table (`org.python.compiler.ScopeConstants`,
/// plus [`jython::USED`]), and its `cook` and `setup_closure` passes.
pub(crate) mod jython {
    pub const BOUND: u8 = 1;
    pub const NGLOBAL: u8 = 2;
    pub const PARAM: u8 = 4;
    pub const CELL: u8 = 16;
    pub const FREE: u8 = 32;
    pub const CLASS_GLOBAL: u8 = 64;
    /// Jython's `GLOBAL` mask.
    pub const GLOBAL: u8 = NGLOBAL | CLASS_GLOBAL;
    /// Not a Jython flag: the name is in the table (Jython's entries with
    /// flags 0, names that are only read).
    pub const USED: u8 = 128;
}

/// The name an import statement binds implicitly in the module's globals.
pub(crate) const PACKAGE: &str = "__package__";

/// Which import statements bind `__package__` in the module namespace when
/// they run.
///
/// Measured on Ignition 8.3.3's Jython (`oracle/run-oracle.sh --implicit`,
/// `tests/oracle/implicit_names.expected`): Jython's import computes the
/// importing module's package from the `__name__` of its globals and stores
/// it as `__package__` (`None` for a name without a dot) before it looks the
/// module up, so every absolute import statement that runs binds it, a
/// failing one too, wherever it is (module, function or class body). A
/// relative import in a module whose name has no dot raises `ValueError`
/// first. Where the globals have no `__name__` (event scripts) nothing is
/// bound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PackageImports {
    pub absolute: bool,
    pub relative: bool,
}

impl PackageImports {
    pub(crate) fn binds(self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Import(_) => self.absolute,
            Stmt::ImportFrom(s) if s.level == 0 => self.absolute,
            Stmt::ImportFrom(_) => self.relative,
            _ => false,
        }
    }
}

struct Builder<'a> {
    scopes: Vec<Scope<'a>>,
    scope_of_node: FxHashMap<usize, ScopeId>,
    current: ScopeId,
    /// Per scope: every name used (any context) with its first range.
    uses: Vec<Vec<(&'a str, TextRange)>>,
    dunder_all: Vec<String>,
    dunder_all_dynamic: bool,
    /// Every import statement, with the scope whose code runs it.
    imports: Vec<(ScopeId, &'a Stmt)>,
}

pub(crate) fn build(module: &Module, module_has_name: bool) -> SymbolTable<'_> {
    let mut b = Builder {
        scopes: Vec::new(),
        scope_of_node: FxHashMap::default(),
        current: ScopeId::MODULE,
        uses: Vec::new(),
        dunder_all: Vec::new(),
        dunder_all_dynamic: false,
        imports: Vec::new(),
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
    // The code binds `__name__` itself: an import may then compute any
    // package, relative imports included.
    let binds_name =
        b.scopes[0].local_index.contains_key("__name__") || moved.contains(&"__name__");
    let package = PackageImports {
        absolute: module_has_name || binds_name,
        relative: binds_name,
    };
    let mut package_bound_elsewhere = false;
    let mut package_bound = false;
    for (scope, stmt) in &b.imports {
        if package.binds(stmt) {
            package_bound = true;
            package_bound_elsewhere |= *scope != ScopeId::MODULE;
        }
    }
    if package_bound {
        moved.push(PACKAGE);
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
            let flags = scope.jython.get(name).copied().unwrap_or(0);
            if flags & (jython::BOUND | jython::NGLOBAL) != 0 {
                continue;
            }
            if seen.insert(name, ()).is_none() {
                free.push((*name, *range));
            }
        }
        b.scopes[i].free_names = free;
    }
    cook(&mut b.scopes);
    SymbolTable {
        scopes: b.scopes,
        scope_of_node: b.scope_of_node,
        dunder_all: b.dunder_all,
        dunder_all_dynamic: b.dunder_all_dynamic,
        package,
        package_bound_elsewhere,
    }
}

/// Jython's `ScopeInfo.cook` (bottom-up: which names are free in each
/// scope and which locals are cells) and `setup_closure` (top-down: a free
/// name stays free only if the enclosing scope has it as a cell or free
/// variable; one the enclosing function declares `global` becomes an
/// explicit global; any other becomes an ordinary global read). Both look
/// past class bodies: the "enclosing scope" is the nearest function or the
/// module (`ScopesCompiler.endScope` skips class scopes). Read in Jython's
/// bytecode and checked with `oracle/scope/`.
fn cook(scopes: &mut [Scope<'_>]) {
    use jython::*;
    let n = scopes.len();
    let mut inner_free: Vec<Vec<&str>> = vec![Vec::new(); n];
    // Children have larger ids than their parents.
    for i in (1..n).rev() {
        let function = scopes[i].kind.is_function_like();
        let parent = enclosing_function(scopes, i);
        let nested = parent != ScopeId::MODULE;
        let names = std::mem::take(&mut inner_free[i]);
        let table = &mut scopes[i].jython;
        for name in names {
            match table.get_mut(name) {
                None => {
                    table.insert(name, FREE);
                }
                Some(flags) => {
                    if !function {
                        *flags |= FREE;
                    } else if *flags & NGLOBAL == 0 && *flags & BOUND != 0 {
                        *flags |= CELL;
                    }
                }
            }
        }
        let mut up = Vec::new();
        for (name, flags) in table.iter_mut() {
            if *flags & FREE != 0 && nested {
                up.push(*name);
            }
            if *flags & (NGLOBAL | PARAM | CELL | CLASS_GLOBAL) != 0 || *flags & BOUND != 0 {
                continue;
            }
            *flags |= FREE;
            if nested {
                up.push(*name);
            }
        }
        let parent_free = &mut inner_free[parent.index()];
        for name in up {
            if !parent_free.contains(&name) {
                parent_free.push(name);
            }
        }
    }
    for i in 1..n {
        let parent = enclosing_function(scopes, i);
        let mut changes: Vec<(&str, u8)> = Vec::new();
        for (name, flags) in &scopes[i].jython {
            if flags & FREE == 0 {
                continue;
            }
            let up = scopes[parent.index()].jython.get(name).copied();
            match up {
                Some(u) if u & (CELL | FREE) != 0 => {}
                Some(u) if parent != ScopeId::MODULE && u & NGLOBAL != 0 => {
                    changes.push((name, BOUND | NGLOBAL));
                }
                _ => changes.push((name, flags & !FREE)),
            }
        }
        for (name, flags) in changes {
            scopes[i].jython.insert(name, flags);
        }
    }
}

/// The nearest scope enclosing scope `i` that is not a class body.
pub(crate) fn enclosing_function(scopes: &[Scope<'_>], i: usize) -> ScopeId {
    let mut at = scopes[i].parent.unwrap_or(ScopeId::MODULE);
    while scopes[at.index()].kind == ScopeKind::Class {
        at = scopes[at.index()].parent.unwrap_or(ScopeId::MODULE);
    }
    at
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
        let enclosing_class = parent.and_then(|p| self.scopes[p.index()].class_name);
        let class_name = if kind == ScopeKind::Class {
            Some(enclosing_class.map_or(name, |c| crate::mangle::mangle(c, name)))
        } else {
            enclosing_class
        };
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
            jython: FxHashMap::default(),
            has_exec: false,
            class_name,
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

    /// Adds Jython symbol table flags for `name` (as spelled).
    fn flag(&mut self, name: &'a str, flags: u8) {
        *self.scope().jython.entry(name).or_insert(0) |= flags | jython::USED;
    }

    /// A name the current scope's code binds (as spelled in the source;
    /// the scope binds its run-time spelling, see [`Scope::runtime_name`]).
    fn bind(&mut self, name: &'a str, range: TextRange) {
        self.uses[self.current.index()].push((name, range));
        self.flag(name, jython::BOUND);
        let scope = self.scope();
        let name = scope.runtime_name(name);
        if !scope.local_index.contains_key(name) {
            scope.local_index.insert(name, scope.locals.len() as u32);
            scope.locals.push(name);
        }
    }

    fn param_name(&mut self, name: &'a str, range: TextRange) {
        self.bind(name, range);
        self.flag(name, jython::PARAM);
    }

    fn use_name(&mut self, name: &'a str, range: TextRange) {
        self.uses[self.current.index()].push((name, range));
        self.flag(name, 0);
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
                self.module_attribute_write(&s.targets);
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
                self.imports.push((self.current, stmt));
                for alias in &s.names {
                    let (name, range) = import_binding(alias);
                    self.bind(name, range);
                }
            }
            Stmt::ImportFrom(s) => {
                self.imports.push((self.current, stmt));
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
                self.scope().has_exec = true;
                self.expr(&s.body);
                self.opt_expr(s.globals.as_deref());
                self.opt_expr(s.locals.as_deref());
                if s.globals.is_none() {
                    let scope = self.scope();
                    if scope.unqualified_exec.is_none() {
                        scope.unqualified_exec = Some(s.range);
                    }
                    scope.reads_locals = true;
                } else {
                    if s.locals.is_none() || is_call_to(s.locals.as_deref(), "locals") {
                        // `exec code in globals()` / `in d, locals()` may read locals.
                        self.scope().reads_locals = true;
                    }
                    // Names the code binds go to the locals mapping (the
                    // globals one when there is none). Verified on Jython:
                    // a function with `exec code in globals()` (or `in
                    // locals()`, `vars()`, `d, globals()`) sees the new
                    // names.
                    let target = s.locals.as_deref().or(s.globals.as_deref());
                    self.mark_namespace_write(target.and_then(namespace_of));
                }
            }
            Stmt::Global(s) => {
                for name in &s.names {
                    self.flag(&name.id, jython::NGLOBAL);
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
            self.param_name(&v.id, v.range);
        }
        if let Some(k) = &args.kwarg {
            self.param_name(&k.id, k.range);
        }
    }

    fn param(&mut self, arg: &'a Expr) {
        match arg {
            Expr::Name(n) => self.param_name(&n.id, n.range),
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

    /// `globals()['x'] = ...`, `vars()['x'] = ...`, `locals()['x'] = ...`,
    /// `__builtins__['x'] = ...`, `sys.modules[__name__].__dict__['x'] = ...`.
    fn globals_write(&mut self, targets: &[Expr]) {
        for t in targets {
            if let Expr::Subscript(s) = t
                && let Some(ns) = namespace_of(&s.value)
            {
                self.mark_namespace_write(Some(ns));
            }
        }
    }

    /// `sys.modules[__name__].x = ...`, `__builtin__.x = ...`: attributes
    /// of the module (or of the builtins, visible everywhere).
    fn module_attribute_write(&mut self, targets: &[Expr]) {
        for t in targets {
            if let Expr::Attribute(a) = t
                && is_module_object(&a.value)
            {
                self.mark_namespace_write(Some("globals"));
            }
        }
    }

    /// `globals().update(...)`, `globals().setdefault(...)`, ...,
    /// `setattr(sys.modules[__name__], ...)`, `setattr(__builtin__, ...)`.
    fn globals_call(&mut self, value: &Expr) {
        let Expr::Call(call) = value else { return };
        if let Expr::Attribute(a) = &*call.func
            && matches!(
                &*a.attr.id,
                "update" | "setdefault" | "__setitem__" | "pop" | "__delitem__"
            )
            && let Some(ns) = namespace_of(&a.value)
        {
            self.mark_namespace_write(Some(ns));
        }
        if let Expr::Name(f) = &*call.func
            && &*f.id == "setattr"
            && call.args.first().is_some_and(is_module_object)
        {
            self.mark_namespace_write(Some("globals"));
        }
    }

    /// Records a write to a namespace: `"globals"` for the module (or the
    /// builtins), `"locals"`/`"vars"` for the current scope's.
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

/// The namespace a mapping expression is: `"globals"` for the module's
/// (`globals()`, `sys.modules[__name__].__dict__`,
/// `vars(sys.modules[__name__])`) or the builtins (`__builtins__`,
/// `__builtin__.__dict__`), `"locals"`/`"vars"` for the current scope's.
fn namespace_of(e: &Expr) -> Option<&'static str> {
    match namespace_call(e) {
        Some("globals") => return Some("globals"),
        Some("locals") => return Some("locals"),
        Some(_) => return Some("vars"),
        None => {}
    }
    match e {
        Expr::Name(n) if &*n.id == "__builtins__" => Some("globals"),
        Expr::Attribute(a) if &*a.attr.id == "__dict__" && is_module_object(&a.value) => {
            Some("globals")
        }
        Expr::Call(c)
            if matches!(&*c.func, Expr::Name(n) if &*n.id == "vars")
                && c.args.len() == 1
                && is_module_object(&c.args[0]) =>
        {
            Some("globals")
        }
        _ => None,
    }
}

/// `sys.modules[__name__]` (the current module), `__builtin__` or
/// `__builtins__`: setting an attribute creates a global name.
fn is_module_object(e: &Expr) -> bool {
    match e {
        Expr::Name(n) => matches!(&*n.id, "__builtin__" | "__builtins__"),
        Expr::Subscript(s) => {
            matches!(&*s.value, Expr::Attribute(a)
                if &*a.attr.id == "modules"
                    && matches!(&*a.value, Expr::Name(n) if &*n.id == "sys"))
        }
        _ => false,
    }
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
