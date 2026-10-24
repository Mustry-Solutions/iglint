//! What Jython's scope analysis (`org.python.compiler.ScopesCompiler` and
//! `ScopeInfo.cook`/`setup_closure`) can decide about each code unit, as
//! upper bounds: how many variable names, cells and free variables it
//! may have, which the code generator emits code for per name.
//!
//! A name is free in a unit when the unit (or a unit nested in it) uses
//! it and a function-like unit around it binds it (Jython passes it as a
//! cell through every unit in between, classes included: they pay per
//! pass). The bound takes every name that occurs in the unit's subtree
//! and that some function-like ancestor binds, without the names the
//! unit binds itself when it is function-like (those are its own locals
//! or cells). Jython's set is a subset of that.

use iglint_python::ast::*;
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use rustc_hash::{FxHashMap, FxHashSet};

use super::mirror::{LDC, ldc_spelled, ldc_string};
use super::{Bytes, b};

/// A code unit: the node Jython compiles into one Java method.
#[derive(Clone, Copy, Debug)]
pub(crate) enum UnitRef<'a> {
    Module(&'a Module),
    /// The `def` statement and its function.
    Function(&'a Stmt, &'a StmtFunctionDef),
    Class(&'a Stmt, &'a StmtClassDef),
    Lambda(&'a Expr, &'a ExprLambda),
    /// A generator expression, set or dict comprehension.
    Gen(&'a Expr),
}

impl UnitRef<'_> {
    /// Identifies the unit (the address of its node).
    pub(crate) fn key(self) -> usize {
        match self {
            UnitRef::Module(m) => std::ptr::from_ref(m).addr(),
            UnitRef::Function(s, _) | UnitRef::Class(s, _) => std::ptr::from_ref(s).addr(),
            UnitRef::Lambda(e, _) | UnitRef::Gen(e) => std::ptr::from_ref(e).addr(),
        }
    }
}

/// The key of the unit a `def` or `class` statement creates.
pub(crate) fn stmt_key(s: &Stmt) -> usize {
    std::ptr::from_ref(s).addr()
}

/// The key of the unit a lambda or generator expression creates.
pub(crate) fn expr_key(e: &Expr) -> usize {
    std::ptr::from_ref(e).addr()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Module,
    Class,
    /// A function, lambda or generator expression (Jython's `FUNCSCOPE`).
    Function,
}

/// Upper bounds of a unit's scope.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Facts {
    /// Contains `yield` (or is a generator expression).
    pub generator: bool,
    /// Contains a `with` statement (`max_with_count > 0`).
    pub with: bool,
    /// Variable names of the code object (`co_varnames`: parameters,
    /// locals and cells).
    pub names: usize,
    /// Cell variables.
    pub cells: usize,
    /// Free variables per `setup_closure` call.
    pub free: usize,
    /// The `ldc` of every variable name, cell and free variable (per
    /// `setup_closure` call): a long name costs more (`Code.ldc`).
    pub names_ldc: Bytes,
    pub cells_ldc: Bytes,
    pub free_ldc: Bytes,
    /// Parameters that are cells.
    pub paramcells: usize,
    /// `argcount` of the code object.
    pub argcount: usize,
    /// Frames from the creating code to the frame holding the cells
    /// (`ScopeInfo.distance`: one more per class body in between).
    pub distance: usize,
}

impl Default for Facts {
    fn default() -> Self {
        Facts {
            generator: false,
            with: false,
            names: 0,
            cells: 0,
            free: 0,
            names_ldc: b(0),
            cells_ldc: b(0),
            free_ldc: b(0),
            paramcells: 0,
            argcount: 0,
            distance: 1,
        }
    }
}

/// What one unit's own code holds (nested units excluded, but the parts
/// of them the creating code evaluates included).
#[derive(Default)]
struct Own<'a> {
    all: FxHashSet<&'a str>,
    bound: FxHashSet<&'a str>,
    params: FxHashSet<&'a str>,
    /// Names nobody can spell (`_[1_4]`, `_(1_4)`, `_(x)`, `(a, b)`),
    /// each a distinct bound name.
    synthetic: usize,
    /// The `ldc` of the synthetic names (a tuple parameter's is as long
    /// as its names).
    synthetic_ldc: Bytes,
    argcount: usize,
    children: Vec<UnitRef<'a>>,
    generator: bool,
    with: bool,
}

impl<'a> Own<'a> {
    fn name(&mut self, id: &'a str, bound: bool) {
        self.all.insert(id);
        if bound {
            self.bound.insert(id);
        }
    }

    fn params(&mut self, args: &'a Arguments) {
        fn param<'a>(own: &mut Own<'a>, e: &'a Expr) {
            match e {
                Expr::Name(n) => {
                    own.name(&n.id, true);
                    own.params.insert(&n.id);
                }
                Expr::Tuple(t) => t.elts.iter().for_each(|e| param(own, e)),
                Expr::List(t) => t.elts.iter().for_each(|e| param(own, e)),
                _ => {}
            }
        }
        for a in &args.args {
            if !matches!(a, Expr::Name(_)) {
                // The tuple's name, `(a, b)`.
                self.synthetic = self.synthetic.saturating_add(1);
                self.synthetic_ldc += ldc_string(super::mirror::tuple_name_len(a));
            }
            param(self, a);
        }
        for n in args.vararg.iter().chain(&args.kwarg) {
            self.name(n.as_str(), true);
            self.params.insert(n.as_str());
        }
        self.argcount = args
            .args
            .len()
            .saturating_add(usize::from(args.vararg.is_some()))
            .saturating_add(usize::from(args.kwarg.is_some()));
    }

    fn unit(&mut self, unit: UnitRef<'a>) {
        match unit {
            UnitRef::Module(m) => m.body.iter().for_each(|s| self.visit_stmt(s)),
            UnitRef::Function(_, f) => {
                self.params(&f.args);
                f.body.iter().for_each(|s| self.visit_stmt(s));
            }
            UnitRef::Class(_, c) => c.body.iter().for_each(|s| self.visit_stmt(s)),
            UnitRef::Lambda(_, l) => {
                self.params(&l.args);
                self.visit_expr(&l.body);
            }
            UnitRef::Gen(e) => {
                // The parameter `_(x)`, the element, the targets, the
                // conditions and the iterables after the first.
                self.synthetic = self.synthetic.saturating_add(1);
                self.synthetic_ldc += b(LDC);
                self.argcount = 1;
                self.generator = true;
                let (elts, generators): (Vec<&'a Expr>, &'a [Comprehension]) = match e {
                    Expr::GeneratorExp(g) => (vec![&g.elt], &g.generators),
                    Expr::SetComp(g) => (vec![&g.elt], &g.generators),
                    Expr::DictComp(g) => (vec![&g.key, &g.value], &g.generators),
                    _ => (Vec::new(), &[]),
                };
                elts.into_iter().for_each(|e| self.visit_expr(e));
                for (i, g) in generators.iter().enumerate() {
                    self.visit_expr(&g.target);
                    if i > 0 {
                        self.visit_expr(&g.iter);
                    }
                    g.ifs.iter().for_each(|e| self.visit_expr(e));
                }
            }
        }
    }
}

impl<'a> Visitor<'a> for Own<'a> {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::FunctionDef(f) => {
                self.name(f.name.as_str(), true);
                f.args.defaults.iter().for_each(|e| self.visit_expr(e));
                f.decorator_list.iter().for_each(|e| self.visit_expr(e));
                self.children.push(UnitRef::Function(s, f));
            }
            Stmt::ClassDef(c) => {
                self.name(c.name.as_str(), true);
                c.decorator_list.iter().for_each(|e| self.visit_expr(e));
                c.bases.iter().for_each(|e| self.visit_expr(e));
                self.children.push(UnitRef::Class(s, c));
            }
            Stmt::Import(i) => {
                for a in &i.names {
                    let bound = a.asname.as_ref().map_or_else(
                        || a.name.as_str().split('.').next().unwrap_or(""),
                        |n| n.as_str(),
                    );
                    self.name(bound, true);
                }
            }
            Stmt::ImportFrom(i) => {
                for a in &i.names {
                    if a.name.as_str() != "*" {
                        self.name(a.asname.as_ref().unwrap_or(&a.name).as_str(), true);
                    }
                }
            }
            Stmt::Global(g) => {
                for n in &g.names {
                    self.name(n.as_str(), false);
                }
            }
            Stmt::With(_) => {
                self.with = true;
                walk_stmt(self, s);
            }
            _ => walk_stmt(self, s),
        }
    }

    fn visit_expr(&mut self, e: &'a Expr) {
        match e {
            Expr::Name(n) => self.name(&n.id, n.ctx != ExprContext::Load),
            Expr::Lambda(l) => {
                l.args.defaults.iter().for_each(|e| self.visit_expr(e));
                self.children.push(UnitRef::Lambda(e, l));
            }
            Expr::GeneratorExp(ExprGeneratorExp { generators, .. })
            | Expr::SetComp(ExprSetComp { generators, .. })
            | Expr::DictComp(ExprDictComp { generators, .. }) => {
                if let Some(g) = generators.first() {
                    self.visit_expr(&g.iter);
                }
                // `_(line_col)`, bound here.
                self.synthetic = self.synthetic.saturating_add(1);
                self.synthetic_ldc += b(LDC);
                self.children.push(UnitRef::Gen(e));
            }
            Expr::ListComp(_) => {
                // `_[line_col]`, bound here.
                self.synthetic = self.synthetic.saturating_add(1);
                self.synthetic_ldc += b(LDC);
                walk_expr(self, e);
            }
            Expr::Yield(_) => {
                self.generator = true;
                walk_expr(self, e);
            }
            _ => walk_expr(self, e),
        }
    }
}

/// The facts of every unit of `module`, by [`UnitRef::key`].
pub(crate) fn facts(module: &Module) -> FxHashMap<usize, Facts> {
    let mut out = FxHashMap::default();
    let mut ancestors: Vec<(Kind, FxHashSet<&str>)> = Vec::new();
    analyse(UnitRef::Module(module), None, &mut ancestors, &mut out);
    out
}

/// Analyses `unit` (inside class `class`, the innermost around it) and
/// its nested units; returns the names that occur in its subtree.
fn analyse<'a>(
    unit: UnitRef<'a>,
    class: Option<&'a str>,
    ancestors: &mut Vec<(Kind, FxHashSet<&'a str>)>,
    out: &mut FxHashMap<usize, Facts>,
) -> FxHashSet<&'a str> {
    let kind = match unit {
        UnitRef::Module(_) => Kind::Module,
        UnitRef::Class(..) => Kind::Class,
        _ => Kind::Function,
    };
    let mut own = Own::default();
    own.unit(unit);
    let children = std::mem::take(&mut own.children);
    ancestors.push((kind, std::mem::take(&mut own.bound)));
    let mut below: FxHashSet<&'a str> = FxHashSet::default();
    let inner = match unit {
        UnitRef::Class(_, c) => Some(c.name.as_str()),
        _ => class,
    };
    for child in children {
        let names = analyse(child, inner, ancestors, out);
        below.extend(names);
    }
    let (_, bound) = ancestors.pop().unwrap_or((kind, FxHashSet::default()));
    let mut subtree = own.all;
    subtree.extend(below.iter().copied());
    let function = kind == Kind::Function;
    let ldc = |names: &mut dyn Iterator<Item = &&'a str>| -> (usize, Bytes) {
        names.fold((0usize, b(0)), |(n, bytes), name| {
            (n.saturating_add(1), bytes + ldc_spelled(class, name))
        })
    };
    let (free, free_ldc) = ldc(&mut subtree
        .iter()
        .filter(|n| !(function && bound.contains(*n)))
        .filter(|n| {
            ancestors
                .iter()
                .any(|(k, b)| *k == Kind::Function && b.contains(*n))
        }));
    let distance = ancestors
        .iter()
        .rev()
        .take_while(|(k, _)| *k == Kind::Class)
        .count()
        .saturating_add(1);
    let (names, names_ldc, cells, cells_ldc, paramcells) = if function {
        let (cells, cells_ldc) = ldc(&mut bound.iter().filter(|n| below.contains(*n)));
        let (_, bound_ldc) = ldc(&mut bound.iter());
        (
            bound.len().saturating_add(own.synthetic),
            bound_ldc + own.synthetic_ldc,
            cells,
            cells_ldc,
            own.params.iter().filter(|n| below.contains(*n)).count(),
        )
    } else {
        (0, b(0), 0, b(0), 0)
    };
    out.insert(
        unit.key(),
        Facts {
            generator: own.generator,
            with: own.with,
            names,
            cells,
            free: if ancestors.is_empty() { 0 } else { free },
            names_ldc,
            cells_ldc,
            free_ldc: if ancestors.is_empty() { b(0) } else { free_ldc },
            paramcells,
            argcount: own.argcount,
            distance,
        },
    );
    subtree
}
