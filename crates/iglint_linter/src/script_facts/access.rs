//! Property accesses of view scripts, found on the Jython syntax tree.
//!
//! The analysis follows values that are view objects through attribute
//! chains starting at the script's `self`:
//!
//! | expression | value |
//! |---|---|
//! | `self` | the owning component (the view in view-level scripts) |
//! | `<component>.view`, `<view>.view` | the view |
//! | `<component>.parent`, `.getParent()` | the parent component |
//! | `<component>.getSibling('A')`, `.getChild('A/B')` | that component (any component when the name is not a literal) |
//! | `<component>.children`, `.getChildren()` | any component |
//! | `<view>.rootContainer`, `<view>.getChild('root/A')` | that component |
//! | `<object>.custom`, `<view>.params` | a property container |
//!
//! An attribute of a container (`self.view.custom.total`) or a subscript
//! with a string literal (`self.custom['total']`) accesses that property;
//! the attribute's context says whether it is read (`Load`), written
//! (`Store`, `Del`) or both (augmented assignment).
//!
//! Everything else that touches these values is treated conservatively:
//!
//! * a container used as a value (`d = self.view.custom`, `len(self.custom)`,
//!   `self.custom[key]`, `self.custom.keys()`) accesses every property of
//!   that container (`name: None`);
//! * a view or component used as a value (passed to a function, returned,
//!   stored in a variable that is assigned more than once, `exec`,
//!   `locals()`) makes everything reachable, so every property of every
//!   component and of the view counts as accessed.
//!
//! A local name assigned exactly once from one of these values
//! (`view = self.view`, `table = self.getSibling('Table')`) is followed as
//! an alias.
//!
//! A value passed to a call of a dotted name that the script does not bind
//! (`lib.util.handle(self)`, `lib.util.handle(view=self.view)`) escapes
//! too, but the assumed accesses carry the call ([`LibraryCall`]). Rules
//! that have the project index resolve the callee: a library function that
//! does not exist reads nothing (the call fails before it runs), and an
//! existing one is followed through [`ParamUse`], the summary of what the
//! function does with each parameter ([`param_uses`]), computed with the
//! same rules on its body.

use std::sync::Arc;

use iglint_project::library::{ArgSlot, ParamPath, ParamUse, PathEnd, PathStep};
use iglint_project::view::{PropCategory, View};
use iglint_project::{ComponentId, Owner};
use iglint_python::ast::{
    CmpOp, Expr, ExprCall, ExprContext, Slice, Stmt, StmtFunctionDef, UnaryOp,
};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_text::TextRange;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{AccessTarget, PropertyAccess, ScriptFacts};
use crate::rule::ScriptUnit;
use iglint_project::summary::ResourceSummary;

/// Script facts from the syntax tree and the semantic model.
pub struct AstScriptFacts;

impl ScriptFacts for AstScriptFacts {
    fn complete(&self) -> bool {
        true
    }

    fn provides_references(&self) -> bool {
        true
    }

    fn summarize(
        &self,
        unit: &ScriptUnit<'_, '_>,
        ctx: &super::FactsContext<'_>,
        summary: &mut ResourceSummary,
    ) {
        let library = unit.script.kind == iglint_project::ScriptKind::LibraryModule;
        if !unit.parsed.errors.is_empty() {
            // The script does not compile and never runs; a module that
            // does not compile has unknown symbols.
            return;
        }
        let mut injected: Vec<&str> = unit.script.implicit_globals().collect();
        if library {
            injected.push("__package__");
        }
        let model = iglint_python_semantic::SemanticModel::build(
            &unit.parsed.module,
            &iglint_python_semantic::SemanticOptions {
                injected,
                unknown_globals: false,
            },
        );
        super::refs::collect(&model, ctx, &mut summary.references);
        if library {
            summary.set_module_symbols(super::symbols::module_symbols(&model));
        }
    }

    fn property_accesses(
        &self,
        unit: &ScriptUnit<'_, '_>,
        view: &View<'_>,
        out: &mut Vec<PropertyAccess>,
    ) {
        let this = match unit.script.owner {
            Owner::View => Val::Object(AccessTarget::View),
            Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
            _ => return,
        };
        let mut found = Vec::new();
        if !unit.parsed.is_valid() {
            // A script that does not compile never runs, so it writes
            // nothing; it is still taken as reading everything so that
            // unused-property findings do not depend on a script the user
            // has to fix anyway (SCR001 reports it).
            let anchor = unit.script.anchor();
            let start = out.len();
            let mut a = Analysis {
                view,
                this,
                aliases: FxHashMap::default(),
                bound: FxHashSet::default(),
                out,
            };
            a.everything(anchor);
            for access in &mut a.out[start..] {
                access.write = false;
            }
            return;
        }
        let mut a = Analysis {
            view,
            this,
            aliases: FxHashMap::default(),
            bound: bound_names(&unit.parsed.module.body),
            out: &mut found,
        };
        a.collect_aliases(&unit.parsed.module.body);
        a.visit_module(&unit.parsed.module);
        for mut access in found {
            access.range = unit.script.file_range(access.range);
            out.push(access);
        }
    }
}

/// A value the analysis follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Val {
    Object(AccessTarget),
    Container(AccessTarget, PropCategory),
}

/// How an expression's value is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The value goes somewhere unknown.
    Value,
    /// The value is the base of an attribute, subscript or call that the
    /// caller interprets.
    Base,
}

struct Analysis<'v, 'a, 'o> {
    view: &'v View<'a>,
    this: Val,
    aliases: FxHashMap<String, Option<Val>>,
    /// Every name the script binds (assignments, imports, `def`, loop
    /// targets, ...): such a name never refers to the script library.
    bound: FxHashSet<String>,
    out: &'o mut Vec<PropertyAccess>,
}

const OPAQUE_CALLS: &[&str] = &["locals", "globals", "vars", "eval", "execfile", "dir"];

impl<'v, 'a> Analysis<'v, 'a, '_> {
    // ---- aliases ----

    /// Names assigned exactly once, from a followed value.
    fn collect_aliases(&mut self, body: &[Stmt]) {
        #[derive(Default)]
        struct Stores<'e> {
            count: FxHashMap<String, (usize, Option<&'e Expr>)>,
        }
        impl<'e> Visitor<'e> for Stores<'e> {
            fn visit_stmt(&mut self, stmt: &'e Stmt) {
                if let Stmt::Assign(s) = stmt
                    && let [Expr::Name(n)] = s.targets.as_slice()
                {
                    let e = self.count.entry(n.id.to_string()).or_default();
                    e.0 += 1;
                    e.1 = Some(&s.value);
                    self.visit_expr(&s.value);
                    return;
                }
                walk_stmt(self, stmt);
            }
            fn visit_expr(&mut self, expr: &'e Expr) {
                if let Expr::Name(n) = expr
                    && matches!(
                        n.ctx,
                        ExprContext::Store | ExprContext::Param | ExprContext::AugStore
                    )
                {
                    // Other kinds of stores: never an alias.
                    self.count.entry(n.id.to_string()).or_default().0 += 2;
                }
                iglint_python::visitor::walk_expr(self, expr);
            }
        }
        let mut stores = Stores::default();
        for stmt in body {
            stores.visit_stmt(stmt);
        }
        // Resolve in a few rounds so aliases of aliases work.
        for _ in 0..3 {
            for (name, (count, value)) in &stores.count {
                if name == "self" {
                    continue;
                }
                if *count == 1
                    && let Some(value) = value
                {
                    let v = self.peek(value);
                    self.aliases.insert(name.clone(), v);
                } else {
                    self.aliases.insert(name.clone(), None);
                }
            }
        }
    }

    /// The value of `e` without recording anything.
    fn peek(&self, e: &Expr) -> Option<Val> {
        match e {
            Expr::Name(n) if n.ctx == ExprContext::Load => self.name_value(&n.id),
            Expr::Attribute(a) => {
                let base = self.peek(&a.value)?;
                self.attribute(base, a.attr.as_str())
            }
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else {
                    return None;
                };
                let base = self.peek(&a.value)?;
                self.method(base, a.attr.as_str(), &c.args)
            }
            Expr::Subscript(s) => match self.peek(&s.value)? {
                Val::Object(AccessTarget::AnyComponent) => {
                    Some(Val::Object(AccessTarget::AnyComponent))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn name_value(&self, name: &str) -> Option<Val> {
        if name == "self" {
            return Some(self.this);
        }
        self.aliases.get(name).copied().flatten()
    }

    // ---- navigation ----

    fn parent(&self, t: AccessTarget) -> AccessTarget {
        match t {
            AccessTarget::Component(c) => match self.view.component(c).parent {
                Some(p) => AccessTarget::Component(p),
                None => AccessTarget::AnyComponent,
            },
            _ => AccessTarget::AnyComponent,
        }
    }

    fn child_path(&self, from: Option<ComponentId>, path: &str) -> Option<AccessTarget> {
        let mut segments = path.split('/').filter(|s| !s.is_empty());
        let mut at = match from {
            Some(c) => c,
            None => {
                // From the view: the first segment names the root.
                let root = self.view.root_component()?;
                if segments.next()? != root.name {
                    return None;
                }
                root.id
            }
        };
        for segment in segments {
            at = self.view.children(at).find(|c| c.name == segment)?.id;
        }
        Some(AccessTarget::Component(at))
    }

    fn attribute(&self, base: Val, attr: &str) -> Option<Val> {
        match base {
            Val::Object(t) => match (t, attr) {
                (_, "custom") => Some(Val::Container(t, PropCategory::Custom)),
                (AccessTarget::View, "params") => Some(Val::Container(t, PropCategory::Params)),
                (AccessTarget::View, "rootContainer") => self
                    .view
                    .root_component()
                    .map(|r| Val::Object(AccessTarget::Component(r.id))),
                // `self.view` also works on the view itself (view-level
                // bindings and events).
                (_, "view") => Some(Val::Object(AccessTarget::View)),
                (AccessTarget::Component(_) | AccessTarget::AnyComponent, "parent") => {
                    Some(Val::Object(self.parent(t)))
                }
                (AccessTarget::Component(_) | AccessTarget::AnyComponent, "children") => {
                    Some(Val::Object(AccessTarget::AnyComponent))
                }
                _ => None,
            },
            Val::Container(..) => None,
        }
    }

    fn method(&self, base: Val, name: &str, args: &[Expr]) -> Option<Val> {
        self.method_literal(base, name, first_literal(args))
    }

    /// [`Analysis::method`] with the first argument as a string literal.
    fn method_literal(&self, base: Val, name: &str, literal: Option<String>) -> Option<Val> {
        let Val::Object(t) = base else {
            return None;
        };
        match (t, name) {
            (AccessTarget::Component(c), "getSibling") => {
                let Some(name) = literal else {
                    return Some(Val::Object(AccessTarget::AnyComponent));
                };
                let parent = self.view.component(c).parent?;
                // `getSibling` looks the name up in the parent, so a
                // component finds itself too.
                let sibling = self.view.children(parent).find(|s| s.name == name)?;
                Some(Val::Object(AccessTarget::Component(sibling.id)))
            }
            (AccessTarget::Component(c), "getChild") => match literal {
                Some(path) => self.child_path(Some(c), &path).map(Val::Object),
                None => Some(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::View, "getChild") => match literal {
                Some(path) => self.child_path(None, &path).map(Val::Object),
                None => Some(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::AnyComponent, "getSibling" | "getChild") => {
                Some(Val::Object(AccessTarget::AnyComponent))
            }
            (AccessTarget::Component(_) | AccessTarget::AnyComponent, "getParent") => {
                Some(Val::Object(self.parent(t)))
            }
            (AccessTarget::Component(_) | AccessTarget::AnyComponent, "getChildren") => {
                Some(Val::Object(AccessTarget::AnyComponent))
            }
            _ => None,
        }
    }

    // ---- recording ----

    fn record(
        &mut self,
        t: AccessTarget,
        category: PropCategory,
        name: Option<String>,
        ctx: ExprContext,
        range: TextRange,
    ) {
        let (read, write) = match ctx {
            ExprContext::Load => (true, false),
            ExprContext::AugLoad | ExprContext::AugStore => (true, true),
            ExprContext::Store | ExprContext::Del | ExprContext::Param => (false, true),
        };
        self.out.push(PropertyAccess {
            target: t,
            category,
            name,
            read,
            write,
            range,
            via: None,
        });
    }

    /// A followed value escapes: everything reachable may be accessed.
    fn escape(&mut self, val: Val, range: TextRange) {
        match val {
            // A container passed on may be read and written.
            Val::Container(t, c) => self.record(t, c, None, ExprContext::AugLoad, range),
            Val::Object(_) => self.everything(range),
        }
    }

    fn everything(&mut self, range: TextRange) {
        for (t, c) in [
            (AccessTarget::View, PropCategory::Custom),
            (AccessTarget::View, PropCategory::Params),
            (AccessTarget::AnyComponent, PropCategory::Custom),
        ] {
            self.out.push(PropertyAccess {
                target: t,
                category: c,
                name: None,
                read: true,
                write: true,
                range,
                via: None,
            });
        }
    }

    /// [`Analysis::escape`] into a call of `callee`: the assumed accesses
    /// carry the call.
    fn escape_into(&mut self, val: Val, range: TextRange, callee: &str, slot: ArgSlot) {
        let start = self.out.len();
        self.escape(val, range);
        let call = Arc::new(LibraryCall {
            callee: callee.to_owned(),
            slot,
            value: val.into(),
        });
        for access in &mut self.out[start..] {
            access.via = Some(Arc::clone(&call));
        }
    }

    /// The dotted name a call calls, when its first name is not bound by
    /// the script (so it may name the script library).
    fn library_callee(&self, func: &Expr) -> Option<String> {
        let dotted = dotted_name(func)?;
        let head = dotted.split('.').next()?;
        (!self.bound.contains(head) && head != "self").then_some(dotted)
    }

    // ---- walking ----

    /// Walks an expression; returns its followed value when `role` is
    /// [`Role::Base`]. In [`Role::Value`] a followed value escapes.
    fn walk(&mut self, e: &Expr, role: Role, aug: bool) -> Option<Val> {
        let result = match e {
            Expr::Name(n) => match n.ctx {
                ExprContext::Load | ExprContext::AugLoad => self.name_value(&n.id),
                _ => None,
            },
            Expr::Attribute(a) => {
                let base = self.walk(&a.value, Role::Base, false);
                match base {
                    Some(Val::Container(t, c)) => {
                        let ctx = if aug { ExprContext::AugStore } else { a.ctx };
                        self.record(t, c, Some(a.attr.as_str().to_owned()), ctx, a.attr.range);
                        None
                    }
                    Some(b @ Val::Object(_)) => {
                        if !matches!(a.ctx, ExprContext::Load) && a.attr.as_str() == "custom" {
                            // `self.custom = {...}` replaces every property.
                            if let Some(Val::Container(t, c)) = self.attribute(b, "custom") {
                                self.record(t, c, None, ExprContext::Store, a.attr.range);
                            }
                            None
                        } else {
                            self.attribute(b, a.attr.as_str())
                        }
                    }
                    None => None,
                }
            }
            Expr::Subscript(s) => {
                let base = self.walk(&s.value, Role::Base, false);
                let key = match &*s.slice {
                    Slice::Index(i) => match &*i.value {
                        Expr::Str(st) => st.s.as_str().map(str::to_owned),
                        _ => None,
                    },
                    _ => None,
                };
                self.walk_slice(&s.slice);
                match base {
                    Some(Val::Container(t, c)) => {
                        let ctx = if aug { ExprContext::AugStore } else { s.ctx };
                        self.record(t, c, key, ctx, s.range);
                        None
                    }
                    Some(Val::Object(AccessTarget::AnyComponent)) => {
                        Some(Val::Object(AccessTarget::AnyComponent))
                    }
                    Some(other) => {
                        self.escape(other, s.range);
                        None
                    }
                    None => None,
                }
            }
            Expr::Call(c) => {
                let mut value = None;
                match &*c.func {
                    Expr::Attribute(a) => {
                        let base = self.walk(&a.value, Role::Base, false);
                        match base {
                            Some(b @ Val::Object(_)) => {
                                value = self.method(b, a.attr.as_str(), &c.args);
                            }
                            Some(Val::Container(t, cat)) => {
                                // `self.custom.keys()`, `.get(...)`, ...
                                self.record(t, cat, None, ExprContext::Load, a.range);
                            }
                            None => {}
                        }
                    }
                    Expr::Name(n) if OPAQUE_CALLS.contains(&&*n.id) => {
                        self.everything(c.range);
                    }
                    other => {
                        self.walk(other, Role::Value, false);
                    }
                }
                match self.library_callee(&c.func) {
                    Some(callee) => self.walk_call_arguments(c, &callee),
                    None => {
                        for arg in &c.args {
                            self.walk(arg, Role::Value, false);
                        }
                        for k in &c.keywords {
                            self.walk(&k.value, Role::Value, false);
                        }
                    }
                }
                if let Some(s) = &c.starargs {
                    self.walk(s, Role::Value, false);
                }
                if let Some(k) = &c.kwargs {
                    self.walk(k, Role::Value, false);
                }
                value
            }
            other => {
                // Any other expression: walk its children as values.
                let mut children = Children::default();
                iglint_python::visitor::walk_expr(&mut children, other);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                None
            }
        };
        match (role, result) {
            (Role::Value, Some(v)) => {
                self.escape(v, e_range(e));
                None
            }
            _ => result,
        }
    }

    fn walk_slice(&mut self, slice: &Slice) {
        let mut children = Children::default();
        children.visit_slice(slice);
        for child in children.0 {
            self.walk(child, Role::Value, false);
        }
    }

    /// The positional and keyword arguments of a call of `callee`: followed
    /// values escape into the call.
    fn walk_call_arguments(&mut self, c: &ExprCall, callee: &str) {
        for (i, arg) in c.args.iter().enumerate() {
            if let Some(v) = self.walk(arg, Role::Base, false) {
                self.escape_into(v, e_range(arg), callee, ArgSlot::Position(i));
            }
        }
        for k in &c.keywords {
            if let Some(v) = self.walk(&k.value, Role::Base, false) {
                let slot = ArgSlot::Keyword(k.arg.as_str().to_owned());
                self.escape_into(v, e_range(&k.value), callee, slot);
            }
        }
    }

    /// What `path` does when the parameter it starts from holds `start`
    /// (a value of this view). Calls it passes the value on to go to
    /// `passed`.
    fn replay(
        &mut self,
        start: Val,
        path: &ParamPath,
        range: TextRange,
        passed: &mut dyn FnMut(&mut Self, &str, &ArgSlot, Val),
    ) {
        let mut v = start;
        let last = path.steps.len().saturating_sub(1);
        for (k, step) in path.steps.iter().enumerate() {
            let ctx = if k < last {
                ExprContext::Load
            } else {
                match path.end {
                    PathEnd::Store => ExprContext::Store,
                    PathEnd::Update => ExprContext::AugStore,
                    PathEnd::Load | PathEnd::Passed { .. } => ExprContext::Load,
                }
            };
            match (v, step) {
                (Val::Container(t, c), PathStep::Attr(n) | PathStep::Index(Some(n))) => {
                    self.record(t, c, Some(n.clone()), ctx, range);
                    return;
                }
                (Val::Container(t, c), PathStep::Index(None)) => {
                    self.record(t, c, None, ctx, range);
                    return;
                }
                (Val::Container(t, c), PathStep::Method { .. }) => {
                    self.record(t, c, None, ExprContext::Load, range);
                    return;
                }
                (Val::Object(_), PathStep::Attr(n))
                    if n == "custom" && ctx != ExprContext::Load =>
                {
                    // `param.custom = {...}` replaces every property.
                    if let Some(Val::Container(t, c)) = self.attribute(v, "custom") {
                        self.record(t, c, None, ExprContext::Store, range);
                    }
                    return;
                }
                (Val::Object(_), PathStep::Attr(n)) => match self.attribute(v, n) {
                    Some(next) => v = next,
                    None => return,
                },
                (Val::Object(_), PathStep::Method { name, literal }) => {
                    match self.method_literal(v, name, literal.clone()) {
                        Some(next) => v = next,
                        None => return,
                    }
                }
                (Val::Object(AccessTarget::AnyComponent), PathStep::Index(_)) => {}
                (Val::Object(_), PathStep::Index(_)) => {
                    self.escape(v, range);
                    return;
                }
            }
        }
        match &path.end {
            PathEnd::Passed { callee, slot } => passed(self, callee, slot, v),
            _ => self.escape(v, range),
        }
    }
}

/// A followed value of a view script that is passed to a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Escaped {
    /// A component or the view.
    Object(AccessTarget),
    /// A property container (`self.view.custom`).
    Container(AccessTarget, PropCategory),
}

impl From<Val> for Escaped {
    fn from(v: Val) -> Escaped {
        match v {
            Val::Object(t) => Escaped::Object(t),
            Val::Container(t, c) => Escaped::Container(t, c),
        }
    }
}

impl From<Escaped> for Val {
    fn from(v: Escaped) -> Val {
        match v {
            Escaped::Object(t) => Val::Object(t),
            Escaped::Container(t, c) => Val::Container(t, c),
        }
    }
}

/// A call that a followed value of a view script is passed to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LibraryCall {
    /// The dotted name called, as written (`lib.util.handle`).
    pub callee: String,
    /// The parameter the value binds to.
    pub slot: ArgSlot,
    pub value: Escaped,
}

/// How a callee resolves, for [`follow_call`].
pub enum Callee<'i> {
    /// It does not exist: the call fails before anything is read.
    Missing,
    /// A library function; `context` resolves the names it calls.
    Function(&'i iglint_project::library::Signature, CalleeContext),
    /// Anything else (not a library name, a class, an import, unknown).
    Unknown,
}

/// Where a function is defined, to resolve the names its body calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalleeContext {
    pub module: iglint_project::ResourceId,
}

/// How many calls deep [`follow_call`] follows a value before it assumes
/// everything is accessed.
pub const MAX_CALL_DEPTH: usize = 4;

/// The accesses a call makes with `value` of `view`, bound to parameter
/// `slot` of `callee`. `resolve` turns a callee name (relative to the
/// calling module, `None` for the view script) into a [`Callee`]. Unknown
/// callees, dynamic parameters and calls nested deeper than
/// [`MAX_CALL_DEPTH`] access everything the value reaches.
pub fn follow_call<'i>(
    view: &View<'_>,
    call: &LibraryCall,
    range: TextRange,
    resolve: &dyn Fn(&str, Option<CalleeContext>) -> Callee<'i>,
) -> Vec<PropertyAccess> {
    let mut out = Vec::new();
    let mut a = Analysis {
        view,
        this: Val::Object(AccessTarget::View),
        aliases: FxHashMap::default(),
        bound: FxHashSet::default(),
        out: &mut out,
    };
    follow(
        &mut a,
        &call.callee,
        None,
        &call.slot,
        call.value.into(),
        range,
        resolve,
        0,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn follow<'i>(
    a: &mut Analysis<'_, '_, '_>,
    callee: &str,
    context: Option<CalleeContext>,
    slot: &ArgSlot,
    value: Val,
    range: TextRange,
    resolve: &dyn Fn(&str, Option<CalleeContext>) -> Callee<'i>,
    depth: usize,
) {
    if depth >= MAX_CALL_DEPTH {
        a.escape(value, range);
        return;
    }
    match resolve(callee, context) {
        Callee::Missing => {}
        Callee::Unknown => a.escape(value, range),
        Callee::Function(signature, inner) => match signature.use_of(slot) {
            ParamUse::Dynamic => a.escape(value, range),
            ParamUse::Paths(paths) => {
                for path in paths {
                    a.replay(value, path, range, &mut |a, callee, slot, v| {
                        follow(a, callee, Some(inner), slot, v, range, resolve, depth + 1);
                    });
                }
            }
        },
    }
}

/// The first argument of a call, when it is a string literal.
fn first_literal(args: &[Expr]) -> Option<String> {
    match args.first() {
        Some(Expr::Str(s)) => s.s.as_str().map(str::to_owned),
        _ => None,
    }
}

/// `a.b.c` for a chain of attribute reads on a name.
fn dotted_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) if n.ctx == ExprContext::Load => Some(n.id.to_string()),
        Expr::Attribute(a) if a.ctx == ExprContext::Load => {
            let mut base = dotted_name(&a.value)?;
            base.push('.');
            base.push_str(a.attr.as_str());
            Some(base)
        }
        _ => None,
    }
}

/// Every name bound in `body`, nested scopes included: assignment and loop
/// targets, parameters, imports, `def`, `class`, `global`.
fn bound_names(body: &[Stmt]) -> FxHashSet<String> {
    #[derive(Default)]
    struct Bound(FxHashSet<String>);
    impl<'e> Visitor<'e> for Bound {
        fn visit_stmt(&mut self, stmt: &'e Stmt) {
            match stmt {
                Stmt::FunctionDef(f) => {
                    self.0.insert(f.name.as_str().to_owned());
                    if let Some(v) = &f.args.vararg {
                        self.0.insert(v.as_str().to_owned());
                    }
                    if let Some(k) = &f.args.kwarg {
                        self.0.insert(k.as_str().to_owned());
                    }
                }
                Stmt::ClassDef(c) => {
                    self.0.insert(c.name.as_str().to_owned());
                }
                Stmt::Import(i) => {
                    for alias in &i.names {
                        let name = alias.asname.as_ref().map_or_else(
                            || alias.name.as_str().split('.').next().unwrap_or(""),
                            |a| a.as_str(),
                        );
                        self.0.insert(name.to_owned());
                    }
                }
                Stmt::ImportFrom(i) => {
                    for alias in &i.names {
                        let name = alias
                            .asname
                            .as_ref()
                            .map_or_else(|| alias.name.as_str(), |a| a.as_str());
                        self.0.insert(name.to_owned());
                    }
                }
                Stmt::Global(g) => {
                    for n in &g.names {
                        self.0.insert(n.as_str().to_owned());
                    }
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'e Expr) {
            if let Expr::Name(n) = expr
                && n.ctx != ExprContext::Load
            {
                self.0.insert(n.id.to_string());
            }
            if let Expr::Lambda(l) = expr {
                if let Some(v) = &l.args.vararg {
                    self.0.insert(v.as_str().to_owned());
                }
                if let Some(k) = &l.args.kwarg {
                    self.0.insert(k.as_str().to_owned());
                }
            }
            walk_expr(self, expr);
        }
    }
    let mut bound = Bound::default();
    for stmt in body {
        bound.visit_stmt(stmt);
    }
    bound.0
}

// ---- library functions ----

/// What `f` does with each of its positional parameters, when they hold a
/// Perspective object (see [`ParamUse`]).
pub fn param_uses(f: &StmtFunctionDef) -> Vec<ParamUse> {
    let names: Vec<Option<String>> = f
        .args
        .args
        .iter()
        .map(|a| match a {
            Expr::Name(n) => Some(n.id.to_string()),
            _ => None,
        })
        .collect();
    let mut w = ParamWalker {
        params: FxHashMap::default(),
        aliases: FxHashMap::default(),
        bound: bound_names(&f.body),
        uses: names.iter().map(|_| ParamUse::Paths(Vec::new())).collect(),
    };
    for (i, name) in names.iter().enumerate() {
        match name {
            // A parameter bound again in the body (or in a nested scope,
            // which may shadow it) is not followed.
            Some(n) if !w.bound.contains(n) => {
                w.params.insert(n.clone(), i);
            }
            _ => w.uses[i] = ParamUse::Dynamic,
        }
    }
    if w.params.is_empty() {
        return w.uses;
    }
    // Defaults are evaluated where the function is defined.
    w.collect_aliases(&f.body);
    for stmt in &f.body {
        w.visit_stmt(stmt);
    }
    w.uses
}

/// A value derived from a parameter: the parameter and the steps.
type Sym = (usize, Vec<PathStep>);

struct ParamWalker {
    params: FxHashMap<String, usize>,
    /// Names assigned exactly once from a parameter path.
    aliases: FxHashMap<String, Option<Sym>>,
    bound: FxHashSet<String>,
    uses: Vec<ParamUse>,
}

impl ParamWalker {
    fn collect_aliases(&mut self, body: &[Stmt]) {
        #[derive(Default)]
        struct Stores<'e> {
            count: FxHashMap<String, (usize, Option<&'e Expr>)>,
        }
        impl<'e> Visitor<'e> for Stores<'e> {
            fn visit_stmt(&mut self, stmt: &'e Stmt) {
                if let Stmt::Assign(s) = stmt
                    && let [Expr::Name(n)] = s.targets.as_slice()
                {
                    let e = self.count.entry(n.id.to_string()).or_default();
                    e.0 += 1;
                    e.1 = Some(&s.value);
                    self.visit_expr(&s.value);
                    return;
                }
                walk_stmt(self, stmt);
            }
            fn visit_expr(&mut self, expr: &'e Expr) {
                if let Expr::Name(n) = expr
                    && n.ctx != ExprContext::Load
                {
                    self.count.entry(n.id.to_string()).or_default().0 += 2;
                }
                walk_expr(self, expr);
            }
        }
        let mut stores = Stores::default();
        for stmt in body {
            stores.visit_stmt(stmt);
        }
        for _ in 0..3 {
            for (name, (count, value)) in &stores.count {
                let v = match (count, value) {
                    (1, Some(value)) => self.peek(value),
                    _ => None,
                };
                self.aliases.insert(name.clone(), v);
            }
        }
    }

    fn peek(&self, e: &Expr) -> Option<Sym> {
        match e {
            Expr::Name(n) if n.ctx == ExprContext::Load => self.name_value(&n.id),
            Expr::Attribute(a) => {
                let (p, mut steps) = self.peek(&a.value)?;
                steps.push(PathStep::Attr(a.attr.as_str().to_owned()));
                Some((p, steps))
            }
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else {
                    return None;
                };
                let (p, mut steps) = self.peek(&a.value)?;
                steps.push(PathStep::Method {
                    name: a.attr.as_str().to_owned(),
                    literal: first_literal(&c.args),
                });
                Some((p, steps))
            }
            _ => None,
        }
    }

    fn name_value(&self, name: &str) -> Option<Sym> {
        if let Some(p) = self.params.get(name) {
            return Some((*p, Vec::new()));
        }
        self.aliases.get(name).cloned().flatten()
    }

    fn record(&mut self, (p, steps): Sym, end: PathEnd) {
        if let ParamUse::Paths(paths) = &mut self.uses[p] {
            let path = ParamPath { steps, end };
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }

    fn dynamic_all(&mut self) {
        self.uses.fill(ParamUse::Dynamic);
    }

    /// Like [`Analysis::walk`], over parameter paths.
    fn walk(&mut self, e: &Expr, role: Role, aug: bool) -> Option<Sym> {
        let result = match e {
            Expr::Name(n) => match n.ctx {
                ExprContext::Load | ExprContext::AugLoad => self.name_value(&n.id),
                _ => None,
            },
            Expr::Attribute(a) => {
                let base = self.walk(&a.value, Role::Base, false)?;
                let (p, mut steps) = base;
                steps.push(PathStep::Attr(a.attr.as_str().to_owned()));
                match (aug, a.ctx) {
                    (true, _) => {
                        self.record((p, steps), PathEnd::Update);
                        None
                    }
                    (false, ExprContext::Load) => Some((p, steps)),
                    _ => {
                        self.record((p, steps), PathEnd::Store);
                        None
                    }
                }
            }
            Expr::Subscript(s) => {
                let base = self.walk(&s.value, Role::Base, false);
                let key = match &*s.slice {
                    Slice::Index(i) => match &*i.value {
                        Expr::Str(st) => st.s.as_str().map(str::to_owned),
                        _ => None,
                    },
                    _ => None,
                };
                let mut children = Children::default();
                children.visit_slice(&s.slice);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                let (p, mut steps) = base?;
                steps.push(PathStep::Index(key));
                match (aug, s.ctx) {
                    (true, _) => {
                        self.record((p, steps), PathEnd::Update);
                        None
                    }
                    (false, ExprContext::Load) => Some((p, steps)),
                    _ => {
                        self.record((p, steps), PathEnd::Store);
                        None
                    }
                }
            }
            Expr::Call(c) => {
                let mut value = None;
                let mut callee = None;
                match &*c.func {
                    Expr::Attribute(a) => {
                        if let Some((p, mut steps)) = self.walk(&a.value, Role::Base, false) {
                            steps.push(PathStep::Method {
                                name: a.attr.as_str().to_owned(),
                                literal: first_literal(&c.args),
                            });
                            value = Some((p, steps));
                        } else {
                            callee = dotted_name(&c.func);
                        }
                    }
                    Expr::Name(n) if OPAQUE_CALLS.contains(&&*n.id) => self.dynamic_all(),
                    Expr::Name(_) => callee = dotted_name(&c.func),
                    other => {
                        self.walk(other, Role::Value, false);
                    }
                }
                // Only names the function does not bind are resolved later.
                let callee = callee.filter(|d| {
                    let head = d.split('.').next().unwrap_or("");
                    !self.bound.contains(head) && !self.params.contains_key(head)
                });
                for (i, arg) in c.args.iter().enumerate() {
                    match &callee {
                        Some(callee) => {
                            if let Some(sym) = self.walk(arg, Role::Base, false) {
                                let slot = ArgSlot::Position(i);
                                let callee = callee.clone();
                                self.record(sym, PathEnd::Passed { callee, slot });
                            }
                        }
                        None => {
                            self.walk(arg, Role::Value, false);
                        }
                    }
                }
                for k in &c.keywords {
                    match &callee {
                        Some(callee) => {
                            if let Some(sym) = self.walk(&k.value, Role::Base, false) {
                                let slot = ArgSlot::Keyword(k.arg.as_str().to_owned());
                                let callee = callee.clone();
                                self.record(sym, PathEnd::Passed { callee, slot });
                            }
                        }
                        None => {
                            self.walk(&k.value, Role::Value, false);
                        }
                    }
                }
                if let Some(s) = &c.starargs {
                    self.walk(s, Role::Value, false);
                }
                if let Some(k) = &c.kwargs {
                    self.walk(k, Role::Value, false);
                }
                value
            }
            // Identity tests and truth values read no property.
            Expr::Compare(c) if c.ops.iter().all(|o| matches!(o, CmpOp::Is | CmpOp::IsNot)) => {
                self.test(&c.left);
                for comparator in &c.comparators {
                    self.test(comparator);
                }
                None
            }
            Expr::UnaryOp(u) if u.op == UnaryOp::Not => {
                self.test(&u.operand);
                None
            }
            Expr::Lambda(_) | Expr::GeneratorExp(_) | Expr::SetComp(_) | Expr::DictComp(_)
                if self.mentions_param(e) =>
            {
                // Nested scopes are not followed.
                self.dynamic_all();
                None
            }
            other => {
                let mut children = Children::default();
                walk_expr(&mut children, other);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                None
            }
        };
        match (role, result) {
            (Role::Value, Some(sym)) => {
                self.record(sym, PathEnd::Load);
                None
            }
            (_, result) => result,
        }
    }

    /// An expression whose truth value or identity is all that is used:
    /// the parameter itself is not read, anything derived from it is.
    fn test(&mut self, e: &Expr) {
        if let Some(sym) = self.walk(e, Role::Base, false)
            && !sym.1.is_empty()
        {
            self.record(sym, PathEnd::Load);
        }
    }

    /// Whether `e` reads a parameter or an alias of one.
    fn mentions_param(&self, e: &Expr) -> bool {
        struct Mentions<'w>(&'w ParamWalker, bool);
        impl<'e> Visitor<'e> for Mentions<'_> {
            fn visit_expr(&mut self, expr: &'e Expr) {
                if let Expr::Name(n) = expr
                    && (self.0.params.contains_key(&*n.id)
                        || self.0.aliases.get(&*n.id).is_some_and(Option::is_some))
                {
                    self.1 = true;
                }
                walk_expr(self, expr);
            }
        }
        let mut m = Mentions(self, false);
        m.visit_expr(e);
        m.1
    }
}

impl<'e> Visitor<'e> for ParamWalker {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                if let [Expr::Name(n)] = s.targets.as_slice()
                    && self.aliases.get(&*n.id).is_some_and(Option::is_some)
                {
                    self.walk(&s.value, Role::Base, false);
                    return;
                }
                for t in &s.targets {
                    self.walk(t, Role::Value, false);
                }
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::AugAssign(s) => {
                self.walk(&s.target, Role::Value, true);
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::Exec(_) => self.dynamic_all(),
            Stmt::If(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::While(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                let mut m = MentionsStmt(self, false);
                m.visit_stmt(stmt);
                if m.1 {
                    self.dynamic_all();
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.walk(expr, Role::Value, false);
    }
}

/// Whether a nested `def` or `class` reads a parameter (closures are not
/// followed).
struct MentionsStmt<'w>(&'w ParamWalker, bool);

impl<'e> Visitor<'e> for MentionsStmt<'_> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        if let Expr::Name(n) = expr
            && (self.0.params.contains_key(&*n.id)
                || self.0.aliases.get(&*n.id).is_some_and(Option::is_some))
        {
            self.1 = true;
        }
        walk_expr(self, expr);
    }
}

/// A component lookup with a literal name that finds nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedLookup {
    /// The name argument in the script source.
    pub range: TextRange,
    pub message: String,
    /// The lookup call in the script source.
    pub call: TextRange,
    /// `getSibling` or `getChild`.
    pub method: &'static str,
    /// The literal name argument.
    pub name: String,
}

/// Literal `getSibling`/`getChild` lookups of a view script whose base
/// component (or the view) is known and that name no component. Lookups
/// below components that cannot hold children in the view (embedded views,
/// components without a `children` array that are not containers) are
/// not reported.
pub fn unresolved_lookups(unit: &ScriptUnit<'_, '_>, view: &View<'_>) -> Vec<UnresolvedLookup> {
    let this = match unit.script.owner {
        Owner::View => Val::Object(AccessTarget::View),
        Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
        _ => return Vec::new(),
    };
    if !unit.parsed.is_valid() {
        return Vec::new();
    }
    let mut sink = Vec::new();
    let mut a = Analysis {
        view,
        this,
        aliases: FxHashMap::default(),
        bound: FxHashSet::default(),
        out: &mut sink,
    };
    a.collect_aliases(&unit.parsed.module.body);
    let mut out = Vec::new();
    crate::rules::script::walk_exprs(&unit.parsed.module.body, &mut |e| {
        let Expr::Call(c) = e else { return };
        let Expr::Attribute(attr) = &*c.func else {
            return;
        };
        let method = attr.attr.as_str();
        if method != "getSibling" && method != "getChild" {
            return;
        }
        let Some(Expr::Str(lit)) = c.args.first() else {
            return;
        };
        let Some(name) = lit.s.as_str() else { return };
        if c.args.len() != 1 || !c.keywords.is_empty() {
            return;
        }
        let Some(Val::Object(base)) = a.peek(&attr.value) else {
            return;
        };
        if let Some(message) = a.lookup_problem(base, method, name) {
            out.push(UnresolvedLookup {
                range: lit.range,
                message,
                call: c.range,
                method: if method == "getSibling" {
                    "getSibling"
                } else {
                    "getChild"
                },
                name: name.to_owned(),
            });
        }
    });
    out
}

impl Analysis<'_, '_, '_> {
    /// Whether components may be looked up below `c` (it is a container).
    fn holds_children(&self, c: ComponentId) -> bool {
        let component = self.view.component(c);
        component.value.get("children").is_some()
            || component
                .type_id
                .is_some_and(|t| t.starts_with("ia.container."))
    }

    /// Why `base.method(name)` finds nothing, if it certainly does not.
    fn lookup_problem(&self, base: AccessTarget, method: &str, name: &str) -> Option<String> {
        let (start, path, describe) = match (base, method) {
            (AccessTarget::Component(c), "getSibling") => {
                let parent = self.view.component(c).parent?;
                if name.contains('/') {
                    return None;
                }
                (Some(parent), name, "sibling")
            }
            (AccessTarget::Component(c), "getChild") => (Some(c), name, "child"),
            (AccessTarget::View, "getChild") => (None, name, "view path"),
            _ => return None,
        };
        let mut segments = path.split('/').filter(|s| !s.is_empty()).peekable();
        segments.peek()?;
        let mut at = match start {
            Some(c) => c,
            None => {
                let root = self.view.root_component()?;
                // Paths start with the root container's name; other forms
                // are not certain to fail.
                if segments.next()? != root.name {
                    return None;
                }
                root.id
            }
        };
        for segment in segments {
            if !self.holds_children(at) {
                return None;
            }
            match self.view.children(at).find(|c| c.name == segment) {
                Some(c) => at = c.id,
                None => {
                    let owner = self.view.component(at);
                    let hint = self
                        .view
                        .children(at)
                        .find(|c| c.name.eq_ignore_ascii_case(segment))
                        .map(|c| format!(" (names are case-sensitive; did you mean `{}`?)", c.name))
                        .unwrap_or_default();
                    return Some(match describe {
                        "sibling" => format!(
                            "No sibling component named `{segment}` (in `{}`){hint}",
                            owner.path
                        ),
                        _ => format!(
                            "No child component named `{segment}` in `{}`{hint}",
                            owner.path
                        ),
                    });
                }
            }
        }
        None
    }
}

fn e_range(e: &Expr) -> TextRange {
    use iglint_text::Ranged;
    e.range()
}

/// Collects the direct child expressions of a node.
#[derive(Default)]
struct Children<'e>(Vec<&'e Expr>);

impl<'e> Visitor<'e> for Children<'e> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        self.0.push(expr);
    }
}

impl<'e> Visitor<'e> for Analysis<'_, '_, '_> {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                if let [Expr::Name(n)] = s.targets.as_slice()
                    && self.aliases.get(&*n.id).copied().flatten().is_some()
                {
                    // An alias definition: the value is followed, not lost.
                    self.walk(&s.value, Role::Base, false);
                    return;
                }
                for t in &s.targets {
                    self.walk(t, Role::Value, false);
                }
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::AugAssign(s) => {
                self.walk(&s.target, Role::Value, true);
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::Exec(s) => {
                self.everything(s.range);
                walk_stmt(self, stmt);
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.walk(expr, Role::Value, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iglint_project::view::build_view;

    const VIEW: &str = r#"{"custom": {"a": 1}, "params": {"p": 1},
      "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [
        {"meta": {"name": "A"}, "type": "ia.display.label", "custom": {"x": 1}},
        {"meta": {"name": "B"}, "type": "ia.input.button", "custom": {"y": 1},
         "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": ""}}}}}
      ]}}"#;

    /// Accesses of `body` run as the button's action script.
    fn accesses(body: &str) -> Vec<String> {
        let mut json: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        json["root"]["children"][1]["events"]["component"]["onActionPerformed"]["config"]["script"] =
            serde_json::Value::String(body.to_owned());
        let text = serde_json::to_string(&json).unwrap();
        let root = iglint_json::parse(&text).unwrap();
        let view = build_view(&root);
        let script = &view.scripts[0];
        let parsed = crate::script_facts::parse_script(script);
        let unit = ScriptUnit::new(iglint_project::FileId(0), script, &parsed, 0);
        let mut out = Vec::new();
        AstScriptFacts.property_accesses(&unit, &view, &mut out);
        out.iter()
            .map(|a| {
                let t = match a.target {
                    AccessTarget::View => "view".to_owned(),
                    AccessTarget::Component(c) => view.component(c).name.to_owned(),
                    AccessTarget::AnyComponent => "*".to_owned(),
                };
                format!(
                    "{t}.{}.{}{}{}",
                    a.category.name(),
                    a.name.as_deref().unwrap_or("*"),
                    if a.read { " r" } else { "" },
                    if a.write { " w" } else { "" },
                )
            })
            .collect()
    }

    #[test]
    fn direct_accesses() {
        assert_eq!(
            accesses(
                "\tx = self.view.custom.a\n\tself.view.params.p = self.custom.y\n\tself.getSibling('A').custom['x'] += 1"
            ),
            vec![
                "view.custom.a r",
                "view.params.p w",
                "B.custom.y r",
                "A.custom.x r w"
            ]
        );
    }

    #[test]
    fn navigation() {
        assert_eq!(
            accesses(
                "\tself.parent.getChild('A').custom.x\n\tself.view.getChild('root/A').custom.q\n\tself.view.rootContainer.custom.r\n\tself.getSibling('Missing').custom.z"
            ),
            vec!["A.custom.x r", "A.custom.q r", "root.custom.r r"]
        );
        // Iterating over children loses track of the components.
        assert_eq!(
            accesses(
                "\tfor c in self.parent.children:\n\t\tc.custom.x = 1\n\tself.getSibling(name).custom.w"
            ),
            vec![
                "view.custom.* r w",
                "view.params.* r w",
                "*.custom.* r w",
                "*.custom.w r"
            ]
        );
    }

    #[test]
    fn aliases() {
        assert_eq!(
            accesses("\tv = self.view\n\tc = v.custom\n\tc.a = v.params.p"),
            vec!["view.custom.a w", "view.params.p r"]
        );
    }

    #[test]
    fn dynamic_uses_are_conservative() {
        assert_eq!(
            accesses("\tk = 'a'\n\tself.view.custom[k]"),
            vec!["view.custom.* r"]
        );
        assert_eq!(
            accesses("\tdata = dict(self.custom)"),
            vec!["B.custom.* r w"]
        );
        assert_eq!(
            accesses("\tfor k in self.view.custom.keys(): pass"),
            vec!["view.custom.* r"]
        );
        let escaped = accesses("\tproject.util.handle(self)");
        assert!(
            escaped.contains(&"*.custom.* r w".to_owned()),
            "{escaped:?}"
        );
        assert!(escaped.contains(&"view.params.* r w".to_owned()));
        let reassigned = accesses("\tv = self.view\n\tv = other\n\tv.custom.a");
        assert!(
            reassigned.contains(&"view.custom.* r w".to_owned()),
            "{reassigned:?}"
        );
        assert!(accesses("\tx = locals()").len() == 3);
    }

    #[test]
    fn unrelated_code_has_no_accesses() {
        assert!(accesses("\tx = self.props.text\n\ty = 'self.custom.a'\n\t# self.view.custom.a\n\tsystem.perspective.print(self.meta.name)").is_empty());
    }
}
