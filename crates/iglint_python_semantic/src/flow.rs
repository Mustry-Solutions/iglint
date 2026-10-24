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
use iglint_text::{Ranged, TextRange};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::*;
use crate::symbols::{PACKAGE, PackageImports, SymbolTable, catches, import_binding, jython};

const UNBOUND: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    reachable: bool,
    /// Per local index: sorted binding ids, `UNBOUND` last.
    defs: Defs,
}

impl State {
    fn new(locals: usize) -> State {
        State {
            reachable: true,
            defs: Defs::new(locals, &[UNBOUND]),
        }
    }

    fn unreachable(locals: usize) -> State {
        State {
            reachable: false,
            defs: Defs::new(locals, &[]),
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
        self.defs.join(&other.defs);
    }

    fn add(&mut self, index: usize, id: u32) {
        let defs = &mut self.defs[index];
        if let Err(at) = defs.binary_search(&id) {
            defs.insert(at, id);
        }
    }
}

/// The ids of every local of a scope, in a persistent tree of shared
/// nodes ([`FANOUT`] locals or children per node): copying a state (every
/// branch, loop and `try` does) copies one pointer, a change copies the
/// nodes on one path, and a join or comparison skips the nodes both states
/// share (review round 25, M3: copying every local made a module of `n`
/// names and `n` branches or loops quadratic).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Defs {
    root: Rc<Node>,
    len: usize,
    /// Levels of branches above the leaves.
    depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Leaf(Vec<Vec<u32>>),
    Branch(Vec<Rc<Node>>),
}

/// Locals per leaf and children per branch of [`Defs`].
const FANOUT: usize = 32;

use std::rc::Rc;

impl Defs {
    fn new(len: usize, ids: &[u32]) -> Defs {
        let mut depth = 0;
        let mut capacity = FANOUT;
        while capacity < len {
            capacity *= FANOUT;
            depth += 1;
        }
        // Full nodes of each level are all the same: built once, shared
        // until written.
        let mut full: Vec<Rc<Node>> = vec![Rc::new(Node::Leaf(vec![ids.to_vec(); FANOUT]))];
        for _ in 0..depth {
            let below = Rc::clone(full.last().expect("a level"));
            full.push(Rc::new(Node::Branch(vec![below; FANOUT])));
        }
        fn build(len: usize, depth: u32, ids: &[u32], full: &[Rc<Node>]) -> Rc<Node> {
            let span = FANOUT.pow(depth);
            if len == span * FANOUT {
                return Rc::clone(&full[depth as usize]);
            }
            if depth == 0 {
                return Rc::new(Node::Leaf(vec![ids.to_vec(); len]));
            }
            let children = (0..len.div_ceil(span))
                .map(|c| build(span.min(len - c * span), depth - 1, ids, full))
                .collect();
            Rc::new(Node::Branch(children))
        }
        Defs {
            root: build(len, depth, ids, &full),
            len,
            depth,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    /// Adds the ids of `other` (as [`union`] does), skipping the nodes the
    /// two share.
    fn join(&mut self, other: &Defs) {
        fn join(mine: &mut Rc<Node>, theirs: &Rc<Node>) {
            if Rc::ptr_eq(mine, theirs) || **mine == **theirs {
                return;
            }
            match (Rc::make_mut(mine), &**theirs) {
                (Node::Leaf(m), Node::Leaf(t)) => {
                    for (m, t) in m.iter_mut().zip(t) {
                        union(m, t);
                    }
                }
                (Node::Branch(m), Node::Branch(t)) => {
                    for (m, t) in m.iter_mut().zip(t) {
                        join(m, t);
                    }
                }
                _ => unreachable!("states of one scope have the same shape"),
            }
        }
        join(&mut self.root, &other.root);
    }
}

impl std::ops::Index<usize> for Defs {
    type Output = Vec<u32>;

    fn index(&self, index: usize) -> &Vec<u32> {
        let mut node = &*self.root;
        let mut span = FANOUT.pow(self.depth);
        let mut at = index;
        loop {
            match node {
                Node::Leaf(ids) => return &ids[at],
                Node::Branch(children) => {
                    node = &children[at / span];
                    at %= span;
                    span /= FANOUT;
                }
            }
        }
    }
}

impl std::ops::IndexMut<usize> for Defs {
    fn index_mut(&mut self, index: usize) -> &mut Vec<u32> {
        let mut node = Rc::make_mut(&mut self.root);
        let mut span = FANOUT.pow(self.depth);
        let mut at = index;
        loop {
            match node {
                Node::Leaf(ids) => return &mut ids[at],
                Node::Branch(children) => {
                    node = Rc::make_mut(&mut children[at / span]);
                    at %= span;
                    span /= FANOUT;
                }
            }
        }
    }
}

/// Adds the sorted ids of `from` to the sorted ids of `into`, by merging
/// (review round 25, M3: inserting one at a time made a join cost the
/// square of the set's size, and a chain of `elif` branches binding the
/// same name the cube of its length).
fn union(into: &mut Vec<u32>, from: &[u32]) {
    if into.as_slice() == from || from.is_empty() {
        return;
    }
    if from.len() == 1 {
        if let Err(at) = into.binary_search(&from[0]) {
            into.insert(at, from[0]);
        }
        return;
    }
    let mut out = Vec::with_capacity(into.len() + from.len());
    let (mut i, mut j) = (0, 0);
    while i < into.len() && j < from.len() {
        match into[i].cmp(&from[j]) {
            std::cmp::Ordering::Less => {
                out.push(into[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(from[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                out.push(into[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out.extend_from_slice(&into[i..]);
    out.extend_from_slice(&from[j..]);
    *into = out;
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

/// What a name occurrence does, for [`Flow::access`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Load,
    Store,
    Del,
}

/// Where a reference is recorded (re-walking a loop finds it again).
#[derive(Clone, Copy)]
enum Slot<'a> {
    /// A `Name` node, by its start.
    Name,
    /// An implicit read, by its start and name.
    Implicit(&'a str),
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
    unknown_globals: bool,
    implicit: FxHashMap<&'a str, BindingId>,
    binding_at: FxHashMap<u32, BindingId>,
    global_at: FxHashMap<u32, BindingId>,
    reference_at: FxHashMap<u32, ReferenceId>,
    implicit_at: FxHashMap<(u32, &'a str), ReferenceId>,
    /// Whether each statement (by address) was ever reached.
    stmt_reachable: FxHashMap<usize, bool>,
    queued: Vec<bool>,
    queue: VecDeque<ScopeId>,
    // Per walked scope.
    scope: ScopeId,
    frames: Vec<Frame>,
    collectors: Vec<Vec<Event>>,
    /// Per list comprehension being walked, the bindings of the state it
    /// replaced (local index and the previous ids), so that the
    /// comprehension runs on the state itself and its bindings are joined
    /// with the state before it afterwards, instead of on a copy of the
    /// whole state (review round 25, M3: a copy per comprehension made a
    /// module of `n` names and `n` comprehensions quadratic).
    undo: Vec<Vec<(usize, Vec<u32>)>>,
    try_parts: Vec<(TextRange, TryPart)>,
    name_error_guard: u32,
    import_guard: u32,
    dunder_all: Vec<String>,
    dunder_all_dynamic: bool,
    package: PackageImports,
    package_bound_elsewhere: bool,
}

impl<'a, 'o> Flow<'a, 'o> {
    pub fn new(
        module: &'a Module,
        table: SymbolTable<'a>,
        injected: &'o FxHashSet<&'o str>,
        unknown_globals: bool,
    ) -> Self {
        let n = table.scopes.len();
        Flow {
            module,
            scopes: table.scopes,
            bindings: Vec::new(),
            references: Vec::new(),
            scope_of_node: table.scope_of_node,
            injected,
            unknown_globals,
            implicit: FxHashMap::default(),
            binding_at: FxHashMap::default(),
            global_at: FxHashMap::default(),
            reference_at: FxHashMap::default(),
            implicit_at: FxHashMap::default(),
            stmt_reachable: FxHashMap::default(),
            queued: vec![false; n],
            queue: VecDeque::new(),
            scope: ScopeId::MODULE,
            frames: Vec::new(),
            collectors: Vec::new(),
            undo: Vec::new(),
            try_parts: Vec::new(),
            name_error_guard: 0,
            import_guard: 0,
            dunder_all: table.dunder_all,
            dunder_all_dynamic: table.dunder_all_dynamic,
            package: table.package,
            package_bound_elsewhere: table.package_bound_elsewhere,
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
            ScopeNode::Class(c) => {
                self.implicit_read("__name__", c.name.range, ImplicitRead::ClassModule, &st);
                self.block(&c.body, &mut st);
                if c.bases.is_empty() {
                    self.implicit_read("__metaclass__", c.name.range, ImplicitRead::Metaclass, &st);
                }
            }
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

    /// The run-time spelling of a name of the current scope's code.
    fn key(&self, name: &'a str) -> &'a str {
        self.scopes[self.scope.index()].runtime_name(name)
    }

    /// How the current scope's code reaches `raw` (spelled in the source;
    /// `key` is its run-time spelling): the accessor Jython's code
    /// generator picks (`CodeCompiler.visitName`).
    fn access(&self, raw: &str, key: &str, op: Op) -> NameAccess {
        use jython::*;
        let scope = &self.scopes[self.scope.index()];
        match scope.kind {
            ScopeKind::Module => {
                let flags = scope.jython.get(raw).copied().unwrap_or(0);
                if flags & GLOBAL != 0 {
                    NameAccess::Global
                } else {
                    NameAccess::Name
                }
            }
            ScopeKind::Class => match scope.jython.get(key).copied() {
                Some(f) if f & GLOBAL != 0 => NameAccess::Global,
                Some(f) if op == Op::Load && f & FREE != 0 && f & BOUND == 0 => NameAccess::Free,
                _ => NameAccess::Name,
            },
            _ => {
                let Some(f) = scope.jython.get(raw).copied() else {
                    return NameAccess::Name;
                };
                if f & GLOBAL != 0 {
                    return NameAccess::Global;
                }
                match op {
                    Op::Load => {
                        if !scope.has_exec && f & (BOUND | CELL | FREE) == 0 {
                            NameAccess::ImplicitGlobal
                        } else if f & CELL != 0 {
                            NameAccess::Cell
                        } else if f & BOUND != 0 {
                            NameAccess::Fast
                        } else if f & FREE != 0 {
                            NameAccess::Free
                        } else {
                            NameAccess::Name
                        }
                    }
                    Op::Store if f & CELL != 0 => NameAccess::Cell,
                    Op::Store | Op::Del => NameAccess::Fast,
                }
            }
        }
    }

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
        let raw = name;
        let name = self.key(raw);
        let access = self.access(raw, name, Op::Store);
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
                    source_name: raw,
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
                    shadowed: Vec::new(),
                    references: Vec::new(),
                    access,
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
            if st.reachable && kind.records_shadowed() {
                let shadowed = &mut self.bindings[id.index()].shadowed;
                let shadows = st.defs[index]
                    .iter()
                    .filter(|d| **d != UNBOUND && **d != id.0)
                    .map(|d| BindingId(*d));
                if shadowed.is_empty() {
                    // The ids of a state are a sorted set: no duplicates.
                    shadowed.extend(shadows);
                } else {
                    // Walked again (a loop): only the new ones, in order.
                    let known: FxHashSet<BindingId> = shadowed.iter().copied().collect();
                    let new: Vec<BindingId> = shadows.filter(|d| !known.contains(d)).collect();
                    shadowed.extend(new);
                }
            }
            let old = std::mem::replace(&mut st.defs[index], vec![id.0]);
            if let Some(log) = self.undo.last_mut() {
                log.push((index, old));
            }
            if let Some(c) = self.collectors.last_mut() {
                c.push(Event::Bind(index as u32, id.0));
            }
        }
        id
    }

    /// The `__package__` binding an import statement makes in the module
    /// namespace when it runs (before it looks the module up, so a failing
    /// import makes it too).
    fn bind_package(&mut self, stmt: &'a Stmt, st: &mut State) {
        if !self.package.binds(stmt) {
            return;
        }
        let start = stmt.range().start();
        let keyword = if matches!(stmt, Stmt::Import(_)) {
            "import"
        } else {
            "from"
        };
        let range = TextRange::at(start, iglint_text::TextSize::of(keyword));
        if self.scope == ScopeId::MODULE {
            self.bind(
                PACKAGE,
                range,
                BindingKind::ImportPackage,
                Some(stmt),
                None,
                false,
                None,
                st,
            );
            return;
        }
        // Elsewhere the import writes the module's globals, whatever the
        // scope binds under the same name.
        if self.binding_at.contains_key(&start.to_u32()) {
            return;
        }
        let id = BindingId(self.bindings.len() as u32);
        self.bindings.push(Binding {
            name: PACKAGE,
            source_name: PACKAGE,
            kind: BindingKind::ImportPackage,
            range,
            scope: ScopeId::MODULE,
            site: self.scope,
            stmt: Some(stmt),
            alias: None,
            value: None,
            qualified: None,
            unpacked: false,
            import_guarded: false,
            try_parts: self.try_parts.clone(),
            shadowed: Vec::new(),
            references: Vec::new(),
            access: NameAccess::Name,
        });
        self.binding_at.insert(start.to_u32(), id);
        self.scopes[0].bindings.entry(PACKAGE).or_default().push(id);
    }

    /// The names an import statement reads from the module's globals to
    /// find the importing package (measured: `__package__`, `__name__`,
    /// `__path__`, whatever the scope).
    fn import_reads(&mut self, stmt: &'a Stmt, st: &State) {
        let start = stmt.range().start();
        let keyword = if matches!(stmt, Stmt::Import(_)) {
            "import"
        } else {
            "from"
        };
        let range = TextRange::at(start, iglint_text::TextSize::of(keyword));
        for name in ["__package__", "__name__", "__path__"] {
            self.implicit_read(name, range, ImplicitRead::Import, st);
        }
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

    fn reference(&mut self, raw: &'a str, range: TextRange, kind: ReferenceKind, st: &State) {
        let name = self.key(raw);
        let op = if kind == ReferenceKind::Del {
            Op::Del
        } else {
            Op::Load
        };
        let access = self.access(raw, name, op);
        self.record_reference(name, raw, range, kind, access, Slot::Name, st);
    }

    /// A read the interpreter makes in the current scope without the code
    /// naming it (see [`ImplicitRead`]).
    fn implicit_read(&mut self, name: &'a str, range: TextRange, read: ImplicitRead, st: &State) {
        let kind = ReferenceKind::Implicit(read);
        match read {
            // Looked up by name in the class body, like a read there.
            ImplicitRead::ClassModule | ImplicitRead::Metaclass => {
                self.record_reference(
                    name,
                    name,
                    range,
                    kind,
                    NameAccess::Name,
                    Slot::Implicit(name),
                    st,
                );
            }
            // Read from the module's globals (and the builtins), whatever
            // the scope binds.
            ImplicitRead::FunctionModule | ImplicitRead::Import | ImplicitRead::Assert => {
                self.record_reference(
                    name,
                    name,
                    range,
                    kind,
                    NameAccess::ImplicitGlobal,
                    Slot::Implicit(name),
                    st,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_reference(
        &mut self,
        name: &'a str,
        source_name: &'a str,
        range: TextRange,
        kind: ReferenceKind,
        access: NameAccess,
        slot: Slot<'a>,
        st: &State,
    ) {
        let scope_id = self.scope;
        let scope_kind = self.scopes[scope_id.index()].kind;
        let mut bindings: Vec<BindingId> = Vec::new();
        let mut unbound = false;
        let mut maybe_unbound = false;
        let mut fallback = None;
        let global_only = matches!(access, NameAccess::Global | NameAccess::ImplicitGlobal);
        let local = if global_only && scope_kind != ScopeKind::Module {
            None
        } else {
            self.local_index(name)
        };
        let resolution = match local {
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
                    } else if self.has_external_global_binding(name)
                        || (name == PACKAGE && self.package_bound_elsewhere)
                    {
                        unbound = false;
                    }
                } else if scope_kind == ScopeKind::Class {
                    // A class body reads by name: the class namespace, then
                    // the module's globals and the builtins. Flow-insensitive
                    // across the two: the read may find either.
                    let (resolution, mut extra) = self.lookup_global(name);
                    fallback = Some(resolution);
                    bindings.append(&mut extra);
                    if !matches!(resolution, Resolution::Unresolved | Resolution::Dynamic) {
                        unbound = false;
                        maybe_unbound = false;
                    }
                }
                Resolution::Local
            }
            None if access == NameAccess::Free => {
                let (resolution, extra) = self.lookup_free(name);
                bindings = extra;
                resolution
            }
            None => {
                let (resolution, extra) = self.lookup_global(name);
                bindings = extra;
                resolution
            }
        };
        let guarded = self.name_error_guard > 0;
        let existing = match slot {
            Slot::Name => self.reference_at.get(&range.start().to_u32()),
            Slot::Implicit(n) => self.implicit_at.get(&(range.start().to_u32(), n)),
        };
        match existing {
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
                    source_name,
                    range,
                    kind,
                    scope: scope_id,
                    resolution,
                    bindings,
                    unbound: unbound && st.reachable,
                    maybe_unbound: maybe_unbound && st.reachable,
                    reachable: st.reachable,
                    name_error_guarded: guarded,
                    access,
                    fallback,
                });
                match slot {
                    Slot::Name => {
                        self.reference_at.insert(range.start().to_u32(), id);
                    }
                    Slot::Implicit(n) => {
                        self.implicit_at.insert((range.start().to_u32(), n), id);
                    }
                }
            }
        }
    }

    /// The truth value of a constant test ([`constant_truth`]). `True`,
    /// `False` and `None` are names in Jython 2.7 that a module can rebind
    /// (`True = 0`, `import x as None`, `globals()['None'] = 1`, verified
    /// on Ignition 8.3.3), so they count only when no scope binds or
    /// declares the name global, no scope is dynamic, and the caller
    /// neither injects the name nor leaves the globals unknown.
    fn constant_truth(&self, test: &Expr) -> Option<bool> {
        if let Expr::Name(n) = test {
            let name: &str = &n.id;
            let rebindable = self.unknown_globals
                || self.injected.contains(name)
                || self.scopes.iter().any(|s| {
                    s.local_index.contains_key(name)
                        || s.globals.contains_key(name)
                        || s.is_dynamic()
                });
            if rebindable {
                return None;
            }
        }
        constant_truth(test)
    }

    /// Whether a function declares `name` global (and may bind it before
    /// module code reads it).
    fn has_external_global_binding(&self, name: &str) -> bool {
        self.scopes[1..]
            .iter()
            .any(|s| s.globals.contains_key(name))
    }

    /// Resolves a free variable: the enclosing function whose cell it is.
    /// Jython's `setup_closure` keeps a name free only while each enclosing
    /// function has it free or as a cell (class bodies are skipped).
    fn lookup_free(&mut self, name: &'a str) -> (Resolution, Vec<BindingId>) {
        let mut at = self.scope;
        while at != ScopeId::MODULE {
            at = crate::symbols::enclosing_function(&self.scopes, at.index());
            let s = &self.scopes[at.index()];
            let flags = s.jython.get(name).copied().unwrap_or(0);
            if flags & jython::CELL != 0 {
                return (Resolution::Free(at), s.bindings_of(name).to_vec());
            }
            if flags & jython::FREE == 0 {
                break;
            }
        }
        self.lookup_global(name)
    }

    /// Resolves a name in the module's globals, then the caller's injected
    /// names, then the builtins.
    fn lookup_global(&mut self, name: &'a str) -> (Resolution, Vec<BindingId>) {
        let scope_kind = self.scopes[self.scope.index()].kind;
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
        if self.unknown_globals {
            return (Resolution::Dynamic, Vec::new());
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
            source_name: name,
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
            shadowed: Vec::new(),
            references: Vec::new(),
            access: NameAccess::Name,
        });
        self.implicit.insert(name, id);
        id
    }

    fn delete(&mut self, raw: &'a str, range: TextRange, st: &mut State) {
        self.reference(raw, range, ReferenceKind::Del, st);
        let name = self.key(raw);
        if let Some(index) = self.local_index(name) {
            let old = std::mem::replace(&mut st.defs[index], vec![UNBOUND]);
            if let Some(log) = self.undo.last_mut() {
                log.push((index, old));
            }
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
                self.implicit_read("__name__", f.name.range, ImplicitRead::FunctionModule, st);
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
                let truth = self.constant_truth(&s.test);
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
                let keyword = TextRange::at(s.range.start(), iglint_text::TextSize::of("assert"));
                self.implicit_read("__debug__", keyword, ImplicitRead::Assert, st);
                self.expr(&s.test, st);
                let mut failing = st.clone();
                if let Some(m) = &s.msg {
                    self.expr(m, &mut failing);
                }
                self.implicit_read("AssertionError", keyword, ImplicitRead::Assert, &failing);
            }
            Stmt::Import(s) => {
                self.import_reads(stmt, st);
                self.bind_package(stmt, st);
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
                self.import_reads(stmt, st);
                self.bind_package(stmt, st);
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
                        source_name: &name.id,
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
                        shadowed: Vec::new(),
                        references: Vec::new(),
                        access: NameAccess::Global,
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
        let truth = self.constant_truth(&s.test);
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
                let keyword = TextRange::at(e.range.start(), iglint_text::TextSize::of("lambda"));
                self.implicit_read("__name__", keyword, ImplicitRead::FunctionModule, st);
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
                // The comprehension may not run: what it binds joins the
                // state before it. It runs on `st` itself, and the
                // bindings it replaces are logged in `undo`.
                self.undo.push(Vec::new());
                for (i, g) in e.generators.iter().enumerate() {
                    if i > 0 {
                        self.expr(&g.iter, st);
                    }
                    self.store(
                        &g.target,
                        BindingKind::ComprehensionVar,
                        None,
                        false,
                        None,
                        st,
                    );
                    for cond in &g.ifs {
                        self.expr(cond, st);
                    }
                }
                self.expr(&e.elt, st);
                let log = self.undo.pop().unwrap_or_default();
                let mut done: FxHashSet<usize> = FxHashSet::default();
                for (index, old) in log {
                    // The first entry of a local holds its ids before the
                    // comprehension.
                    if !done.insert(index) {
                        continue;
                    }
                    if st.reachable {
                        // `State::join` of the state before and after.
                        let mut joined = old.clone();
                        union(&mut joined, &st.defs[index]);
                        st.defs[index] = joined;
                    } else {
                        // Joining an unreachable state changes nothing.
                        st.defs[index] = old.clone();
                    }
                    // An enclosing comprehension sees the change too.
                    if let Some(outer) = self.undo.last_mut() {
                        outer.push((index, old));
                    }
                }
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
        let at = TextRange::empty(node.range().start());
        self.implicit_read("__name__", at, ImplicitRead::FunctionModule, st);
        self.enqueue_node(node as *const Expr as usize);
    }

    // ---- results ----

    fn finish(mut self) -> Output<'a> {
        // Module names: references resolve to every module binding,
        // including those made by functions through `global`. Module code
        // adds only the latter, listed once per name (review round 25, M3:
        // going through every module binding of the name for every read
        // made a module that binds and reads one name `n` times
        // quadratic).
        let mut from_functions: FxHashMap<&str, Vec<BindingId>> = FxHashMap::default();
        for (name, ids) in &self.scopes[0].bindings {
            let made: Vec<BindingId> = ids
                .iter()
                .copied()
                .filter(|id| self.bindings[id.index()].site != ScopeId::MODULE)
                .collect();
            if !made.is_empty() {
                from_functions.insert(name, made);
            }
        }
        let add = |into: &mut Vec<BindingId>, ids: &[BindingId]| {
            if into.len() * ids.len() <= 64 {
                for id in ids {
                    if !into.contains(id) {
                        into.push(*id);
                    }
                }
            } else {
                let mut known: FxHashSet<BindingId> = into.iter().copied().collect();
                for id in ids {
                    if known.insert(*id) {
                        into.push(*id);
                    }
                }
            }
        };
        for r in &mut self.references {
            match r.resolution {
                Resolution::Global => {
                    r.bindings = self.scopes[0].bindings_of(r.name).to_vec();
                }
                Resolution::Local if r.fallback == Some(Resolution::Global) => {
                    add(&mut r.bindings, self.scopes[0].bindings_of(r.name));
                }
                Resolution::Local if r.scope == ScopeId::MODULE => {
                    if let Some(ids) = from_functions.get(r.name) {
                        add(&mut r.bindings, ids);
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
