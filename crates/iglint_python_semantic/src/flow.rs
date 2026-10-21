//! Second pass: bindings, references and control flow.
//!
//! Every scope body is walked once in breadth-first order (a scope after the
//! scopes that enclose it), with a flow state that maps each local name to
//! the bindings that may reach the current point (plus "unbound" when some
//! path has no binding). Loops are iterated to a fixed point and `finally`
//! clauses are walked once for the normal path and once for the
//! exceptional and jump paths; recording bindings and references is
//! idempotent (keyed by source position), so re-walking only widens the
//! sets. Exceptions are modelled conservatively: an `except` handler starts
//! from the state before the `try` plus every binding made in its body.

use std::collections::VecDeque;

use iglint_python::ast::*;
use iglint_text::TextRange;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::*;
use crate::symbols::{SymbolTable, catches, import_binding};

const UNBOUND: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    reachable: bool,
    /// Per local index: sorted binding ids, `UNBOUND` last.
    defs: Vec<Vec<u32>>,
}

impl State {
    fn new(locals: usize) -> State {
        State {
            reachable: true,
            defs: vec![vec![UNBOUND]; locals],
        }
    }

    fn unreachable(locals: usize) -> State {
        State {
            reachable: false,
            defs: vec![Vec::new(); locals],
        }
    }

    fn join(&mut self, other: &State) {
        if !other.reachable {
            return;
        }
        if !self.reachable {
            *self = other.clone();
            return;
        }
        for (mine, theirs) in self.defs.iter_mut().zip(&other.defs) {
            union(mine, theirs);
        }
    }

    fn add(&mut self, index: usize, id: u32) {
        let defs = &mut self.defs[index];
        if let Err(at) = defs.binary_search(&id) {
            defs.insert(at, id);
        }
    }
}

fn union(into: &mut Vec<u32>, from: &[u32]) {
    if into == from {
        return;
    }
    for id in from {
        if let Err(at) = into.binary_search(id) {
            into.insert(at, *id);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Jump {
    Break,
    Continue,
    Return,
}

enum Frame {
    Loop {
        breaks: Vec<State>,
        continues: Vec<State>,
    },
    Finally {
        jumps: Vec<(Jump, State)>,
    },
}

#[derive(Clone, Copy)]
enum Event {
    Bind(u32, u32),
    Del(u32),
}

pub(crate) struct Output<'a> {
    pub scopes: Vec<Scope<'a>>,
    pub bindings: Vec<Binding<'a>>,
    pub references: Vec<Reference<'a>>,
    pub scope_of_node: FxHashMap<usize, ScopeId>,
    pub implicit: FxHashMap<&'a str, BindingId>,
    pub unreachable: Vec<(&'a Stmt, &'a Stmt)>,
    pub reference_at: FxHashMap<u32, ReferenceId>,
    pub dunder_all: Vec<String>,
    pub dunder_all_dynamic: bool,
}

pub(crate) struct Flow<'a, 'o> {
    module: &'a Module,
    scopes: Vec<Scope<'a>>,
    bindings: Vec<Binding<'a>>,
    references: Vec<Reference<'a>>,
    scope_of_node: FxHashMap<usize, ScopeId>,
    injected: &'o FxHashSet<&'o str>,
    implicit: FxHashMap<&'a str, BindingId>,
    binding_at: FxHashMap<u32, BindingId>,
    global_at: FxHashMap<u32, BindingId>,
    reference_at: FxHashMap<u32, ReferenceId>,
    /// Whether each statement (by address) was ever reached.
    stmt_reachable: FxHashMap<usize, bool>,
    queued: Vec<bool>,
    queue: VecDeque<ScopeId>,
    // Per walked scope.
    scope: ScopeId,
    frames: Vec<Frame>,
    collectors: Vec<Vec<Event>>,
    try_parts: Vec<(TextRange, TryPart)>,
    name_error_guard: u32,
    import_guard: u32,
    dunder_all: Vec<String>,
    dunder_all_dynamic: bool,
}

impl<'a, 'o> Flow<'a, 'o> {
    pub fn new(
        module: &'a Module,
        table: SymbolTable<'a>,
        injected: &'o FxHashSet<&'o str>,
    ) -> Self {
        let n = table.scopes.len();
        Flow {
            module,
            scopes: table.scopes,
            bindings: Vec::new(),
            references: Vec::new(),
            scope_of_node: table.scope_of_node,
            injected,
            implicit: FxHashMap::default(),
            binding_at: FxHashMap::default(),
            global_at: FxHashMap::default(),
            reference_at: FxHashMap::default(),
            stmt_reachable: FxHashMap::default(),
            queued: vec![false; n],
            queue: VecDeque::new(),
            scope: ScopeId::MODULE,
            frames: Vec::new(),
            collectors: Vec::new(),
            try_parts: Vec::new(),
            name_error_guard: 0,
            import_guard: 0,
            dunder_all: table.dunder_all,
            dunder_all_dynamic: table.dunder_all_dynamic,
        }
    }

    pub fn run(mut self) -> Output<'a> {
        self.enqueue(ScopeId::MODULE);
        while let Some(id) = self.queue.pop_front() {
            self.walk_scope(id);
        }
        self.finish()
    }

    fn enqueue(&mut self, id: ScopeId) {
        if !self.queued[id.index()] {
            self.queued[id.index()] = true;
            self.queue.push_back(id);
        }
    }

    fn enqueue_node(&mut self, key: usize) {
        if let Some(&id) = self.scope_of_node.get(&key) {
            self.enqueue(id);
        }
    }

    fn walk_scope(&mut self, id: ScopeId) {
        self.scope = id;
        self.frames.clear();
        self.collectors.clear();
        self.try_parts.clear();
        self.name_error_guard = 0;
        self.import_guard = 0;
        let locals = self.scopes[id.index()].locals.len();
        let mut st = State::new(locals);
        match self.scopes[id.index()].node {
            ScopeNode::Module(m) => self.block(&m.body, &mut st),
            ScopeNode::Class(c) => self.block(&c.body, &mut st),
            ScopeNode::Function(f) => {
                self.parameters(&f.args, &mut st);
                self.block(&f.body, &mut st);
                self.scopes[id.index()].can_fall_through = st.reachable;
            }
            ScopeNode::Lambda(l) => {
                self.parameters(&l.args, &mut st);
                self.expr(&l.body, &mut st);
            }
            ScopeNode::Generator(node) => {
                let (generators, elts): (&'a [Comprehension], Vec<&'a Expr>) = match node {
                    Expr::GeneratorExp(g) => (&g.generators, vec![&*g.elt]),
                    Expr::SetComp(g) => (&g.generators, vec![&*g.elt]),
                    Expr::DictComp(g) => (&g.generators, vec![&*g.key, &*g.value]),
                    _ => (&[], Vec::new()),
                };
                for (i, g) in generators.iter().enumerate() {
                    if i > 0 {
                        self.expr(&g.iter, &mut st);
                    }
                    self.store(
                        &g.target,
                        BindingKind::ComprehensionVar,
                        None,
                        false,
                        None,
                        &mut st,
                    );
                    for cond in &g.ifs {
                        self.expr(cond, &mut st);
                    }
                }
                for e in elts {
                    self.expr(e, &mut st);
                }
            }
        }
    }

    fn parameters(&mut self, args: &'a Arguments, st: &mut State) {
        for arg in &args.args {
            self.parameter(arg, false, st);
        }
        for name in [&args.vararg, &args.kwarg].into_iter().flatten() {
            self.bind(
                &name.id,
                name.range,
                BindingKind::Parameter,
                None,
                None,
                false,
                None,
                st,
            );
        }
    }

    fn parameter(&mut self, arg: &'a Expr, unpacked: bool, st: &mut State) {
        match arg {
            Expr::Name(n) => {
                self.bind(
                    &n.id,
                    n.range,
                    BindingKind::Parameter,
                    None,
                    None,
                    unpacked,
                    None,
                    st,
                );
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.parameter(e, true, st);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.parameter(e, true, st);
                }
            }
            other => self.expr(other, st),
        }
    }

    // ---- recording ----

    fn local_index(&self, name: &str) -> Option<usize> {
        let scope = &self.scopes[self.scope.index()];
        if self.scope != ScopeId::MODULE && scope.globals.contains_key(name) {
            return None;
        }
        scope.local_index.get(name).map(|i| *i as usize)
    }

    #[allow(clippy::too_many_arguments)]
    fn bind(
        &mut self,
        name: &'a str,
        range: TextRange,
        kind: BindingKind,
        stmt: Option<&'a Stmt>,
        alias: Option<&'a Alias>,
        unpacked: bool,
        value: Option<&'a Expr>,
        st: &mut State,
    ) -> BindingId {
        let local = self.local_index(name);
        let target_scope = if local.is_some() {
            self.scope
        } else {
            ScopeId::MODULE
        };
        let id = match self.binding_at.get(&range.start().to_u32()) {
            Some(id) => *id,
            None => {
                let id = BindingId(self.bindings.len() as u32);
                let qualified = alias.map(|a| self.qualified_import(stmt, a));
                self.bindings.push(Binding {
                    name,
                    kind,
                    range,
                    scope: target_scope,
                    site: self.scope,
                    stmt,
                    alias,
                    value,
                    qualified,
                    unpacked,
                    import_guarded: kind.is_import() && self.import_guard > 0,
                    try_parts: self.try_parts.clone(),
                    references: Vec::new(),
                });
                self.binding_at.insert(range.start().to_u32(), id);
                self.scopes[target_scope.index()]
                    .bindings
                    .entry(name)
                    .or_default()
                    .push(id);
                id
            }
        };
        if let Some(index) = local {
            st.defs[index] = vec![id.0];
            if let Some(c) = self.collectors.last_mut() {
                c.push(Event::Bind(index as u32, id.0));
            }
        }
        id
    }

    fn qualified_import(&self, stmt: Option<&'a Stmt>, alias: &Alias) -> String {
        match stmt {
            Some(Stmt::ImportFrom(s)) => {
                let mut q = ".".repeat(s.level as usize);
                if let Some(m) = &s.module {
                    q.push_str(&m.id);
                }
                if !q.is_empty() && !q.ends_with('.') {
                    q.push('.');
                }
                q.push_str(&alias.name.id);
                q
            }
            _ => {
                if alias.asname.is_some() {
                    alias.name.id.to_string()
                } else {
                    import_binding(alias).0.to_owned()
                }
            }
        }
    }

    fn reference(&mut self, name: &'a str, range: TextRange, kind: ReferenceKind, st: &State) {
        let scope_id = self.scope;
        let scope_kind = self.scopes[scope_id.index()].kind;
        let mut bindings: Vec<BindingId> = Vec::new();
        let mut unbound = false;
        let mut maybe_unbound = false;
        let resolution = match self.local_index(name) {
            Some(index) => {
                let defs = &st.defs[index];
                bindings.extend(
                    defs.iter()
                        .filter(|d| **d != UNBOUND)
                        .map(|d| BindingId(*d)),
                );
                maybe_unbound = defs.last() == Some(&UNBOUND);
                unbound = bindings.is_empty();
                if unbound && scope_kind == ScopeKind::Module {
                    // Module code falls back to builtins (and to bindings made
                    // by functions through `global`).
                    if let Some(id) = self.implicit_binding(name) {
                        bindings.push(id);
                        unbound = false;
                    } else if self.has_external_global_binding(name) {
                        unbound = false;
                    }
                } else if scope_kind == ScopeKind::Class && maybe_unbound {
                    // Class bodies fall back to globals and builtins.
                    let (resolution, mut extra) = self.lookup_nonlocal(name);
                    if !matches!(resolution, Resolution::Unresolved | Resolution::Dynamic) {
                        bindings.append(&mut extra);
                        unbound = false;
                        maybe_unbound = false;
                    }
                }
                Resolution::Local
            }
            None => {
                let (resolution, extra) = self.lookup_nonlocal(name);
                bindings = extra;
                resolution
            }
        };
        let guarded = self.name_error_guard > 0;
        match self.reference_at.get(&range.start().to_u32()) {
            Some(&id) => {
                let r = &mut self.references[id.index()];
                for b in bindings {
                    if !r.bindings.contains(&b) {
                        r.bindings.push(b);
                    }
                }
                if st.reachable {
                    if r.reachable {
                        r.unbound &= unbound;
                        r.maybe_unbound |= maybe_unbound;
                    } else {
                        r.unbound = unbound;
                        r.maybe_unbound = maybe_unbound;
                    }
                    r.reachable = true;
                }
            }
            None => {
                let id = ReferenceId(self.references.len() as u32);
                self.references.push(Reference {
                    name,
                    range,
                    kind,
                    scope: scope_id,
                    resolution,
                    bindings,
                    unbound: unbound && st.reachable,
                    maybe_unbound: maybe_unbound && st.reachable,
                    reachable: st.reachable,
                    name_error_guarded: guarded,
                });
                self.reference_at.insert(range.start().to_u32(), id);
            }
        }
    }

    /// Whether a function declares `name` global (and may bind it before
    /// module code reads it).
    fn has_external_global_binding(&self, name: &str) -> bool {
        self.scopes[1..]
            .iter()
            .any(|s| s.globals.contains_key(name))
    }

    /// Resolves a name that is not local to the current scope.
    fn lookup_nonlocal(&mut self, name: &'a str) -> (Resolution, Vec<BindingId>) {
        let scope = &self.scopes[self.scope.index()];
        let declared_global = scope.globals.contains_key(name);
        let scope_kind = scope.kind;
        if !declared_global {
            let mut at = scope.parent;
            while let Some(p) = at {
                let s = &self.scopes[p.index()];
                if p == ScopeId::MODULE {
                    break;
                }
                if s.kind != ScopeKind::Class && s.local_index.contains_key(name) {
                    let ids = s.bindings_of(name).to_vec();
                    return (Resolution::Free(p), ids);
                }
                at = s.parent;
            }
        }
        if self.scopes[0].local_index.contains_key(name) {
            // Filled in by `finish` (functions may still add bindings).
            return (Resolution::Global, Vec::new());
        }
        if name == "__module__" && scope_kind == ScopeKind::Class {
            let id = self.implicit(name, BindingKind::ClassModule);
            return (Resolution::Builtin, vec![id]);
        }
        if self.injected.contains(name) || name == "__builtins__" {
            let id = self.implicit(name, BindingKind::Injected);
            return (Resolution::Injected, vec![id]);
        }
        if iglint_stubs::is_jython_builtin(name) {
            let id = self.implicit(name, BindingKind::Builtin);
            return (Resolution::Builtin, vec![id]);
        }
        // Dynamic namespaces between here and the module may hold the name.
        let mut at = Some(self.scope);
        while let Some(p) = at {
            let s = &self.scopes[p.index()];
            if s.is_dynamic() {
                return (Resolution::Dynamic, Vec::new());
            }
            at = s.parent;
        }
        (Resolution::Unresolved, Vec::new())
    }

    fn implicit_binding(&mut self, name: &'a str) -> Option<BindingId> {
        if self.injected.contains(name) || name == "__builtins__" {
            Some(self.implicit(name, BindingKind::Injected))
        } else if iglint_stubs::is_jython_builtin(name) {
            Some(self.implicit(name, BindingKind::Builtin))
        } else {
            None
        }
    }

    fn implicit(&mut self, name: &'a str, kind: BindingKind) -> BindingId {
        if let Some(id) = self.implicit.get(name) {
            return *id;
        }
        let id = BindingId(self.bindings.len() as u32);
        self.bindings.push(Binding {
            name,
            kind,
            range: TextRange::default(),
            scope: ScopeId::MODULE,
            site: ScopeId::MODULE,
            stmt: None,
            alias: None,
            value: None,
            qualified: Some(name.to_owned()),
            unpacked: false,
            import_guarded: false,
            try_parts: Vec::new(),
            references: Vec::new(),
        });
        self.implicit.insert(name, id);
        id
    }

    fn delete(&mut self, name: &'a str, range: TextRange, st: &mut State) {
        self.reference(name, range, ReferenceKind::Del, st);
        if let Some(index) = self.local_index(name) {
            st.defs[index] = vec![UNBOUND];
            if let Some(c) = self.collectors.last_mut() {
                c.push(Event::Del(index as u32));
            }
        }
    }

    // ---- control flow ----

    fn jump(&mut self, kind: Jump, st: State) {
        for frame in self.frames.iter_mut().rev() {
            match frame {
                Frame::Loop { breaks, continues } => match kind {
                    Jump::Break => {
                        breaks.push(st);
                        return;
                    }
                    Jump::Continue => {
                        continues.push(st);
                        return;
                    }
                    Jump::Return => {}
                },
                Frame::Finally { jumps } => {
                    jumps.push((kind, st));
                    return;
                }
            }
        }
    }

    fn pop_collector(&mut self) -> Vec<Event> {
        let events = self.collectors.pop().unwrap_or_default();
        if let Some(parent) = self.collectors.last_mut() {
            parent.extend_from_slice(&events);
        }
        events
    }

    fn apply_events(st: &mut State, events: &[Event]) {
        for e in events {
            match *e {
                Event::Bind(index, id) => st.add(index as usize, id),
                Event::Del(index) => st.add(index as usize, UNBOUND),
            }
        }
    }

    fn block(&mut self, body: &'a [Stmt], st: &mut State) {
        for stmt in body {
            let key = stmt as *const Stmt as usize;
            let seen = self.stmt_reachable.entry(key).or_insert(false);
            *seen |= st.reachable;
            self.stmt(stmt, st);
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt, st: &mut State) {
        match stmt {
            Stmt::FunctionDef(f) => {
                for d in &f.decorator_list {
                    self.expr(d, st);
                }
                for d in &f.args.defaults {
                    self.expr(d, st);
                }
                self.bind(
                    &f.name.id,
                    f.name.range,
                    BindingKind::FunctionDef,
                    Some(stmt),
                    None,
                    false,
                    None,
                    st,
                );
                self.enqueue_node(f as *const StmtFunctionDef as usize);
            }
            Stmt::ClassDef(c) => {
                for d in &c.decorator_list {
                    self.expr(d, st);
                }
                for b in &c.bases {
                    self.expr(b, st);
                }
                self.enqueue_node(c as *const StmtClassDef as usize);
                self.bind(
                    &c.name.id,
                    c.name.range,
                    BindingKind::ClassDef,
                    Some(stmt),
                    None,
                    false,
                    None,
                    st,
                );
            }
            Stmt::Return(s) => {
                if let Some(v) = &s.value {
                    self.expr(v, st);
                }
                self.jump(Jump::Return, st.clone());
                st.reachable = false;
            }
            Stmt::Delete(s) => {
                for t in &s.targets {
                    self.del_target(t, st);
                }
            }
            Stmt::Assign(s) => {
                self.expr(&s.value, st);
                for t in &s.targets {
                    self.store(
                        t,
                        BindingKind::Assignment,
                        Some(stmt),
                        false,
                        Some(&s.value),
                        st,
                    );
                }
            }
            Stmt::AugAssign(s) => match &*s.target {
                Expr::Name(n) => {
                    self.reference(&n.id, n.range, ReferenceKind::AugLoad, st);
                    self.expr(&s.value, st);
                    self.bind(
                        &n.id,
                        n.range,
                        BindingKind::AugAssignment,
                        Some(stmt),
                        None,
                        false,
                        None,
                        st,
                    );
                }
                other => {
                    self.target_parts(other, st);
                    self.expr(&s.value, st);
                }
            },
            Stmt::Print(s) => {
                if let Some(d) = &s.dest {
                    self.expr(d, st);
                }
                for v in &s.values {
                    self.expr(v, st);
                }
            }
            Stmt::For(s) => {
                self.expr(&s.iter, st);
                self.for_loop(stmt, s, st);
            }
            Stmt::While(s) => self.while_loop(s, st),
            Stmt::If(s) => {
                self.expr(&s.test, st);
                let truth = constant_truth(&s.test);
                let mut body = st.clone();
                body.reachable &= truth != Some(false);
                st.reachable &= truth != Some(true);
                self.block(&s.body, &mut body);
                self.block(&s.orelse, st);
                st.join(&body);
            }
            Stmt::With(s) => {
                self.expr(&s.context_expr, st);
                if let Some(v) = &s.optional_vars {
                    self.store(v, BindingKind::WithVar, Some(stmt), false, None, st);
                }
                self.collectors.push(Vec::new());
                self.block(&s.body, st);
                let events = self.pop_collector();
                if st.reachable {
                    // A context manager may swallow an exception raised
                    // part-way through the body.
                    Self::apply_events(st, &events);
                }
            }
            Stmt::Raise(s) => {
                for e in [&s.type_, &s.inst, &s.tback].into_iter().flatten() {
                    self.expr(e, st);
                }
                st.reachable = false;
            }
            Stmt::TryExcept(s) => self.try_except(s, st),
            Stmt::TryFinally(s) => self.try_finally(s, st),
            Stmt::Assert(s) => {
                self.expr(&s.test, st);
                if let Some(m) = &s.msg {
                    let mut failing = st.clone();
                    self.expr(m, &mut failing);
                }
            }
            Stmt::Import(s) => {
                for alias in &s.names {
                    let (name, range) = import_binding(alias);
                    self.bind(
                        name,
                        range,
                        BindingKind::Import,
                        Some(stmt),
                        Some(alias),
                        false,
                        None,
                        st,
                    );
                }
            }
            Stmt::ImportFrom(s) => {
                let future =
                    s.level == 0 && s.module.as_ref().is_some_and(|m| &*m.id == "__future__");
                for alias in &s.names {
                    if &*alias.name.id == "*" {
                        continue;
                    }
                    let (name, range) = import_binding(alias);
                    let kind = if future {
                        BindingKind::FutureImport
                    } else {
                        BindingKind::FromImport
                    };
                    self.bind(name, range, kind, Some(stmt), Some(alias), false, None, st);
                }
            }
            Stmt::Exec(s) => {
                self.expr(&s.body, st);
                for e in [&s.globals, &s.locals].into_iter().flatten() {
                    self.expr(e, st);
                }
            }
            Stmt::Global(s) => {
                for name in &s.names {
                    if self.global_at.contains_key(&name.range.start().to_u32()) {
                        continue;
                    }
                    let id = BindingId(self.bindings.len() as u32);
                    self.bindings.push(Binding {
                        name: &name.id,
                        kind: BindingKind::Global,
                        range: name.range,
                        scope: self.scope,
                        site: self.scope,
                        stmt: Some(stmt),
                        alias: None,
                        value: None,
                        qualified: None,
                        unpacked: false,
                        import_guarded: false,
                        try_parts: self.try_parts.clone(),
                        references: Vec::new(),
                    });
                    self.global_at.insert(name.range.start().to_u32(), id);
                    self.scopes[self.scope.index()].global_declarations.push(id);
                }
            }
            Stmt::Expr(s) => self.expr(&s.value, st),
            Stmt::Pass(_) => {}
            Stmt::Break(_) => {
                self.jump(Jump::Break, st.clone());
                st.reachable = false;
            }
            Stmt::Continue(_) => {
                self.jump(Jump::Continue, st.clone());
                st.reachable = false;
            }
        }
    }

    fn for_loop(&mut self, stmt: &'a Stmt, s: &'a StmtFor, st: &mut State) {
        let entry = st.clone();
        let mut head = entry.clone();
        let (breaks, exit) = loop {
            self.frames.push(Frame::Loop {
                breaks: Vec::new(),
                continues: Vec::new(),
            });
            let mut body = head.clone();
            self.store(
                &s.target,
                BindingKind::LoopVar,
                Some(stmt),
                false,
                None,
                &mut body,
            );
            self.block(&s.body, &mut body);
            let Some(Frame::Loop { breaks, continues }) = self.frames.pop() else {
                unreachable!("loop frame");
            };
            let mut next = entry.clone();
            next.join(&body);
            for c in &continues {
                next.join(c);
            }
            if next == head {
                break (breaks, head);
            }
            head = next;
        };
        let mut after = exit;
        self.block(&s.orelse, &mut after);
        for b in &breaks {
            after.join(b);
        }
        *st = after;
    }

    fn while_loop(&mut self, s: &'a StmtWhile, st: &mut State) {
        let truth = constant_truth(&s.test);
        let entry = st.clone();
        let mut head = entry.clone();
        let (breaks, exit) = loop {
            self.frames.push(Frame::Loop {
                breaks: Vec::new(),
                continues: Vec::new(),
            });
            let mut test_state = head.clone();
            self.expr(&s.test, &mut test_state);
            let mut body = test_state.clone();
            body.reachable &= truth != Some(false);
            self.block(&s.body, &mut body);
            let Some(Frame::Loop { breaks, continues }) = self.frames.pop() else {
                unreachable!("loop frame");
            };
            let mut next = entry.clone();
            next.join(&body);
            for c in &continues {
                next.join(c);
            }
            if next == head {
                let mut exit = test_state;
                exit.reachable &= truth != Some(true);
                break (breaks, exit);
            }
            head = next;
        };
        let mut after = exit;
        self.block(&s.orelse, &mut after);
        for b in &breaks {
            after.join(b);
        }
        *st = after;
    }

    fn try_except(&mut self, s: &'a StmtTryExcept, st: &mut State) {
        let pre = st.clone();
        let guards_names = s
            .handlers
            .iter()
            .any(|h| h.type_.is_some() && catches(h, &["NameError", "UnboundLocalError"]));
        let guards_imports = s.handlers.iter().any(|h| {
            catches(
                h,
                &[
                    "ImportError",
                    "Exception",
                    "BaseException",
                    "StandardError",
                    "Throwable",
                ],
            )
        });
        self.collectors.push(Vec::new());
        self.try_parts.push((s.range, TryPart::Body));
        self.name_error_guard += u32::from(guards_names);
        self.import_guard += u32::from(guards_imports);
        self.block(&s.body, st);
        self.name_error_guard -= u32::from(guards_names);
        self.import_guard -= u32::from(guards_imports);
        self.try_parts.pop();
        let events = self.pop_collector();
        let mut entry = pre;
        Self::apply_events(&mut entry, &events);
        let mut handled = State::unreachable(st.defs.len());
        self.try_parts.push((s.range, TryPart::Handler));
        for h in &s.handlers {
            let mut hs = entry.clone();
            if let Some(t) = &h.type_ {
                self.expr(t, &mut hs);
            }
            if let Some(n) = &h.name {
                self.store_handler_name(n, &mut hs);
            }
            self.block(&h.body, &mut hs);
            handled.join(&hs);
        }
        self.try_parts.pop();
        self.try_parts.push((s.range, TryPart::Else));
        self.block(&s.orelse, st);
        self.try_parts.pop();
        st.join(&handled);
    }

    fn store_handler_name(&mut self, target: &'a Expr, st: &mut State) {
        match target {
            Expr::Name(n) => {
                self.bind(
                    &n.id,
                    n.range,
                    BindingKind::ExceptVar,
                    None,
                    None,
                    false,
                    None,
                    st,
                );
            }
            other => self.store(other, BindingKind::ExceptVar, None, false, None, st),
        }
    }

    fn try_finally(&mut self, s: &'a StmtTryFinally, st: &mut State) {
        let pre = st.clone();
        self.frames.push(Frame::Finally { jumps: Vec::new() });
        self.collectors.push(Vec::new());
        self.try_parts.push((s.range, TryPart::Body));
        self.block(&s.body, st);
        self.try_parts.pop();
        let events = self.pop_collector();
        let Some(Frame::Finally { jumps }) = self.frames.pop() else {
            unreachable!("finally frame");
        };
        let mut other = pre;
        Self::apply_events(&mut other, &events);
        for (_, js) in &jumps {
            other.join(js);
        }
        self.try_parts.push((s.range, TryPart::Finally));
        // Normal completion.
        self.block(&s.finalbody, st);
        // Exceptions and jumps through the clause.
        self.block(&s.finalbody, &mut other);
        self.try_parts.pop();
        if other.reachable {
            for kind in [Jump::Break, Jump::Continue, Jump::Return] {
                if jumps.iter().any(|(k, _)| *k == kind) {
                    self.jump(kind, other.clone());
                }
            }
        }
    }

    /// An assignment target.
    fn store(
        &mut self,
        target: &'a Expr,
        kind: BindingKind,
        stmt: Option<&'a Stmt>,
        unpacked: bool,
        value: Option<&'a Expr>,
        st: &mut State,
    ) {
        match target {
            Expr::Name(n) => {
                self.bind(
                    &n.id,
                    n.range,
                    kind,
                    stmt,
                    None,
                    unpacked,
                    if unpacked { None } else { value },
                    st,
                );
            }
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.store(e, kind, stmt, true, None, st);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.store(e, kind, stmt, true, None, st);
                }
            }
            other => self.target_parts(other, st),
        }
    }

    /// The loads inside a non-name target (`a.b`, `a[i]`, invalid targets).
    fn target_parts(&mut self, target: &'a Expr, st: &mut State) {
        match target {
            Expr::Attribute(a) => self.expr(&a.value, st),
            Expr::Subscript(s) => {
                self.expr(&s.value, st);
                self.slice(&s.slice, st);
            }
            Expr::Name(n) => self.reference(&n.id, n.range, ReferenceKind::Load, st),
            other => self.expr(other, st),
        }
    }

    fn del_target(&mut self, target: &'a Expr, st: &mut State) {
        match target {
            Expr::Name(n) => self.delete(&n.id, n.range, st),
            Expr::Tuple(t) => {
                for e in &t.elts {
                    self.del_target(e, st);
                }
            }
            Expr::List(t) => {
                for e in &t.elts {
                    self.del_target(e, st);
                }
            }
            other => self.target_parts(other, st),
        }
    }

    fn slice(&mut self, slice: &'a Slice, st: &mut State) {
        match slice {
            Slice::Ellipsis(_) => {}
            Slice::Slice(s) => {
                for e in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    self.expr(e, st);
                }
            }
            Slice::ExtSlice(s) => {
                for d in &s.dims {
                    self.slice(d, st);
                }
            }
            Slice::Index(s) => self.expr(&s.value, st),
        }
    }

    fn expr(&mut self, expr: &'a Expr, st: &mut State) {
        match expr {
            Expr::Name(n) => match n.ctx {
                ExprContext::Store | ExprContext::AugStore | ExprContext::Param => {
                    self.bind(
                        &n.id,
                        n.range,
                        BindingKind::Assignment,
                        None,
                        None,
                        false,
                        None,
                        st,
                    );
                }
                ExprContext::Del => self.delete(&n.id, n.range, st),
                ExprContext::Load | ExprContext::AugLoad => {
                    self.reference(&n.id, n.range, ReferenceKind::Load, st);
                }
            },
            Expr::BoolOp(e) => {
                let mut values = e.values.iter();
                if let Some(first) = values.next() {
                    self.expr(first, st);
                }
                for v in values {
                    let mut taken = st.clone();
                    self.expr(v, &mut taken);
                    st.join(&taken);
                }
            }
            Expr::BinOp(e) => {
                self.expr(&e.left, st);
                self.expr(&e.right, st);
            }
            Expr::UnaryOp(e) => self.expr(&e.operand, st),
            Expr::Lambda(e) => {
                for d in &e.args.defaults {
                    self.expr(d, st);
                }
                self.enqueue_node(e as *const ExprLambda as usize);
            }
            Expr::IfExp(e) => {
                self.expr(&e.test, st);
                let mut body = st.clone();
                self.expr(&e.body, &mut body);
                self.expr(&e.orelse, st);
                st.join(&body);
            }
            Expr::Dict(e) => {
                for (k, v) in e.keys.iter().zip(&e.values) {
                    self.expr(v, st);
                    self.expr(k, st);
                }
            }
            Expr::Set(e) => {
                for v in &e.elts {
                    self.expr(v, st);
                }
            }
            Expr::ListComp(e) => {
                let Some(first) = e.generators.first() else {
                    return;
                };
                self.expr(&first.iter, st);
                let mut inner = st.clone();
                for (i, g) in e.generators.iter().enumerate() {
                    if i > 0 {
                        self.expr(&g.iter, &mut inner);
                    }
                    self.store(
                        &g.target,
                        BindingKind::ComprehensionVar,
                        None,
                        false,
                        None,
                        &mut inner,
                    );
                    for cond in &g.ifs {
                        self.expr(cond, &mut inner);
                    }
                }
                self.expr(&e.elt, &mut inner);
                st.join(&inner);
            }
            Expr::SetComp(e) => self.generator(expr, &e.generators, st),
            Expr::DictComp(e) => self.generator(expr, &e.generators, st),
            Expr::GeneratorExp(e) => self.generator(expr, &e.generators, st),
            Expr::Yield(e) => {
                if let Some(v) = &e.value {
                    self.expr(v, st);
                }
            }
            Expr::Compare(e) => {
                self.expr(&e.left, st);
                for c in &e.comparators {
                    self.expr(c, st);
                }
            }
            Expr::Call(e) => {
                self.expr(&e.func, st);
                for a in &e.args {
                    self.expr(a, st);
                }
                for k in &e.keywords {
                    self.expr(&k.value, st);
                }
                for a in [&e.starargs, &e.kwargs].into_iter().flatten() {
                    self.expr(a, st);
                }
            }
            Expr::Repr(e) => self.expr(&e.value, st),
            Expr::Num(_) | Expr::Str(_) => {}
            Expr::Attribute(e) => self.expr(&e.value, st),
            Expr::Subscript(e) => {
                self.expr(&e.value, st);
                self.slice(&e.slice, st);
            }
            Expr::List(e) => {
                if matches!(e.ctx, ExprContext::Store | ExprContext::Del) {
                    // Only reached through invalid targets; handled by callers.
                    for v in &e.elts {
                        self.expr(v, st);
                    }
                } else {
                    for v in &e.elts {
                        self.expr(v, st);
                    }
                }
            }
            Expr::Tuple(e) => {
                for v in &e.elts {
                    self.expr(v, st);
                }
            }
        }
    }

    fn generator(&mut self, node: &'a Expr, generators: &'a [Comprehension], st: &mut State) {
        if let Some(first) = generators.first() {
            self.expr(&first.iter, st);
        }
        self.enqueue_node(node as *const Expr as usize);
    }

    // ---- results ----

    fn finish(mut self) -> Output<'a> {
        // Module names: references resolve to every module binding,
        // including those made by functions through `global`.
        for r in &mut self.references {
            match r.resolution {
                Resolution::Global => {
                    r.bindings = self.scopes[0].bindings_of(r.name).to_vec();
                }
                Resolution::Local if r.scope == ScopeId::MODULE => {
                    for id in self.scopes[0].bindings_of(r.name) {
                        if self.bindings[id.index()].site != ScopeId::MODULE
                            && !r.bindings.contains(id)
                        {
                            r.bindings.push(*id);
                        }
                    }
                }
                _ => {}
            }
        }
        for (i, r) in self.references.iter().enumerate() {
            for b in &r.bindings {
                self.bindings[b.index()]
                    .references
                    .push(ReferenceId(i as u32));
            }
        }
        for scope in &mut self.scopes {
            for ids in scope.bindings.values_mut() {
                ids.sort_by_key(|id| self.bindings[id.index()].range.start());
            }
        }
        let mut unreachable = Vec::new();
        collect_unreachable(&self.module.body, &self.stmt_reachable, &mut unreachable);
        Output {
            scopes: self.scopes,
            bindings: self.bindings,
            references: self.references,
            scope_of_node: self.scope_of_node,
            implicit: self.implicit,
            unreachable,
            reference_at: self.reference_at,
            dunder_all: self.dunder_all,
            dunder_all_dynamic: self.dunder_all_dynamic,
        }
    }
}

/// The truth value of a constant test (`True`, `1`, `'x'`, `False`, `0`,
/// `None`, `''`).
pub(crate) fn constant_truth(test: &Expr) -> Option<bool> {
    match test {
        Expr::Name(n) => match &*n.id {
            "True" => Some(true),
            "False" | "None" => Some(false),
            _ => None,
        },
        Expr::Num(n) => Some(match &n.n {
            Number::Int(i) => *i != 0,
            Number::Long(l) => !l.trim_start_matches('-').trim_start_matches('0').is_empty(),
            Number::Float(f) | Number::Complex(f) => *f != 0.0,
        }),
        Expr::Str(s) => Some(match &s.s {
            StringValue::Bytes(b) => !b.is_empty(),
            StringValue::Unicode(u) => !u.is_empty(),
        }),
        _ => None,
    }
}

fn collect_unreachable<'a>(
    body: &'a [Stmt],
    reachable: &FxHashMap<usize, bool>,
    out: &mut Vec<(&'a Stmt, &'a Stmt)>,
) {
    let is_reachable = |s: &Stmt| {
        reachable
            .get(&(s as *const Stmt as usize))
            .copied()
            .unwrap_or(true)
    };
    for (i, stmt) in body.iter().enumerate() {
        if i > 0 && !is_reachable(stmt) && is_reachable(&body[i - 1]) {
            out.push((stmt, &body[i - 1]));
        }
        match stmt {
            Stmt::FunctionDef(f) => collect_unreachable(&f.body, reachable, out),
            Stmt::ClassDef(c) => collect_unreachable(&c.body, reachable, out),
            Stmt::For(s) => {
                collect_unreachable(&s.body, reachable, out);
                collect_unreachable(&s.orelse, reachable, out);
            }
            Stmt::While(s) => {
                collect_unreachable(&s.body, reachable, out);
                collect_unreachable(&s.orelse, reachable, out);
            }
            Stmt::If(s) => {
                collect_unreachable(&s.body, reachable, out);
                collect_unreachable(&s.orelse, reachable, out);
            }
            Stmt::With(s) => collect_unreachable(&s.body, reachable, out),
            Stmt::TryExcept(s) => {
                collect_unreachable(&s.body, reachable, out);
                for h in &s.handlers {
                    collect_unreachable(&h.body, reachable, out);
                }
                collect_unreachable(&s.orelse, reachable, out);
            }
            Stmt::TryFinally(s) => {
                collect_unreachable(&s.body, reachable, out);
                collect_unreachable(&s.finalbody, reachable, out);
            }
            _ => {}
        }
    }
}
