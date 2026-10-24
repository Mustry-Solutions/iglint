//! The semantic model: scopes, bindings and references.

use iglint_python::ast::{Alias, Expr, ExprLambda, Module, Stmt, StmtClassDef, StmtFunctionDef};
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub(crate) u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id!(
    /// A scope of a [`SemanticModel`].
    ScopeId
);
id!(
    /// A binding of a [`SemanticModel`].
    BindingId
);
id!(
    /// A reference of a [`SemanticModel`].
    ReferenceId
);

impl ScopeId {
    /// The module scope is always the first scope.
    pub const MODULE: ScopeId = ScopeId(0);
}

/// What kind of namespace a scope is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScopeKind {
    Module,
    /// A class body. Its names are not visible to functions nested in it.
    Class,
    Function,
    Lambda,
    /// A generator expression, set comprehension or dict comprehension. (A
    /// list comprehension has no scope of its own in Python 2: its variables
    /// leak into the enclosing scope.)
    Generator,
}

impl ScopeKind {
    /// Function, lambda and generator scopes: names bound in them are fast
    /// locals, and reading one before it is bound raises `UnboundLocalError`.
    pub fn is_function_like(self) -> bool {
        matches!(
            self,
            ScopeKind::Function | ScopeKind::Lambda | ScopeKind::Generator
        )
    }
}

/// The syntax node that opens a scope.
#[derive(Clone, Copy, Debug)]
pub enum ScopeNode<'a> {
    Module(&'a Module),
    Class(&'a StmtClassDef),
    Function(&'a StmtFunctionDef),
    Lambda(&'a ExprLambda),
    /// A `GeneratorExp`, `SetComp` or `DictComp` expression.
    Generator(&'a Expr),
}

impl ScopeNode<'_> {
    /// The address of the node, used to find a node's scope.
    pub(crate) fn key(&self) -> usize {
        match self {
            ScopeNode::Module(n) => *n as *const Module as usize,
            ScopeNode::Class(n) => *n as *const StmtClassDef as usize,
            ScopeNode::Function(n) => *n as *const StmtFunctionDef as usize,
            ScopeNode::Lambda(n) => *n as *const ExprLambda as usize,
            ScopeNode::Generator(n) => *n as *const Expr as usize,
        }
    }
}

/// One namespace.
#[derive(Debug)]
pub struct Scope<'a> {
    pub kind: ScopeKind,
    pub node: ScopeNode<'a>,
    pub parent: Option<ScopeId>,
    pub children: Vec<ScopeId>,
    /// The scope's name: the function or class name, `<lambda>`,
    /// `<genexpr>`, or `<module>`.
    pub name: &'a str,
    /// Names bound in this scope (assigned, imported, defined, deleted,
    /// parameters), in order of first appearance. For the module this
    /// includes names that functions bind through `global`.
    pub(crate) locals: Vec<&'a str>,
    pub(crate) local_index: FxHashMap<&'a str, u32>,
    /// `global` declarations: name and first declaration.
    pub globals: FxHashMap<&'a str, TextRange>,
    /// Bindings by name, in source order.
    pub(crate) bindings: FxHashMap<&'a str, Vec<BindingId>>,
    /// `global` declarations as [`BindingKind::Global`] bindings.
    pub global_declarations: Vec<BindingId>,
    /// The first `from x import *` of the scope.
    pub star_import: Option<TextRange>,
    /// The first `exec` statement without `in` (Python 2 allows it to bind
    /// any local name).
    pub unqualified_exec: Option<TextRange>,
    /// The scope calls `locals()`, `vars()`, `eval(...)` or `dir()` (so
    /// every local may be read by name).
    pub reads_locals: bool,
    /// The scope writes through `globals()` or `vars()`
    /// (`globals()['x'] = 1`, `globals().update(...)`).
    pub writes_globals: bool,
    /// The scope contains `yield` (it is a generator function).
    pub is_generator: bool,
    /// For functions and lambdas: whether execution can reach the end of the
    /// body without `return` or `raise` (the function then returns `None`).
    pub can_fall_through: bool,
    /// Names read or written in this scope that are not bound in it and not
    /// declared global (Jython's notion of free variables).
    pub(crate) free_names: Vec<(&'a str, TextRange)>,
    /// Jython's symbol table of the scope (`org.python.compiler.ScopeInfo`
    /// after `cook` and `setup_closure`): flags by name as spelled in the
    /// source (the symbol table pass does not mangle), see
    /// [`crate::symbols::jython`].
    pub(crate) jython: FxHashMap<&'a str, u8>,
    /// The scope contains an `exec` statement (with or without `in`):
    /// Jython then reads names the scope does not bind through the frame
    /// (`getname`) instead of the module's globals.
    pub has_exec: bool,
    /// The class name Jython mangles private names with in this scope's
    /// code (attribute names, `def` and `class` names, and in a class body
    /// every name): the scope's own run-time name for a class body (itself
    /// mangled when the class is nested in a class), the innermost
    /// enclosing class's otherwise. `None` outside classes.
    pub class_name: Option<&'a str>,
}

impl<'a> Scope<'a> {
    /// Whether `name` is bound in this scope (and is not declared global).
    pub fn is_local(&self, name: &str) -> bool {
        self.local_index.contains_key(name)
    }

    /// Names bound in the scope, in order of first appearance.
    pub fn local_names(&self) -> &[&'a str] {
        &self.locals
    }

    /// Every binding of `name` in this scope, in source order.
    pub fn bindings_of(&self, name: &str) -> &[BindingId] {
        self.bindings.get(name).map_or(&[], Vec::as_slice)
    }

    /// Every binding of the scope, grouped by name.
    pub fn all_bindings(&self) -> impl Iterator<Item = (&'a str, &[BindingId])> + '_ {
        self.bindings
            .iter()
            .map(|(name, ids)| (*name, ids.as_slice()))
    }

    /// Whether names that the code does not bind may still exist at run
    /// time: `from x import *`, unqualified `exec`, writes through
    /// `globals()`.
    pub fn is_dynamic(&self) -> bool {
        self.star_import.is_some() || self.unqualified_exec.is_some() || self.writes_globals
    }

    /// Names that are free in this scope (Jython's definition: used here but
    /// not bound here and not declared global), with their first use.
    pub fn free_names(&self) -> &[(&'a str, TextRange)] {
        &self.free_names
    }

    /// How a name spelled `source` in this scope's code is named at run
    /// time: mangled in a class body (`__x` is `_C__x` in class `C`, see
    /// [`crate::mangle`]), unchanged elsewhere.
    pub fn runtime_name<'s>(&self, source: &'s str) -> &'s str {
        match self.class_name {
            Some(class) if self.kind == ScopeKind::Class => crate::mangle::mangle(class, source),
            _ => source,
        }
    }

    /// An attribute name as this scope's code spells it at run time
    /// (`obj.__x` is `obj._C__x` anywhere inside class `C`).
    pub fn runtime_attribute<'s>(&self, attribute: &'s str) -> &'s str {
        match self.class_name {
            Some(class) => crate::mangle::mangle(class, attribute),
            None => attribute,
        }
    }

    /// Whether `name` (a run-time spelling, see [`Scope::runtime_name`]) is
    /// declared `global` for this scope's code. In a class body Jython
    /// checks the mangled spelling against the unmangled declarations.
    pub fn declares_global(&self, name: &str) -> bool {
        self.globals.contains_key(name)
    }
}

/// How the code Jython generates for a scope reaches a name: which frame
/// accessor a read, store or delete compiles to. Measured with
/// `oracle/scope/` (the `scope_oracle` test).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameAccess {
    /// By name in the frame's namespaces (`getname`, `setlocal(String)`,
    /// `dellocal(String)`): module and class bodies, and reads in a
    /// function with `exec` of names it does not bind. A read looks in the
    /// local namespace (the class body's, or the frame's), then the
    /// module's globals, then the builtins, whatever the code binds later.
    Name,
    /// In the module's globals, then the builtins (`getglobal`,
    /// `setglobal`, `delglobal`), because the scope (or, for a free name,
    /// the nearest enclosing function) declares the name `global`.
    Global,
    /// Like [`NameAccess::Global`], for a name the function neither binds
    /// nor declares (Jython optimizes the lookup when the function has no
    /// `exec`).
    ImplicitGlobal,
    /// A fast local of a function (`getlocal(int)`).
    Fast,
    /// A local of a function that nested scopes read (`getderef`,
    /// `setderef` on one of the function's cells).
    Cell,
    /// A variable of an enclosing function (`getderef` on a free variable).
    Free,
}

impl NameAccess {
    pub fn name(self) -> &'static str {
        match self {
            NameAccess::Name => "name",
            NameAccess::Global => "global",
            NameAccess::ImplicitGlobal => "global",
            NameAccess::Fast => "fast",
            NameAccess::Cell => "cell",
            NameAccess::Free => "free",
        }
    }
}

/// A name the interpreter reads without the code naming it.
///
/// Measured on Ignition 8.3.3's Jython, by compiling (`oracle/scope/`) and
/// by running statements with a module namespace that records every
/// lookup (`oracle/scope/jython_implicit_reads_oracle.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImplicitRead {
    /// A class body starts by reading `__name__` by name (its
    /// `__module__`): the class namespace, then the globals, then the
    /// builtins.
    ClassModule,
    /// A class statement without bases reads `__metaclass__` from the
    /// class namespace, then from the module's globals, once the body has
    /// run.
    Metaclass,
    /// Creating a function (`def`, `lambda`, generator expression, set or
    /// dict comprehension) reads `__name__` from the module's globals (its
    /// `__module__`).
    FunctionModule,
    /// An import statement reads `__name__`, `__package__` and `__path__`
    /// from the module's globals to find the importing package.
    Import,
    /// `assert` reads `__debug__` and, when the test fails,
    /// `AssertionError` from the module's globals, then the builtins
    /// (compiled as `getglobal` in every scope).
    Assert,
}

/// What created a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    /// `import a.b.c` (binds `a`) or `import a.b as c`.
    Import,
    /// `from m import x` or `from m import x as y`.
    FromImport,
    /// `from __future__ import x`.
    FutureImport,
    /// `x = ...` and other plain assignments, including names unpacked from
    /// a tuple target.
    Assignment,
    /// `x += ...`.
    AugAssignment,
    /// A parameter, including names inside tuple parameters.
    Parameter,
    /// `def x(...)`.
    FunctionDef,
    /// `class X`.
    ClassDef,
    /// The target of a `for` statement.
    LoopVar,
    /// The target of a comprehension (`[x for x in y]`).
    ComprehensionVar,
    /// `except E, x:`.
    ExceptVar,
    /// `with ctx as x:`.
    WithVar,
    /// A `global x` declaration (kept in [`Scope::global_declarations`];
    /// the assignments it affects bind in the module scope).
    Global,
    /// A name of Jython's `__builtin__` module.
    Builtin,
    /// A name the caller injected (Ignition's implicit globals).
    Injected,
    /// `__module__`, implicitly bound in every class body.
    ClassModule,
    /// `__package__`, which an import statement binds in the module's
    /// globals when it runs (see [`crate::SemanticOptions::module_has_name`]).
    /// The binding's `stmt` is the import, its range the `import` or `from`
    /// keyword, its scope the module and its site the scope of the import.
    ImportPackage,
}

impl BindingKind {
    pub fn is_import(self) -> bool {
        matches!(
            self,
            BindingKind::Import | BindingKind::FromImport | BindingKind::FutureImport
        )
    }

    /// Bindings that define a name in a way the definition alone matters:
    /// functions, classes, imports.
    pub fn is_definition(self) -> bool {
        matches!(
            self,
            BindingKind::Import
                | BindingKind::FromImport
                | BindingKind::FunctionDef
                | BindingKind::ClassDef
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            BindingKind::Import => "import",
            BindingKind::FromImport => "from-import",
            BindingKind::FutureImport => "future-import",
            BindingKind::Assignment => "assignment",
            BindingKind::AugAssignment => "augmented-assignment",
            BindingKind::Parameter => "parameter",
            BindingKind::FunctionDef => "function",
            BindingKind::ClassDef => "class",
            BindingKind::LoopVar => "loop-variable",
            BindingKind::ComprehensionVar => "comprehension-variable",
            BindingKind::ExceptVar => "except-variable",
            BindingKind::WithVar => "with-variable",
            BindingKind::Global => "global",
            BindingKind::Builtin => "builtin",
            BindingKind::Injected => "injected",
            BindingKind::ClassModule => "class-module",
            BindingKind::ImportPackage => "import-package",
        }
    }
}

/// A name bound to a value.
#[derive(Debug)]
pub struct Binding<'a> {
    /// The name as it is bound at run time: mangled in class bodies
    /// (`_C__x` for `__x` in class `C`).
    pub name: &'a str,
    /// The name as the source spells it (what Jython's symbol table pass
    /// sees).
    pub source_name: &'a str,
    pub kind: BindingKind,
    /// The bound name in the source (the alias for imports, the name of a
    /// `def` or `class`). Empty at offset 0 for builtins and injected names.
    pub range: TextRange,
    /// The namespace the name is bound in. Differs from `site` for
    /// assignments under `global`.
    pub scope: ScopeId,
    /// The scope whose code performs the binding.
    pub site: ScopeId,
    /// The statement that binds the name, if any.
    pub stmt: Option<&'a Stmt>,
    /// The import alias for imports.
    pub alias: Option<&'a Alias>,
    /// For `x = value` with a single name target: the value.
    pub value: Option<&'a Expr>,
    /// For imports: the dotted name the binding refers to (`a` for `import
    /// a.b`, `a.b` for `import a.b as c`, `m.x` for `from m import x`;
    /// relative modules keep their leading dots).
    pub qualified: Option<String>,
    /// The name is part of a tuple or list target (`a, b = f()`) or a tuple
    /// parameter.
    pub unpacked: bool,
    /// An import inside a `try` whose handlers catch `ImportError` (an
    /// availability check).
    pub import_guarded: bool,
    /// The `try` statements around the binding (innermost last): the range
    /// of each `try` statement and the part the binding is in.
    pub try_parts: Vec<(TextRange, TryPart)>,
    /// Bindings of the same name that may still be live where this one
    /// replaces them (the reaching definitions just before it). Only for
    /// names local to the binding's scope.
    pub shadowed: Vec<BindingId>,
    /// References that may read this binding.
    pub references: Vec<ReferenceId>,
    /// How the code of `site` stores the name (for bindings the source
    /// names; [`NameAccess::Name`] for implicit ones).
    pub access: NameAccess,
}

impl Binding<'_> {
    /// Whether any reference may read this binding.
    pub fn is_used(&self) -> bool {
        !self.references.is_empty()
    }

    /// Builtins, injected names and names bound without being named in
    /// the source (`__module__`, `__package__`).
    pub fn is_implicit(&self) -> bool {
        matches!(
            self.kind,
            BindingKind::Builtin
                | BindingKind::Injected
                | BindingKind::ClassModule
                | BindingKind::ImportPackage
        )
    }
}

/// A part of a `try` statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TryPart {
    Body,
    Handler,
    Else,
    Finally,
}

/// How a name was used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceKind {
    Load,
    /// `del x`.
    Del,
    /// The read half of `x += 1`.
    AugLoad,
    /// A read the interpreter makes without the code naming the name. Its
    /// range is the keyword (or expression) that makes it. Not a use the
    /// author wrote: rules about names in the source skip it, but it keeps
    /// the bindings it reads alive.
    Implicit(ImplicitRead),
}

/// How a reference was resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// A name bound in the reference's own scope. The bindings are those
    /// that reach the reference on some path.
    Local,
    /// A name bound in an enclosing function (a closure variable).
    Free(ScopeId),
    /// A module-level name.
    Global,
    /// A name of Jython's `__builtin__` module.
    Builtin,
    /// A name injected by the caller.
    Injected,
    /// The name is not bound anywhere visible, but the scope or the module
    /// is dynamic (`from x import *`, unqualified `exec`), so it may exist.
    Dynamic,
    /// The name is not bound anywhere: `NameError` at run time.
    Unresolved,
}

/// A read (or `del`) of a name.
#[derive(Debug)]
pub struct Reference<'a> {
    /// The name as it is looked up at run time: mangled in class bodies
    /// (`_C__x` for `__x` in class `C`); the source spelling is the text at
    /// `range`.
    pub name: &'a str,
    /// The name as the source spells it (what Jython's symbol table pass
    /// sees).
    pub source_name: &'a str,
    pub range: TextRange,
    pub kind: ReferenceKind,
    /// The scope whose code contains the reference.
    pub scope: ScopeId,
    pub resolution: Resolution,
    /// The bindings the reference may read.
    pub bindings: Vec<BindingId>,
    /// For local names: no binding reaches the reference on any path (the
    /// read always fails if executed). Always false for other resolutions.
    pub unbound: bool,
    /// For local names: some path reaches the reference without a binding.
    pub maybe_unbound: bool,
    /// The reference is on a path that can execute.
    pub reachable: bool,
    /// The reference is inside a `try` body whose handlers catch
    /// `NameError` (or `UnboundLocalError`).
    pub name_error_guarded: bool,
    /// How the scope's code reads (or deletes) the name.
    pub access: NameAccess,
    /// For a read by name in a class body of a name the class binds: where
    /// the lookup continues when the class namespace does not hold the
    /// name ([`Resolution::Global`], [`Resolution::Builtin`],
    /// [`Resolution::Injected`], [`Resolution::Dynamic`] or
    /// [`Resolution::Unresolved`]). Flow-insensitive: the read may find
    /// any of those bindings too, whatever the class binds before it, and
    /// they are in `bindings`.
    pub fallback: Option<Resolution>,
}

impl Reference<'_> {
    /// Whether the interpreter makes the read without the code naming it
    /// ([`ReferenceKind::Implicit`]).
    pub fn is_implicit(&self) -> bool {
        matches!(self.kind, ReferenceKind::Implicit(_))
    }
}

/// The analysis of one module.
#[derive(Debug)]
pub struct SemanticModel<'a> {
    pub module: &'a Module,
    pub(crate) scopes: Vec<Scope<'a>>,
    pub(crate) bindings: Vec<Binding<'a>>,
    pub(crate) references: Vec<Reference<'a>>,
    pub(crate) scope_of_node: FxHashMap<usize, ScopeId>,
    /// Builtin and injected bindings by name (created on first use).
    pub(crate) implicit: FxHashMap<&'a str, BindingId>,
    /// Statements that execution can never reach, with the
    /// statement before them (whose completion makes them unreachable).
    pub(crate) unreachable: Vec<(&'a Stmt, &'a Stmt)>,
    /// Reference ids by name range start.
    pub(crate) reference_at: FxHashMap<u32, ReferenceId>,
    /// `__all__` entries of the module (literal lists and tuples of strings).
    pub(crate) dunder_all: Vec<String>,
    /// `__all__` is assigned something we cannot read.
    pub(crate) dunder_all_dynamic: bool,
}

impl<'a> SemanticModel<'a> {
    pub fn scope(&self, id: ScopeId) -> &Scope<'a> {
        &self.scopes[id.index()]
    }

    pub fn scopes(&self) -> impl Iterator<Item = (ScopeId, &Scope<'a>)> {
        self.scopes
            .iter()
            .enumerate()
            .map(|(i, s)| (ScopeId(i as u32), s))
    }

    pub fn binding(&self, id: BindingId) -> &Binding<'a> {
        &self.bindings[id.index()]
    }

    pub fn bindings(&self) -> impl Iterator<Item = (BindingId, &Binding<'a>)> {
        self.bindings
            .iter()
            .enumerate()
            .map(|(i, b)| (BindingId(i as u32), b))
    }

    pub fn reference(&self, id: ReferenceId) -> &Reference<'a> {
        &self.references[id.index()]
    }

    pub fn references(&self) -> impl Iterator<Item = (ReferenceId, &Reference<'a>)> {
        self.references
            .iter()
            .enumerate()
            .map(|(i, r)| (ReferenceId(i as u32), r))
    }

    /// The reference made by the `Name` node at `range`, if any.
    pub fn reference_at(&self, range: TextRange) -> Option<&Reference<'a>> {
        let id = self.reference_at.get(&range.start().to_u32())?;
        let reference = self.reference(*id);
        (reference.range == range).then_some(reference)
    }

    /// The scope opened by a function definition.
    pub fn function_scope(&self, function: &StmtFunctionDef) -> Option<ScopeId> {
        self.scope_of_node
            .get(&(function as *const StmtFunctionDef as usize))
            .copied()
    }

    /// The scope opened by a class definition.
    pub fn class_scope(&self, class: &StmtClassDef) -> Option<ScopeId> {
        self.scope_of_node
            .get(&(class as *const StmtClassDef as usize))
            .copied()
    }

    /// The scope opened by a lambda.
    pub fn lambda_scope(&self, lambda: &ExprLambda) -> Option<ScopeId> {
        self.scope_of_node
            .get(&(lambda as *const ExprLambda as usize))
            .copied()
    }

    /// The scope opened by a generator expression or set/dict comprehension.
    pub fn generator_scope(&self, expr: &Expr) -> Option<ScopeId> {
        self.scope_of_node
            .get(&(expr as *const Expr as usize))
            .copied()
    }

    /// Whether execution can reach the end of `function`'s body (it then
    /// returns `None`). False when every path ends in `return` or `raise`
    /// or loops forever.
    pub fn function_can_fall_through(&self, function: &StmtFunctionDef) -> bool {
        self.function_scope(function)
            .is_some_and(|id| self.scope(id).can_fall_through)
    }

    /// Statements that can never execute because the statement before them
    /// always returns, raises, breaks, continues or loops forever: `(first
    /// unreachable statement, statement before it)`. Only the first
    /// statement of each unreachable run is listed.
    pub fn unreachable_statements(&self) -> &[(&'a Stmt, &'a Stmt)] {
        &self.unreachable
    }

    /// The binding of a builtin or injected name, if some reference used it.
    pub fn implicit_binding(&self, name: &str) -> Option<BindingId> {
        self.implicit.get(name).copied()
    }

    /// Strings listed in the module's `__all__`.
    pub fn dunder_all(&self) -> &[String] {
        &self.dunder_all
    }

    /// Whether `__all__` is built in a way we cannot read.
    pub fn dunder_all_is_dynamic(&self) -> bool {
        self.dunder_all_dynamic
    }

    /// Whether the scope or any enclosing scope can bind names the code
    /// does not show.
    pub fn is_dynamic_chain(&self, mut id: ScopeId) -> bool {
        loop {
            let scope = self.scope(id);
            if scope.is_dynamic() {
                return true;
            }
            match scope.parent {
                Some(parent) => id = parent,
                None => return false,
            }
        }
    }

    /// The innermost scope enclosing `scope` that is a function or the
    /// module (class scopes are skipped, as Python does for name lookup).
    pub fn enclosing_lookup_scope(&self, scope: ScopeId) -> Option<ScopeId> {
        let mut at = self.scope(scope).parent?;
        loop {
            if self.scope(at).kind != ScopeKind::Class {
                return Some(at);
            }
            at = self.scope(at).parent?;
        }
    }
}
