//! The project script library (`ignition/script-python`, format spec 4):
//! module models, the symbol summary filled in by the Python layer, and
//! dotted-name resolution with Ignition 8.3 rules.

use rustc_hash::FxHashMap;

use iglint_text::TextRange;

use crate::ids::{FileId, ResourceId};
use crate::script::EmbeddedScript;

/// A library module (`code.py`).
#[derive(Debug)]
pub struct LibraryModule<'a> {
    /// Dotted module name, e.g. `core.format`.
    pub module_path: String,
    pub file: FileId,
    /// The module source as written.
    pub script: EmbeddedScript<'a>,
    /// `attributes.hintScope` (Designer hints only).
    pub hint_scope: Option<i64>,
}

/// True when every path component matches `[_a-zA-Z]\w*`
/// (`ModuleLibrary.NAME_PATTERN`).
pub fn is_valid_module_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|seg| {
            let mut chars = seg.chars();
            chars
                .next()
                .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
                && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        })
}

/// The Java members of Ignition's project library module objects
/// (`ProjectScriptModule`), as Jython finds them when the module does not
/// bind the name itself (a name the module binds wins). Measured on 8.3.3
/// by `oracle/jython_library_routes_oracle.py` (replayed by
/// `library_object_members_match_the_oracle` in `iglint_linter`). `dict`
/// and `getDict()` hand out the module's namespace (its `__builtins__`
/// too), `setCode` replaces its code and `reloadModule` runs it again: a
/// use of any of them is a use of the module as a value.
pub const MODULE_OBJECT_MEMBERS: &[&str] = &[
    "code",
    "dict",
    "getCode",
    "getDict",
    "getName",
    "getPath",
    "loadModule",
    "name",
    "path",
    "reloadModule",
    "setCode",
    "setName",
    "toString",
];

/// The Java members of Ignition's library package objects
/// (`ScriptPackage`), found when the package has no child of that name (a
/// child wins). Measured like [`MODULE_OBJECT_MEMBERS`]: `dict` and
/// `getDict()` hand out the package's modules, `clear()` and `setDict`
/// replace them.
pub const PACKAGE_OBJECT_MEMBERS: &[&str] = &[
    "clear", "dict", "getDict", "getName", "name", "setDict", "setName", "toString",
];

/// Attributes of library modules, packages, functions and classes whose
/// reading exposes nothing: names and documentation.
pub const HARMLESS_ATTRIBUTES: &[&str] =
    &["__name__", "func_name", "__doc__", "func_doc", "__module__"];

/// Whether the attribute `name` of a library object is something other
/// than one of its names, whatever the object: a special attribute
/// (`__dict__`, `__getattribute__`, `__class__`, `__reduce_ex__`,
/// `__setattr__`, ...), other than the [`HARMLESS_ATTRIBUTES`]. Default
/// deny: every special name counts, so that a route the measurement did
/// not list is still a use of the object as a value.
pub fn is_special_attribute(name: &str) -> bool {
    name.len() > 4
        && name.starts_with("__")
        && name.ends_with("__")
        && !HARMLESS_ATTRIBUTES.contains(&name)
}

/// The attributes of Jython's function, method, class, instance and
/// generator objects that are not special names (`func_globals`,
/// `im_func`, `gi_frame`, `mro`, ...), measured on 8.3.3 by
/// `oracle/jython_library_routes_oracle.py` (replayed by
/// `library_object_members_match_the_oracle` in `iglint_linter`). Their
/// Java members are not attributes (exposed types).
pub const OBJECT_MEMBERS: &[&str] = &[
    "close",
    "func_closure",
    "func_code",
    "func_defaults",
    "func_dict",
    "func_doc",
    "func_globals",
    "func_name",
    "gi_code",
    "gi_frame",
    "gi_running",
    "im_class",
    "im_func",
    "im_self",
    "mro",
    "next",
    "send",
    "throw",
];

/// Whether the attribute `name`, read from an object a library module
/// defines or holds, may hand out something other than a plain value:
/// a special attribute ([`is_special_attribute`]), a member of Jython's
/// function, method, class, instance or generator objects
/// ([`OBJECT_MEMBERS`]) or of Ignition's library objects
/// ([`MODULE_OBJECT_MEMBERS`], [`PACKAGE_OBJECT_MEMBERS`]), or a name
/// shaped like the attributes of frames, code objects and tracebacks
/// (`f_`, `co_`, `tb_`, and `func_`, `im_`, `gi_`). Names and
/// documentation ([`HARMLESS_ATTRIBUTES`]) are plain.
pub fn is_value_attribute(name: &str) -> bool {
    !HARMLESS_ATTRIBUTES.contains(&name)
        && (is_special_attribute(name)
            || OBJECT_MEMBERS.contains(&name)
            || MODULE_OBJECT_MEMBERS.contains(&name)
            || PACKAGE_OBJECT_MEMBERS.contains(&name)
            || ["f_", "co_", "tb_", "func_", "im_", "gi_"]
                .iter()
                .any(|p| name.starts_with(p)))
}

/// [`is_value_attribute`] after a class, a variable or an import of a
/// module the module knows: the members of Ignition's library objects
/// (`dict`, `clear`, `name`, ...) do not count there. Such a value holds a
/// library module or package only if code put it there as a value
/// (`alias = core.util`, `C.x = core`, `getattr(core, 'util')`), and that
/// use counts by itself; any other value's `clear` or `name` is its own
/// (`CACHE.clear()` of a dict).
fn reaches_from_value(name: &str) -> bool {
    is_value_attribute(name)
        && !MODULE_OBJECT_MEMBERS.contains(&name)
        && !PACKAGE_OBJECT_MEMBERS.contains(&name)
}

/// The library object (a module or a package) whose own member a dotted
/// name reaches ([`LibraryTree::object_member`]): code that reads it may
/// read and change everything in the object.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LibraryObject {
    Module {
        resource: ResourceId,
        dotted: String,
    },
    Package {
        dotted: String,
    },
}

impl LibraryObject {
    /// The dotted name of the module or package.
    pub fn dotted(&self) -> &str {
        match self {
            LibraryObject::Module { dotted, .. } | LibraryObject::Package { dotted } => dotted,
        }
    }
}

/// Whether `name` is a member of a library module object.
fn module_member(name: &str) -> bool {
    MODULE_OBJECT_MEMBERS.contains(&name) || is_special_attribute(name)
}

/// Whether `name` is a member of a library package object.
fn package_member(name: &str) -> bool {
    PACKAGE_OBJECT_MEMBERS.contains(&name) || is_special_attribute(name)
}

/// Whether `name` is a special name (`__doc__`, `__name__`, `__file__`):
/// the objects have some of them (measured), which lookups do not
/// decide.
fn dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// Top-level definitions of a module, produced by the Python layer from its
/// parse tree. iglint_project never parses Python itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleSymbols {
    pub symbols: Vec<Symbol>,
    /// True when the module's namespace cannot be known statically
    /// (`from x import *`, `exec`, writes to `globals()`): lookups of
    /// unknown names are then [`LibraryLookup::Unknown`].
    pub dynamic: bool,
    /// Top-level variables bound only to literals (`LIMIT = 10`,
    /// `NAMES = ['a', 'b']`): their value is never a function.
    pub constants: Vec<String>,
    /// The module uses something that is not vetted (a module, Java
    /// class, builtin or module attribute; see
    /// `iglint_python_semantic::vetted`): safe fixes that depend on what
    /// code can read do not trust it.
    pub unknown_code: bool,
    /// Imports of modules that are not vetted, which are fine only when
    /// they are plain project library modules.
    pub imports: Vec<String>,
    /// `system.*` chains the module uses that the 8.3 stubs do not list:
    /// not vetted unless the project vets them
    /// ([`crate::index::Vetted`]).
    pub system: Vec<String>,
    /// Dotted names the module uses that may be library names.
    pub library_names: Vec<String>,
    /// The modules its module-level `from x import *` statements name.
    pub star_imports: Vec<String>,
    /// The module is [`ModuleSymbols::dynamic`] only because of
    /// module-level `from x import *` statements of absolute modules
    /// (no `exec`, namespace writes or frame routes, no relative star
    /// import): a name it does not bind can only come from the modules
    /// [`ModuleSymbols::star_imports`] names.
    pub names_from_stars_only: bool,
    /// The names `from <this module> import *` binds.
    pub star_exports: StarExports,
    /// The import bindings of top-level names that are not a
    /// [`SymbolKind::Import`] (bound several times, or possibly rebound):
    /// the name may hold the import's target.
    pub variable_imports: Vec<VariableImport>,
    /// The symbols for findings, when they differ: the module has an
    /// escape route that does not observe names by itself (a reflective
    /// Java member such as `.type`), which makes every name a variable and
    /// the namespace dynamic here, for fixes, but not for findings
    /// ([`ModuleSymbols::for_findings`]).
    pub findings: Option<Box<ModuleSymbols>>,
    /// Where the last definition of each name is in `symbols`, built on
    /// the first [`ModuleSymbols::get`] (review round 25, M3: a linear
    /// search per lookup made the index quadratic in the number of
    /// top-level names). Not part of the value: it ignores comparison and
    /// `Debug`, and `symbols` must not change after the first lookup.
    pub names: NameIndex,
}

/// What `from <module> import *` binds (Python 2.7), by the module's
/// `__all__` ([`ModuleSymbols::star_exports`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StarExports {
    /// The module has no `__all__`: every top-level name not starting
    /// with `_`.
    #[default]
    Public,
    /// The names `__all__` lists, read exactly (one assignment of literal
    /// names in the module's own statement list, and module-level
    /// `append` and `extend` calls of literal names).
    Listed(Vec<String>),
    /// `__all__` may hold names the code does not show (review round 29,
    /// N1): bound several times or conditionally, changed through a
    /// subscript or slice, a method other than a literal `append` or
    /// `extend`, an augmented assignment, an alias or from a function, or
    /// the module writes its namespace dynamically. Default deny: any
    /// name, also one starting with `_`.
    Any,
}

impl StarExports {
    /// Whether `from <module> import *` may bind `name`.
    pub fn exports(&self, name: &str) -> bool {
        match self {
            StarExports::Public => !name.starts_with('_'),
            StarExports::Listed(names) => names.iter().any(|n| n == name),
            StarExports::Any => true,
        }
    }
}

/// An import binding of a top-level name that is a
/// [`SymbolKind::Variable`] ([`ModuleSymbols::variable_imports`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableImport {
    pub name: String,
    /// The dotted source of the import (`lib.util` for `from lib import
    /// util`).
    pub target: String,
    /// The module binds the name otherwise too, or rebinds it in a way
    /// its bindings show (`try: from lib import util` / `except: util =
    /// None`, a later star import): what the name holds is not known. When
    /// false, the import is the name's only binding and only code the
    /// module runs from strings or reaches through frames may rebind it
    /// (such code is unknown code, or records what it binds).
    pub several: bool,
}

/// See [`ModuleSymbols::names`].
#[derive(Clone, Default)]
pub struct NameIndex(std::sync::OnceLock<FxHashMap<String, usize>>);

impl PartialEq for NameIndex {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NameIndex {}

impl std::fmt::Debug for NameIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NameIndex")
    }
}

impl ModuleSymbols {
    /// The symbols rules that report findings use (see
    /// [`ModuleSymbols::findings`]). Fixes use `self`.
    pub fn for_findings(&self) -> &ModuleSymbols {
        self.findings.as_deref().unwrap_or(self)
    }

    /// The last definition of `name` (later definitions win).
    pub fn get(&self, name: &str) -> Option<&Symbol> {
        let names = self.names.0.get_or_init(|| {
            let mut names = FxHashMap::default();
            for (i, s) in self.symbols.iter().enumerate() {
                names.insert(s.name.clone(), i);
            }
            names
        });
        self.symbols.get(*names.get(name)?)
    }
}

/// A top-level name of a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Range of the definition (name) in the module file.
    pub range: TextRange,
}

/// What a top-level name is bound to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function(Signature),
    Class,
    /// Assignment, `for` target, `with` target, ...
    Variable,
    /// `import x` / `from x import y`; `target` is the dotted source when
    /// known.
    Import {
        target: Option<String>,
    },
}

/// A function signature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub params: Vec<Parameter>,
    /// How the function uses each of `params` (same order) when it is a
    /// Perspective object: what a view script hands over when it calls
    /// `lib.f(self)`. Empty when unknown (every parameter is
    /// [`ParamUse::Dynamic`]).
    pub uses: Vec<ParamUse>,
}

impl Signature {
    /// The use of the parameter an argument binds to: the `index`-th
    /// positional argument, or the keyword argument `keyword`. `Dynamic`
    /// for `*args` / `**kwargs` and unknown parameters.
    pub fn use_of(&self, slot: &ArgSlot) -> &ParamUse {
        const DYNAMIC: &ParamUse = &ParamUse::Dynamic;
        const UNUSED: &ParamUse = &ParamUse::Paths(Vec::new());
        let position = match slot {
            // Calling a function binds no value to a parameter.
            ArgSlot::Callee => return UNUSED,
            ArgSlot::Position(i) => {
                let named = self
                    .params
                    .iter()
                    .take_while(|p| p.kind == ParameterKind::Positional)
                    .count();
                (*i < named).then_some(*i)
            }
            ArgSlot::Keyword(k) => self
                .params
                .iter()
                .position(|p| p.kind == ParameterKind::Positional && p.name == *k),
        };
        position.and_then(|i| self.uses.get(i)).unwrap_or(DYNAMIC)
    }
}

/// Which parameter an argument of a call binds to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArgSlot {
    Position(usize),
    Keyword(String),
    /// No parameter: the callee itself is checked. A name that is not a
    /// plain `def` function (or class) may be a builtin such as `eval` or
    /// `locals` that reads the caller's frame, where every followed value
    /// can be reached.
    Callee,
}

/// What a function does with one parameter, followed like `self` in view
/// scripts (see `iglint_linter::script_facts::access`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ParamUse {
    /// Every use of the parameter is one of these paths (none: unused).
    Paths(Vec<ParamPath>),
    /// Used in a way that cannot be followed (rebound, used in a nested
    /// function, `exec`, `locals()`, ...): it may access anything.
    #[default]
    Dynamic,
}

/// One use of a parameter: the attribute, method and subscript steps from
/// the parameter, and what happens to the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamPath {
    pub steps: Vec<PathStep>,
    pub end: PathEnd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathStep {
    /// `.name`
    Attr(String),
    /// `.name(...)`; `literal` is the first argument when it is a string
    /// literal.
    Method {
        name: String,
        literal: Option<String>,
    },
    /// `[key]`; `None` for a key that is not a string literal.
    Index(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathEnd {
    /// The value is read and used (stored, returned, compared, ...).
    Load,
    /// The last step is assigned or deleted.
    Store,
    /// The last step is updated (`+=`).
    Update,
    /// The value is passed to `callee` (a dotted name as written: a
    /// top-level name of the module or a library name). With
    /// [`ArgSlot::Callee`] and no steps, the function calls (or uses as a
    /// value) `callee` while the parameter is live: when `callee` is not a
    /// plain function it may read the parameter through the frame.
    Passed { callee: String, slot: ArgSlot },
    /// The value is bound to a local name that is assigned only here and
    /// whose every use is itself recorded as a path: the steps are
    /// evaluated (a property container step reads that property), the
    /// value is not lost.
    Alias,
    /// `'name' in value` (or `not in`): reads property `name` of a
    /// container.
    Contains(String),
    /// Only the truth value or identity of the value is used (`if v:`,
    /// `not v`, `v is None`): a container is read, a component, view, page
    /// or session is not.
    Test,
}

/// One parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    /// The name; tuple parameters (`def f((a, b))`) have an empty name.
    pub name: String,
    pub kind: ParameterKind,
    pub has_default: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// Kind of a [`Parameter`].
pub enum ParameterKind {
    Positional,
    /// `*args`
    VarArgs,
    /// `**kwargs`
    KwArgs,
}

/// Why a call does not match a signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallMismatch {
    TooFewArguments { required: usize, given: usize },
    TooManyArguments { max: usize, given: usize },
    UnknownKeyword(String),
    DuplicateArgument(String),
}

impl Signature {
    /// Checks a call with `positional` positional arguments and the given
    /// keyword names. When the call uses `*args` or `**kwargs` itself, only
    /// definite errors are reported.
    pub fn check_call(
        &self,
        positional: usize,
        keywords: &[&str],
        star_args: bool,
        star_kwargs: bool,
    ) -> Result<(), CallMismatch> {
        let named: Vec<&Parameter> = self
            .params
            .iter()
            .filter(|p| p.kind == ParameterKind::Positional)
            .collect();
        let var_args = self.params.iter().any(|p| p.kind == ParameterKind::VarArgs);
        let kw_args = self.params.iter().any(|p| p.kind == ParameterKind::KwArgs);
        if positional > named.len() && !var_args {
            return Err(CallMismatch::TooManyArguments {
                max: named.len(),
                given: positional,
            });
        }
        for kw in keywords {
            match named.iter().position(|p| p.name == *kw) {
                Some(i) if i < positional => {
                    return Err(CallMismatch::DuplicateArgument((*kw).to_owned()));
                }
                Some(_) => {}
                None if kw_args => {}
                None => return Err(CallMismatch::UnknownKeyword((*kw).to_owned())),
            }
        }
        if !star_args && !star_kwargs {
            let required = named.iter().filter(|p| !p.has_default).count();
            let covered = named
                .iter()
                .enumerate()
                .filter(|(i, p)| {
                    !p.has_default && (*i < positional || keywords.contains(&p.name.as_str()))
                })
                .count();
            if covered < required {
                return Err(CallMismatch::TooFewArguments {
                    required,
                    given: positional + keywords.len(),
                });
            }
        }
        Ok(())
    }
}

/// Result of resolving a dotted name against a project library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryLookup<'i> {
    /// The first segment is not a top-level library name.
    NotLibrary,
    /// A package folder (no `code.py`).
    Package { dotted: String },
    /// A module; `rest` are the segments after it that were not checked
    /// (only when the module's symbols are unknown).
    Module {
        resource: ResourceId,
        dotted: String,
    },
    /// A top-level symbol of a module; `rest` is the number of further
    /// attribute segments (not checked).
    Symbol {
        resource: ResourceId,
        symbol: &'i Symbol,
        rest: usize,
    },
    /// Segment `index` does not exist in the package `parent`.
    Missing { index: usize, parent: String },
    /// Segment `index` is a child of a module that has its own `code.py`,
    /// which hides it (format spec 4.2).
    HiddenByModule { index: usize, module: String },
    /// Segment `index` is not a top-level name of the module.
    MissingAttribute { index: usize, resource: ResourceId },
    /// Cannot be decided: the module's symbols are unknown or dynamic, or
    /// the project inherits from a missing parent.
    Unknown,
}

/// How many imports of library modules one path follows
/// ([`LibraryTree::resolve_path`]); beyond, what the path reaches counts as
/// used as a value.
pub const MAX_IMPORT_HOPS: usize = 64;

/// A dotted library path resolved through the imports of library modules
/// ([`LibraryTree::resolve_path`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryPath<'i> {
    /// Where the path ends. Segment indexes count in the path as written;
    /// a miss inside an import's own target (`from lib import nope`) is
    /// `Unknown` here (the import is reported where it is written).
    /// `Unknown` when the path may end in several places.
    pub end: LibraryLookup<'i>,
    /// Every top-level name of a module the path uses, in order: the names
    /// of the imports it follows (`m0.util` for `m0.util.json`) and the
    /// name where it ends (`json` of `lib.util`), in every place it may
    /// continue.
    pub names: Vec<(ResourceId, &'i Symbol)>,
    /// Every module whose names the path reads (it runs their code).
    pub modules: Vec<ResourceId>,
    /// The path in the target of each import followed (`lib.util.json`),
    /// for facts kept by the path as written (stores).
    pub paths: Vec<String>,
    /// The library objects the path may reach but cannot follow: they count
    /// as used as values (default deny).
    pub values: Vec<LibraryObject>,
    ends: Vec<End<'i>>,
    hops: Vec<Hop>,
}

impl<'i> LibraryPath<'i> {
    /// Every place where the path may end (one, unless a module with
    /// dynamic names star-imports others), and what a variable that may
    /// hold an import's target leads to, each with the path in the module
    /// where it ends.
    pub fn ends(&self) -> impl Iterator<Item = (&LibraryLookup<'i>, &[String])> {
        self.ends.iter().map(|e| (&e.lookup, e.path.as_slice()))
    }
}

/// A place where a path ends: the lookup and the path in that module.
#[derive(Clone, Debug, PartialEq, Eq)]
struct End<'i> {
    lookup: LibraryLookup<'i>,
    path: Vec<String>,
    /// How many trailing segments of `path` are the path as written.
    shared: usize,
    /// Reached through a variable that may hold an import's target
    /// ([`VariableImport`]): what the path may read, not where it ends.
    side: bool,
}

/// An import a path followed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hop {
    /// The module that binds the name, and its dotted name.
    resource: ResourceId,
    module: String,
    /// The segments after the name.
    tail: Vec<String>,
}

#[derive(Default)]
struct Walk<'i> {
    names: Vec<(ResourceId, &'i Symbol)>,
    modules: Vec<ResourceId>,
    paths: Vec<String>,
    values: Vec<LibraryObject>,
    ends: Vec<End<'i>>,
    hops: Vec<Hop>,
    followed: usize,
    seen: rustc_hash::FxHashSet<String>,
    ambiguous: bool,
    /// Inside a branch through a [`VariableImport`].
    side: usize,
    /// What `modules` and `values` hold, to keep each once.
    module_set: rustc_hash::FxHashSet<ResourceId>,
    value_set: rustc_hash::FxHashSet<LibraryObject>,
}

impl Walk<'_> {
    /// Adds a module whose names the path reads, once.
    fn module(&mut self, resource: ResourceId) {
        if self.module_set.insert(resource) {
            self.modules.push(resource);
        }
    }

    /// Adds an object the path may reach but not follow, once (a path
    /// through many star imports reaches the same ones again and again).
    fn value(&mut self, object: LibraryObject) {
        if !self.value_set.contains(&object) {
            self.value_set.insert(object.clone());
            self.values.push(object);
        }
    }
}

/// What [`LibraryTree::resolve_path_with`] keeps between paths resolved
/// with the same symbols (review round 29, M2): the star-import graph of
/// the library, which modules may provide a name through star imports,
/// where a name a module does not bind may come from, and the object each
/// import target names. None of it depends on the path that asks.
/// Thread-safe: the project index keeps one per project and symbols view.
#[derive(Default)]
pub struct ResolveMemo {
    graph: std::sync::OnceLock<StarGraph>,
    /// By name: the modules that bind it, and those that may get it through
    /// star imports.
    names: std::sync::Mutex<FxHashMap<String, std::sync::Arc<NameStars>>>,
    /// By module and name: where the name may come from.
    reach: std::sync::Mutex<FxHashMap<(usize, String), std::sync::Arc<StarReach>>>,
    objects: std::sync::Mutex<FxHashMap<String, Option<LibraryObject>>>,
    /// By name and star-imported module: [`LibraryTree::star_lookup`].
    star_lookups:
        std::sync::Mutex<FxHashMap<String, FxHashMap<String, std::sync::Arc<StarLookup>>>>,
}

/// What `from <star> import *` brings of a name
/// ([`LibraryTree::star_lookup`]).
#[derive(Debug)]
enum StarLookup {
    /// The module does not export it.
    Nothing,
    /// Look the name up in the star-imported module.
    Itself,
    /// The module only passes on what its own star imports bring: where
    /// the name may come from in it.
    Via {
        module: usize,
        reach: std::sync::Arc<StarReach>,
    },
}

impl std::fmt::Debug for ResolveMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolveMemo")
    }
}

/// A set of modules by their index in [`StarGraph`], allocated as it grows
/// (most names are bound by few modules, if any).
#[derive(Clone, Debug, Default)]
struct ModuleSet(Vec<u64>);

impl ModuleSet {
    fn contains(&self, i: usize) -> bool {
        self.0.get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0)
    }

    /// Adds `i`; whether it was new.
    fn insert(&mut self, i: usize) -> bool {
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        if self.0.len() <= word {
            self.0.resize(word + 1, 0);
        }
        let new = self.0[word] & bit == 0;
        self.0[word] |= bit;
        new
    }

    fn remove(&mut self, i: usize) {
        if let Some(w) = self.0.get_mut(i / 64) {
            *w &= !(1u64 << (i % 64));
        }
    }

    fn union(&mut self, other: &ModuleSet) {
        if self.0.len() < other.0.len() {
            self.0.resize(other.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= *b;
        }
    }

    fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(w, bits)| {
            (0..64)
                .filter(move |b| bits & (1u64 << b) != 0)
                .map(move |b| w * 64 + b)
        })
    }
}

/// The library's modules and their star imports of library modules.
#[derive(Debug, Default)]
struct StarGraph {
    resources: Vec<ResourceId>,
    dotted: Vec<String>,
    by_resource: FxHashMap<ResourceId, usize>,
    /// What `from <module> import *` binds, by module.
    exports: Vec<StarExports>,
    /// Each module's module-level star imports of library paths, in order:
    /// the path as written and the module it names, when it names one.
    stars: Vec<Vec<(String, Option<usize>)>>,
    /// The modules star-importing each module.
    importers: Vec<Vec<usize>>,
    /// Modules whose names are not known: no symbols, or dynamic otherwise
    /// than by star imports of library modules (`exec`, namespace writes,
    /// frame routes, a star import of a module outside the library).
    unknown: ModuleSet,
    /// Modules that may get names from an unknown module through star
    /// imports (whatever the name).
    reach_unknown: ModuleSet,
    /// By name, the modules that bind it.
    binders: FxHashMap<String, Vec<usize>>,
}

/// For one name: the modules that bind it and the modules whose star
/// imports may bring it (a module that does not bind it, is dynamic only
/// by star imports, and star-imports one that binds it or brings it, and
/// whose `__all__` passes it on).
#[derive(Debug)]
struct NameStars {
    binds: ModuleSet,
    brings: ModuleSet,
    /// A cycle of star imports that pass the name on was met: each
    /// module's answer is searched on its own ([`StarGraph::reach`]).
    cyclic: std::sync::atomic::AtomicBool,
    /// Where the name may come from in a module that passes it on, for
    /// star imports without cycles ([`StarGraph::reach_acyclic`]).
    kept: std::sync::Mutex<FxHashMap<usize, std::sync::Arc<Sources>>>,
}

/// Where a name may come from in a module ([`NameStars::kept`]).
#[derive(Clone, Debug)]
struct Sources {
    places: ModuleSet,
    other: Vec<String>,
    through: ModuleSet,
}

/// Where a name a module does not bind may come from through its star
/// imports ([`ResolveMemo::reach`]).
#[derive(Debug, Default)]
struct StarReach {
    /// The star-imported modules that bind the name or whose names are not
    /// known, and star imports of library paths that are not modules: the
    /// path continues in each.
    places: Vec<String>,
    /// The modules on the way, which only pass the name on: their
    /// namespaces are read.
    through: Vec<ResourceId>,
    /// Both as sets.
    sources: Option<Sources>,
}

impl StarReach {
    fn is_empty(&self) -> bool {
        self.places.is_empty() && self.through.is_empty()
    }
}

impl ResolveMemo {
    fn graph<'i>(
        &self,
        tree: &LibraryTree,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> &StarGraph {
        self.graph.get_or_init(|| StarGraph::build(tree, symbols))
    }

    fn name(&self, graph: &StarGraph, name: &str) -> std::sync::Arc<NameStars> {
        if let Some(known) = self.names.lock().expect("memo").get(name) {
            return known.clone();
        }
        let facts = std::sync::Arc::new(graph.name_stars(name));
        self.names
            .lock()
            .expect("memo")
            .insert(name.to_owned(), facts.clone());
        facts
    }

    /// Where `name`, which the module `resource` does not bind, may come
    /// from through its star imports.
    fn reach<'i>(
        &self,
        tree: &LibraryTree,
        resource: ResourceId,
        name: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> std::sync::Arc<StarReach> {
        let graph = self.graph(tree, symbols);
        let Some(&root) = graph.by_resource.get(&resource) else {
            return Default::default();
        };
        let key = (root, name.to_owned());
        if let Some(known) = self.reach.lock().expect("memo").get(&key) {
            return known.clone();
        }
        let facts = self.name(graph, name);
        let reach = std::sync::Arc::new(graph.reach(root, name, &facts));
        self.reach.lock().expect("memo").insert(key, reach.clone());
        reach
    }

    /// [`LibraryTree::object_of`], kept.
    fn object_of<'i>(
        &self,
        tree: &LibraryTree,
        dotted: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Option<LibraryObject> {
        if let Some(object) = self.objects.lock().expect("memo").get(dotted) {
            return object.clone();
        }
        let object = tree.object_of(dotted, symbols);
        self.objects
            .lock()
            .expect("memo")
            .insert(dotted.to_owned(), object.clone());
        object
    }
}

impl StarGraph {
    fn build<'i>(
        tree: &LibraryTree,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Self {
        let mut g = StarGraph::default();
        for (dotted, resource) in tree.modules() {
            g.by_resource.insert(resource, g.resources.len());
            g.resources.push(resource);
            g.dotted.push(dotted.to_owned());
        }
        let n = g.resources.len();
        g.importers = vec![Vec::new(); n];
        for i in 0..n {
            let Some(syms) = symbols(g.resources[i]) else {
                g.unknown.insert(i);
                g.exports.push(StarExports::Any);
                g.stars.push(Vec::new());
                continue;
            };
            g.exports.push(syms.star_exports.clone());
            for symbol in &syms.symbols {
                let list = g.binders.entry(symbol.name.clone()).or_default();
                if list.last() != Some(&i) {
                    list.push(i);
                }
            }
            let mut foreign = false;
            let mut stars = Vec::new();
            for star in &syms.star_imports {
                if !tree.is_library_path(star) {
                    foreign = true;
                    continue;
                }
                let segments: Vec<String> = star.split('.').map(str::to_owned).collect();
                let target = match tree.lookup(&segments, symbols).0 {
                    LibraryLookup::Module { resource, .. } => g.by_resource.get(&resource).copied(),
                    _ => None,
                };
                stars.push((star.clone(), target));
            }
            if syms.dynamic && (foreign || !syms.names_from_stars_only) {
                g.unknown.insert(i);
            }
            g.stars.push(stars);
        }
        for i in 0..n {
            for (_, target) in &g.stars[i] {
                // Modules are visited in order: `i` is last when present.
                if let Some(t) = *target
                    && g.importers[t].last() != Some(&i)
                {
                    g.importers[t].push(i);
                }
            }
        }
        // Whatever the name, a module may get it from an unknown module it
        // star-imports, directly or not (export lists are not consulted:
        // more than may happen).
        let mut work: Vec<usize> = (0..n).filter(|i| g.unknown.contains(*i)).collect();
        while let Some(m) = work.pop() {
            for &i in &g.importers[m] {
                if !g.unknown.contains(i) && g.reach_unknown.insert(i) {
                    work.push(i);
                }
            }
        }
        g
    }

    fn name_stars(&self, name: &str) -> NameStars {
        let mut binds = ModuleSet::default();
        let mut brings = ModuleSet::default();
        let mut work: Vec<usize> = Vec::new();
        for &i in self.binders.get(name).into_iter().flatten() {
            binds.insert(i);
            work.push(i);
        }
        while let Some(m) = work.pop() {
            if !self.exports[m].exports(name) {
                continue;
            }
            for &i in &self.importers[m] {
                if !binds.contains(i) && !self.unknown.contains(i) && brings.insert(i) {
                    work.push(i);
                }
            }
        }
        NameStars {
            binds,
            brings,
            cyclic: Default::default(),
            kept: Default::default(),
        }
    }

    /// How the star import `star` (naming `target`) of a module may bring
    /// `name`: as a place where the path continues (it binds the name, its
    /// names are not known, or it is not a module), as a module that passes
    /// it on, or not at all.
    fn star_kind(&self, target: Option<usize>, name: &str, facts: &NameStars) -> StarKind {
        let Some(t) = target else {
            return StarKind::Other;
        };
        if !self.exports[t].exports(name) {
            StarKind::None
        } else if facts.binds.contains(t) || self.unknown.contains(t) {
            StarKind::Place(t)
        } else if facts.brings.contains(t) || self.reach_unknown.contains(t) {
            StarKind::Through(t)
        } else {
            StarKind::None
        }
    }

    /// Where `name` may come from in the module `root`, which does not
    /// bind it: every module its star imports reach, through modules that
    /// pass it on, that binds it or whose names are not known (the path
    /// continues there); a module that can neither provide it nor pass it
    /// on is not followed (review round 29, M2). Every such module is
    /// found, also around cycles of star imports (a name may come from any
    /// module of the cycle, whatever the order they load in).
    fn reach(&self, root: usize, name: &str, facts: &NameStars) -> StarReach {
        let sources = if facts.cyclic.load(std::sync::atomic::Ordering::Relaxed) {
            None
        } else {
            self.reach_acyclic(root, name, facts)
        };
        let sources = sources.unwrap_or_else(|| {
            facts
                .cyclic
                .store(true, std::sync::atomic::Ordering::Relaxed);
            self.reach_search(root, name, facts)
        });
        StarReach {
            places: sources
                .places
                .iter()
                .map(|i| self.dotted[i].clone())
                .chain(sources.other.iter().cloned())
                .collect(),
            through: sources.through.iter().map(|i| self.resources[i]).collect(),
            sources: (!sources.places.is_empty()
                || !sources.through.is_empty()
                || !sources.other.is_empty())
            .then_some(sources),
        }
    }

    /// [`StarGraph::reach`] by a search from `root` alone.
    fn reach_search(&self, root: usize, name: &str, facts: &NameStars) -> Sources {
        let mut out = Sources {
            places: ModuleSet::default(),
            other: Vec::new(),
            through: ModuleSet::default(),
        };
        let mut visited = ModuleSet::default();
        visited.insert(root);
        let mut work = vec![root];
        while let Some(m) = work.pop() {
            for (star, target) in &self.stars[m] {
                match self.star_kind(*target, name, facts) {
                    StarKind::Other => {
                        if !out.other.contains(star) {
                            out.other.push(star.clone());
                        }
                    }
                    StarKind::Place(t) => {
                        out.places.insert(t);
                    }
                    StarKind::Through(t) => {
                        if visited.insert(t) {
                            out.through.insert(t);
                            work.push(t);
                        }
                    }
                    StarKind::None => {}
                }
            }
        }
        out
    }

    /// [`StarGraph::reach`] from the answers of the modules `root`'s star
    /// imports pass the name on through, kept per module ([`NameStars::kept`]):
    /// a library where every module star-imports every earlier one is
    /// answered in time about linear in its star imports. `None` when a
    /// cycle of such modules is met (nothing found on the way is kept).
    fn reach_acyclic(&self, root: usize, name: &str, facts: &NameStars) -> Option<Sources> {
        let mut local: FxHashMap<usize, Sources> = FxHashMap::default();
        let mut gray = ModuleSet::default();
        let kept = |m: usize| facts.kept.lock().expect("memo").get(&m).cloned();
        // Post-order over the modules that pass the name on: (module, next
        // star to look at, what it reaches so far).
        let new = || Sources {
            places: ModuleSet::default(),
            other: Vec::new(),
            through: ModuleSet::default(),
        };
        let mut stack: Vec<(usize, usize, Sources)> = vec![(root, 0, new())];
        gray.insert(root);
        while let Some((m, next, sources)) = stack.last_mut() {
            let m = *m;
            let Some((star, target)) = self.stars[m].get(*next) else {
                let (m, _, done) = stack.pop().expect("frame");
                gray.remove(m);
                match stack.last_mut() {
                    Some((_, _, parent)) => {
                        parent.through.insert(m);
                        merge(parent, &done);
                        local.insert(m, done);
                    }
                    None => {
                        let mut keep = facts.kept.lock().expect("memo");
                        for (k, v) in local {
                            keep.insert(k, std::sync::Arc::new(v));
                        }
                        return Some(done);
                    }
                }
                continue;
            };
            *next += 1;
            match self.star_kind(*target, name, facts) {
                StarKind::Other => {
                    if !sources.other.contains(star) {
                        sources.other.push(star.clone());
                    }
                }
                StarKind::Place(t) => {
                    sources.places.insert(t);
                }
                StarKind::Through(t) => {
                    if gray.contains(t) {
                        return None;
                    }
                    if let Some(done) = local.get(&t) {
                        let done = done.clone();
                        sources.through.insert(t);
                        merge(sources, &done);
                    } else if let Some(done) = kept(t) {
                        sources.through.insert(t);
                        merge(sources, &done);
                    } else {
                        gray.insert(t);
                        stack.push((t, 0, new()));
                    }
                }
                StarKind::None => {}
            }
        }
        None
    }
}

/// How a star import may bring a name ([`StarGraph::star_kind`]).
enum StarKind {
    /// Not a module of the library (a package, a missing module).
    Other,
    Place(usize),
    Through(usize),
    None,
}

/// Adds what `from` reaches to `into`.
fn merge(into: &mut Sources, from: &Sources) {
    into.places.union(&from.places);
    into.through.union(&from.through);
    for o in &from.other {
        if !into.other.contains(o) {
            into.other.push(o.clone());
        }
    }
}

/// The lookup where a path ends, with segment indexes in the path as
/// written (`written` segments, of which the last `end.shared` are the
/// last of `end.path`).
fn remap<'i>(end: &End<'i>, written: usize) -> LibraryLookup<'i> {
    let len = end.path.len();
    let map = |index: usize| (index >= len - end.shared).then(|| written - (len - index));
    match end.lookup.clone() {
        LibraryLookup::Missing { index, parent } => {
            map(index).map_or(LibraryLookup::Unknown, |index| LibraryLookup::Missing {
                index,
                parent,
            })
        }
        LibraryLookup::HiddenByModule { index, module } => map(index)
            .map_or(LibraryLookup::Unknown, |index| {
                LibraryLookup::HiddenByModule { index, module }
            }),
        LibraryLookup::MissingAttribute { index, resource } => map(index)
            .map_or(LibraryLookup::Unknown, |index| {
                LibraryLookup::MissingAttribute { index, resource }
            }),
        other => other,
    }
}

#[derive(Debug, Default)]
struct Node {
    dotted: String,
    resource: Option<ResourceId>,
    children: FxHashMap<String, usize>,
}

/// The module tree a project sees (after inheritance).
#[derive(Debug, Default)]
pub struct LibraryTree {
    nodes: Vec<Node>,
    roots: FxHashMap<String, usize>,
}

impl LibraryTree {
    /// Builds the tree from `(resource path, resource)` pairs of library
    /// modules (`core/format`).
    pub fn new<'p>(modules: impl IntoIterator<Item = (&'p str, ResourceId)>) -> LibraryTree {
        let mut tree = LibraryTree::default();
        for (path, resource) in modules {
            let mut parent: Option<usize> = None;
            let mut dotted = String::new();
            for seg in path.split('/') {
                if !dotted.is_empty() {
                    dotted.push('.');
                }
                dotted.push_str(seg);
                let existing = match parent {
                    None => tree.roots.get(seg).copied(),
                    Some(p) => tree.nodes[p].children.get(seg).copied(),
                };
                let idx = match existing {
                    Some(i) => i,
                    None => {
                        let i = tree.nodes.len();
                        tree.nodes.push(Node {
                            dotted: dotted.clone(),
                            ..Node::default()
                        });
                        match parent {
                            None => tree.roots.insert(seg.to_owned(), i),
                            Some(p) => tree.nodes[p].children.insert(seg.to_owned(), i),
                        };
                        i
                    }
                };
                parent = Some(idx);
            }
            if let Some(p) = parent {
                tree.nodes[p].resource = Some(resource);
            }
        }
        tree
    }

    /// Top-level names, which every script sees as globals.
    pub fn globals(&self) -> impl Iterator<Item = &str> {
        self.roots.keys().map(String::as_str)
    }

    /// Whether `name` is a top-level library name.
    pub fn is_global(&self, name: &str) -> bool {
        self.roots.contains_key(name)
    }

    /// Modules (`dotted name`, resource) in the tree.
    pub fn modules(&self) -> impl Iterator<Item = (&str, ResourceId)> {
        self.nodes
            .iter()
            .filter_map(|n| n.resource.map(|r| (n.dotted.as_str(), r)))
    }

    /// Where `pkg.mod.func...` ends, through the imports of library
    /// modules ([`LibraryTree::resolve_path`]). `symbols` returns the
    /// summary of a module resource, if known.
    pub fn resolve<'i>(
        &self,
        dotted: &str,
        symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> LibraryLookup<'i> {
        self.resolve_path(dotted, symbols).end
    }

    /// The resolver of dotted library paths (review round 28: every
    /// consumer goes through it, so that none credits a use, judges a
    /// value or reports a miss on a path another follows differently).
    ///
    /// A path is looked up segment by segment from a top-level package; at
    /// a module, the segment is one of its top-level names. A name the
    /// module binds once by an import of another library module, package
    /// or name (`import lib.util as util`, `from lib import util`, `import
    /// m1`, `import lib.util`, which binds `lib`) is that object or unbound
    /// (measured on 8.3.3: Ignition's `ProjectScriptModule` and
    /// `ScriptPackage` objects resolve such chains, `m0.util.json` for
    /// `from lib import util` in `m0`), so the rest of the path continues
    /// in the import's target: `m0.util.json` ends at `json` of
    /// `lib.util`, and both `m0.util` and `lib.util.json` are names the
    /// path uses. Default deny where the path cannot be followed with
    /// certainty:
    ///
    /// * a name the module binds through imports of library objects but
    ///   that is a [`SymbolKind::Variable`] ([`VariableImport`]): the path
    ///   ends at the variable, and what it reads through each import is
    ///   counted; when the module binds the name otherwise too or rebinds
    ///   it (`try: from lib import util` / `except: util = None`), each
    ///   import's target counts as used as a value
    ///   ([`LibraryPath::values`]);
    /// * a name a module with dynamic names does not bind, when the module
    ///   star-imports library modules: the path continues in every module
    ///   its star imports reach, directly or through modules that only
    ///   pass the name on, that binds the name or whose names are not known
    ///   (the name may come from any), the modules on the way are read, and
    ///   the end is unknown; a module that can neither provide the name nor
    ///   pass it on is not followed (review round 29, M2: the old search
    ///   followed every star import that may export the name, up to
    ///   [`MAX_IMPORT_HOPS`] of them, again for every path);
    /// * an import that leads back to a path already followed (a cycle),
    ///   or more than [`MAX_IMPORT_HOPS`] imports: the module and the
    ///   import's target count as used as values, and the end is unknown.
    pub fn resolve_path<'i>(
        &self,
        dotted: &str,
        symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> LibraryPath<'i> {
        self.resolve_path_with(dotted, &symbols, &ResolveMemo::default())
    }

    /// [`LibraryTree::resolve_path`] keeping answers in `memo`, which must
    /// only ever see the same `symbols` (review round 29, M2).
    pub fn resolve_path_with<'i>(
        &self,
        dotted: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        memo: &ResolveMemo,
    ) -> LibraryPath<'i> {
        let segments: Vec<String> = dotted.split('.').map(str::to_owned).collect();
        let written = segments.len();
        let mut walk = Walk::default();
        walk.seen.insert(dotted.to_owned());
        self.walk(segments, written, symbols, &mut walk, memo);
        let main: Vec<&End<'i>> = walk.ends.iter().filter(|e| !e.side).collect();
        let end = match (walk.ambiguous, main.as_slice()) {
            (false, [only]) => remap(only, written),
            _ => LibraryLookup::Unknown,
        };
        LibraryPath {
            end,
            names: walk.names,
            modules: walk.modules,
            paths: walk.paths,
            values: walk.values,
            ends: walk.ends,
            hops: walk.hops,
        }
    }

    /// One step of [`LibraryTree::resolve_path`]: looks `segments` up and
    /// follows what can be followed.
    fn walk<'i>(
        &self,
        segments: Vec<String>,
        shared: usize,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        w: &mut Walk<'i>,
        memo: &ResolveMemo,
    ) {
        let (lookup, dynamic_miss) = self.lookup(&segments, symbols);
        match &lookup {
            &LibraryLookup::Symbol {
                resource,
                symbol,
                rest,
            } => {
                w.module(resource);
                w.names.push((resource, symbol));
                let at = segments.len() - rest - 1;
                let tail = &segments[at + 1..];
                match &symbol.kind {
                    SymbolKind::Import {
                        target: Some(target),
                    } if self.is_library_path(target) => {
                        let module = segments[..at].join(".");
                        let shared = shared.min(rest);
                        self.follow(resource, &module, target, tail, shared, symbols, w, memo);
                        return;
                    }
                    SymbolKind::Variable => {
                        let imports = symbols(resource)
                            .into_iter()
                            .flat_map(|s| &s.variable_imports)
                            .filter(|i| i.name == symbol.name && self.is_library_path(&i.target));
                        let module = segments[..at].join(".");
                        for import in imports {
                            if import.several
                                && let Some(object) = memo.object_of(self, &import.target, symbols)
                            {
                                // Default deny: the name may hold it.
                                w.value(object);
                            }
                            // What the path reads if the name holds it; the
                            // path itself ends at the variable.
                            let ambiguous = w.ambiguous;
                            w.side += 1;
                            let shared = shared.min(rest);
                            self.follow(
                                resource,
                                &module,
                                &import.target,
                                tail,
                                shared,
                                symbols,
                                w,
                                memo,
                            );
                            w.side -= 1;
                            w.ambiguous = ambiguous;
                        }
                    }
                    _ => {}
                }
            }
            &LibraryLookup::Module { resource, .. }
            | &LibraryLookup::MissingAttribute { resource, .. } => {
                w.module(resource);
            }
            LibraryLookup::Unknown => {
                if let Some((resource, at)) = dynamic_miss {
                    w.module(resource);
                    let reach = memo.reach(self, resource, &segments[at], symbols);
                    if !reach.is_empty() {
                        w.ambiguous = true;
                        for &through in &reach.through {
                            w.module(through);
                        }
                        let module = segments[..at].join(".");
                        let tail = &segments[at..];
                        for place in &reach.places {
                            let shared = shared.min(tail.len());
                            self.follow(resource, &module, place, tail, shared, symbols, w, memo);
                        }
                    }
                }
            }
            _ => {}
        }
        w.ends.push(End {
            lookup,
            path: segments,
            shared,
            side: w.side > 0,
        });
    }

    /// Continues a path in `target` (an import of the module `resource`,
    /// dotted `module`) with the segments `tail`, unless that path was
    /// already followed or the bound is reached.
    #[allow(clippy::too_many_arguments)]
    fn follow<'i>(
        &self,
        resource: ResourceId,
        module: &str,
        target: &str,
        tail: &[String],
        shared: usize,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        w: &mut Walk<'i>,
        memo: &ResolveMemo,
    ) {
        // The bound first: past it, no path is built (review round 29, M2).
        let fresh = w.followed < MAX_IMPORT_HOPS && {
            let mut key = String::with_capacity(
                target.len() + tail.iter().map(|t| t.len() + 1).sum::<usize>(),
            );
            key.push_str(target);
            for t in tail {
                key.push('.');
                key.push_str(t);
            }
            match w.seen.contains(&key) {
                true => false,
                false => {
                    w.seen.insert(key.clone());
                    w.paths.push(key);
                    true
                }
            }
        };
        if !fresh {
            // Default deny: what the path reaches is not known.
            w.ambiguous = true;
            w.value(LibraryObject::Module {
                resource,
                dotted: module.to_owned(),
            });
            if let Some(object) = memo.object_of(self, target, symbols) {
                w.value(object);
            }
            return;
        }
        let next: Vec<String> = target
            .split('.')
            .map(str::to_owned)
            .chain(tail.iter().cloned())
            .collect();
        w.followed += 1;
        w.hops.push(Hop {
            resource,
            module: module.to_owned(),
            tail: tail.to_vec(),
        });
        self.walk(next, shared, symbols, w, memo);
    }

    /// Where `name` may come from through the star imports `stars` of a
    /// script that does not bind it (review round 29, M2): the places to
    /// look the name up in, star-imported modules first, and the modules
    /// on the way, each once (a module star-importing every earlier one
    /// took time cubic in their number when each star import was looked up
    /// on its own).
    pub fn star_imports<'i>(
        &self,
        stars: &[String],
        name: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        memo: &ResolveMemo,
    ) -> (Vec<String>, Vec<ResourceId>) {
        let mut itself: Vec<String> = Vec::new();
        let mut via: Option<Sources> = None;
        let mut through: Vec<ResourceId> = Vec::new();
        for star in stars {
            match &*self.star_lookup(star, name, symbols, memo) {
                StarLookup::Nothing => {}
                StarLookup::Itself => {
                    if !itself.contains(star) {
                        itself.push(star.clone());
                    }
                }
                StarLookup::Via { module, reach } => {
                    let all = via.get_or_insert_with(|| Sources {
                        places: ModuleSet::default(),
                        other: Vec::new(),
                        through: ModuleSet::default(),
                    });
                    if let Some(found) = &reach.sources {
                        merge(all, found);
                    }
                    all.through.insert(*module);
                }
            }
        }
        let mut places = itself;
        if let Some(all) = via {
            let graph = memo.graph(self, symbols);
            for p in all
                .places
                .iter()
                .map(|i| &graph.dotted[i])
                .chain(&all.other)
            {
                if !places.contains(p) {
                    places.push(p.clone());
                }
            }
            through.extend(all.through.iter().map(|i| graph.resources[i]));
        }
        (places, through)
    }

    /// What `from <star> import *` brings of `name` (review round 29, M2):
    /// nothing when `star` does not export it ([`LibraryTree::exports`]);
    /// when `star` is a module of the library that does not bind `name`
    /// but passes on what its own star imports bring, that module;
    /// otherwise `star` itself is where to look.
    fn star_lookup<'i>(
        &self,
        star: &str,
        name: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        memo: &ResolveMemo,
    ) -> std::sync::Arc<StarLookup> {
        if let Some(known) = memo
            .star_lookups
            .lock()
            .expect("memo")
            .get(name)
            .and_then(|m| m.get(star))
        {
            return known.clone();
        }
        let lookup = std::sync::Arc::new(self.star_lookup_uncached(star, name, symbols, memo));
        memo.star_lookups
            .lock()
            .expect("memo")
            .entry(name.to_owned())
            .or_default()
            .insert(star.to_owned(), lookup.clone());
        lookup
    }

    fn star_lookup_uncached<'i>(
        &self,
        star: &str,
        name: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        memo: &ResolveMemo,
    ) -> StarLookup {
        if !self.exports(star, name, symbols) {
            return StarLookup::Nothing;
        }
        let segments: Vec<String> = star.split('.').map(str::to_owned).collect();
        let LibraryLookup::Module { resource, .. } = self.lookup(&segments, symbols).0 else {
            return StarLookup::Itself;
        };
        let graph = memo.graph(self, symbols);
        let Some(&t) = graph.by_resource.get(&resource) else {
            return StarLookup::Itself;
        };
        let facts = memo.name(graph, name);
        match graph.star_kind(Some(t), name, &facts) {
            StarKind::Through(_) => {}
            // Neither binds the name nor passes it on: it does not come
            // from there.
            StarKind::None => return StarLookup::Nothing,
            StarKind::Place(_) | StarKind::Other => return StarLookup::Itself,
        }
        StarLookup::Via {
            module: t,
            reach: memo.reach(self, resource, name, symbols),
        }
    }

    /// Whether `from <star> import *` may bind `name`: the library module
    /// `star` exports it, or what `star` is is not known.
    pub fn exports<'i>(
        &self,
        star: &str,
        name: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> bool {
        let segments: Vec<String> = star.split('.').map(str::to_owned).collect();
        match self.lookup(&segments, symbols).0 {
            LibraryLookup::Module { resource, .. } => match symbols(resource) {
                Some(s) => s.star_exports.exports(name),
                None => true,
            },
            _ => true,
        }
    }

    /// Whether `dotted` starts with a top-level library name (an import
    /// that stays in the library; relative imports do not).
    fn is_library_path(&self, dotted: &str) -> bool {
        dotted
            .split('.')
            .next()
            .is_some_and(|root| self.roots.contains_key(root))
    }

    /// The library object that holds what `dotted` names, without following
    /// imports: the module or package itself, or the module of a name.
    fn object_of<'i>(
        &self,
        dotted: &str,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Option<LibraryObject> {
        let segments: Vec<String> = dotted.split('.').map(str::to_owned).collect();
        let module = |resource, at: usize| LibraryObject::Module {
            resource,
            dotted: segments[..at].join("."),
        };
        match self.lookup(&segments, symbols).0 {
            LibraryLookup::Module { resource, dotted } => {
                Some(LibraryObject::Module { resource, dotted })
            }
            LibraryLookup::Package { dotted } => Some(LibraryObject::Package { dotted }),
            LibraryLookup::Symbol { resource, rest, .. } => {
                Some(module(resource, segments.len() - rest - 1))
            }
            LibraryLookup::MissingAttribute { resource, index } => Some(module(resource, index)),
            _ => None,
        }
    }

    /// Looks `segments` up without following imports. The second value is
    /// the module and segment index where a module with dynamic names did
    /// not bind the segment (the lookup is then `Unknown`).
    fn lookup<'i>(
        &self,
        segments: &[String],
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> (LibraryLookup<'i>, Option<(ResourceId, usize)>) {
        let Some(&root) = segments.first().and_then(|s| self.roots.get(s.as_str())) else {
            return (LibraryLookup::NotLibrary, None);
        };
        let mut at = root;
        for (i, seg) in segments.iter().enumerate().skip(1) {
            let seg = seg.as_str();
            let node = &self.nodes[at];
            if let Some(resource) = node.resource {
                let lookup = match symbols(resource) {
                    Some(syms) => match syms.get(seg) {
                        Some(symbol) => LibraryLookup::Symbol {
                            resource,
                            symbol,
                            rest: segments.len() - i - 1,
                        },
                        None if node.children.contains_key(seg) => LibraryLookup::HiddenByModule {
                            index: i,
                            module: node.dotted.clone(),
                        },
                        // A member of the module object itself (`m.dict`,
                        // `m.__dict__`): it exists, and what it reaches is
                        // not a name of the module.
                        None if module_member(seg) || dunder(seg) => LibraryLookup::Unknown,
                        None if syms.dynamic => {
                            return (LibraryLookup::Unknown, Some((resource, i)));
                        }
                        None => LibraryLookup::MissingAttribute { index: i, resource },
                    },
                    None if node.children.contains_key(seg) => LibraryLookup::HiddenByModule {
                        index: i,
                        module: node.dotted.clone(),
                    },
                    None => LibraryLookup::Unknown,
                };
                return (lookup, None);
            }
            match node.children.get(seg) {
                Some(&child) => at = child,
                // A member of the package object (`pkg.dict`, `pkg.clear`).
                None if package_member(seg) || dunder(seg) => {
                    return (LibraryLookup::Unknown, None);
                }
                None => {
                    return (
                        LibraryLookup::Missing {
                            index: i,
                            parent: node.dotted.clone(),
                        },
                        None,
                    );
                }
            }
        }
        let node = &self.nodes[at];
        let lookup = match node.resource {
            Some(resource) => LibraryLookup::Module {
                resource,
                dotted: node.dotted.clone(),
            },
            None => LibraryLookup::Package {
                dotted: node.dotted.clone(),
            },
        };
        (lookup, None)
    }

    /// The first module or package that a use of `dotted` may hand out as
    /// a value ([`LibraryTree::value_objects`]), if any.
    pub fn object_member<'i>(
        &self,
        dotted: &str,
        symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Option<LibraryObject> {
        let path = self.resolve_path(dotted, &symbols);
        self.value_objects(&path, &symbols).into_iter().next()
    }

    /// The modules and packages that a use of the resolved `path` may hand
    /// out as values: code that reads one may read and change everything
    /// in it. Default deny over every segment of the path (review round
    /// 27; every source of dotted references goes through here: scripts,
    /// `runScript` targets, expressions, imports), judged where the path
    /// ends (in the module an import led to):
    ///
    /// * right after a module, a member of Ignition's module object
    ///   ([`MODULE_OBJECT_MEMBERS`]) or a special attribute
    ///   ([`is_special_attribute`]) that the module does not bind (a name it
    ///   binds wins); after a package, a member of the package object
    ///   ([`PACKAGE_OBJECT_MEMBERS`]) or a special attribute where the
    ///   package has no child of that name;
    /// * after a function the module defines, any attribute other than its
    ///   name and documentation ([`HARMLESS_ATTRIBUTES`]): `m.f.func_globals`,
    ///   `m.f.__globals__`, `m.f.__call__`, `m.f.func_code`, an attribute
    ///   stored on the function, ... reach the module's namespace or code;
    /// * after a variable bound only to literals, a special attribute;
    /// * after a class, another variable, an import of a module outside the
    ///   library, or a name the module does not define, any later segment
    ///   that names a special attribute or a member of Jython's function,
    ///   method, class, instance or generator objects
    ///   ([`reaches_from_value`]: `m.C.meth.im_func`, `m.logger.__class__`,
    ///   `m.handler.func_globals`); when the module's names are not known,
    ///   a member of the library objects too ([`is_value_attribute`]);
    /// * when nothing where the path ends counts, the module of an import
    ///   the path followed, when the rest of the path after it names a
    ///   value attribute ([`is_value_attribute`]).
    ///
    /// The objects the path cannot follow ([`LibraryPath::values`]) are
    /// not included.
    pub fn value_objects<'i>(
        &self,
        path: &LibraryPath<'i>,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Vec<LibraryObject> {
        let mut out: Vec<LibraryObject> = Vec::new();
        for end in &path.ends {
            if let Some(object) = self.judge(&end.path, symbols)
                && !out.contains(&object)
            {
                out.push(object);
            }
        }
        if out.is_empty() {
            for hop in path.hops.iter().rev() {
                if hop.tail.iter().any(|s| is_value_attribute(s)) {
                    out.push(LibraryObject::Module {
                        resource: hop.resource,
                        dotted: hop.module.clone(),
                    });
                    break;
                }
            }
        }
        out
    }

    /// [`LibraryTree::value_objects`] where a path ends.
    fn judge<'i>(
        &self,
        segments: &[String],
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Option<LibraryObject> {
        let mut at = *self.roots.get(segments.first()?.as_str())?;
        for (i, seg) in segments.iter().enumerate().skip(1) {
            let seg = seg.as_str();
            let node = &self.nodes[at];
            let rest = &segments[i + 1..];
            if let Some(resource) = node.resource {
                let module = || LibraryObject::Module {
                    resource,
                    dotted: node.dotted.clone(),
                };
                let syms = symbols(resource);
                let Some(symbol) = syms.and_then(|s| s.get(seg)) else {
                    if node.children.contains_key(seg) {
                        // A child module the module's own code hides.
                        return None;
                    }
                    // A member of the module object, or (default deny) a
                    // name the module does not define, used further: it
                    // may hold anything when the module's names are not
                    // known.
                    let known = syms.is_some_and(|s| !s.dynamic);
                    let further = |s: &String| {
                        if known {
                            reaches_from_value(s)
                        } else {
                            is_value_attribute(s)
                        }
                    };
                    return (module_member(seg) || rest.iter().any(further)).then(module);
                };
                return match &symbol.kind {
                    SymbolKind::Function(_) => rest
                        .iter()
                        .any(|s| !HARMLESS_ATTRIBUTES.contains(&s.as_str()))
                        .then(module),
                    // A variable bound only to literals holds a plain value
                    // (`CACHE = {}`: `m.CACHE.clear` is the dict's method):
                    // only its special attributes count.
                    SymbolKind::Variable
                        if syms.is_some_and(|s| s.constants.iter().any(|c| c == seg)) =>
                    {
                        rest.iter().any(|s| is_special_attribute(s)).then(module)
                    }
                    // An import the path did not follow (of a module outside
                    // the library, or beyond the bound) is a value too.
                    SymbolKind::Class | SymbolKind::Variable | SymbolKind::Import { .. } => {
                        rest.iter().any(|s| reaches_from_value(s)).then(module)
                    }
                };
            }
            match node.children.get(seg) {
                Some(&child) => at = child,
                None => {
                    return (package_member(seg) || rest.iter().any(|s| is_value_attribute(s)))
                        .then(|| LibraryObject::Package {
                            dotted: node.dotted.clone(),
                        });
                }
            }
        }
        None
    }

    /// The library objects a module's namespace may hold through its
    /// imports: whoever holds the module as a value reaches them
    /// (`m.util.json` when `m` imports `lib.util` as `util`). Names it
    /// imports from library modules count by their module.
    pub fn imported_objects<'i>(
        &self,
        module: &ModuleSymbols,
        symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Vec<LibraryObject> {
        let targets = module
            .symbols
            .iter()
            .filter_map(|s| match &s.kind {
                SymbolKind::Import { target: Some(t) } => Some(t.as_str()),
                _ => None,
            })
            .chain(module.variable_imports.iter().map(|i| i.target.as_str()))
            .chain(module.star_imports.iter().map(String::as_str));
        let mut out = Vec::new();
        for target in targets {
            if self.is_library_path(target)
                && let Some(object) = self.object_of(target, symbols)
                && !out.contains(&object)
            {
                out.push(object);
            }
        }
        out
    }

    /// The modules of a library object: the module, or every module below
    /// the package.
    pub fn object_modules(&self, object: &LibraryObject) -> Vec<ResourceId> {
        match object {
            LibraryObject::Module { resource, .. } => vec![*resource],
            LibraryObject::Package { dotted } => {
                let prefix = format!("{dotted}.");
                self.modules()
                    .filter(|(m, _)| m.starts_with(&prefix))
                    .map(|(_, r)| r)
                    .collect()
            }
        }
    }

    /// Modules whose own `code.py` hides child modules: `(module, hidden
    /// children)`.
    pub fn shadowed_packages(&self) -> Vec<(&str, Vec<&str>)> {
        let mut out = Vec::new();
        for n in &self.nodes {
            if n.resource.is_some() && !n.children.is_empty() {
                let mut kids: Vec<&str> = n
                    .children
                    .values()
                    .map(|&c| self.nodes[c].dotted.as_str())
                    .collect();
                kids.sort_unstable();
                out.push((n.dotted.as_str(), kids));
            }
        }
        out.sort();
        out
    }
}

/// The resolver as of review round 28, before the memo of round 29 (M2):
/// the reference the memoised one must agree with.
#[cfg(test)]
pub(crate) mod reference {
    use super::*;

    fn push_unique(list: &mut Vec<ResourceId>, resource: ResourceId) {
        if !list.contains(&resource) {
            list.push(resource);
        }
    }

    impl LibraryTree {
        pub(super) fn reference_resolve_path<'i>(
            &self,
            dotted: &str,
            symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        ) -> LibraryPath<'i> {
            let segments: Vec<String> = dotted.split('.').map(str::to_owned).collect();
            let written = segments.len();
            let mut walk = Walk::default();
            walk.seen.insert(dotted.to_owned());
            self.reference_walk(segments, written, &symbols, &mut walk);
            let main: Vec<&End<'i>> = walk.ends.iter().filter(|e| !e.side).collect();
            let end = match (walk.ambiguous, main.as_slice()) {
                (false, [only]) => remap(only, written),
                _ => LibraryLookup::Unknown,
            };
            LibraryPath {
                end,
                names: walk.names,
                modules: walk.modules,
                paths: walk.paths,
                values: walk.values,
                ends: walk.ends,
                hops: walk.hops,
            }
        }

        fn reference_walk<'i>(
            &self,
            segments: Vec<String>,
            shared: usize,
            symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
            w: &mut Walk<'i>,
        ) {
            let (lookup, dynamic_miss) = self.lookup(&segments, symbols);
            match &lookup {
                &LibraryLookup::Symbol {
                    resource,
                    symbol,
                    rest,
                } => {
                    push_unique(&mut w.modules, resource);
                    w.names.push((resource, symbol));
                    let at = segments.len() - rest - 1;
                    let tail = &segments[at + 1..];
                    match &symbol.kind {
                        SymbolKind::Import {
                            target: Some(target),
                        } if self.is_library_path(target) => {
                            let module = segments[..at].join(".");
                            self.reference_follow(
                                resource,
                                module,
                                target,
                                tail,
                                shared.min(rest),
                                symbols,
                                w,
                            );
                            return;
                        }
                        SymbolKind::Variable => {
                            let imports = symbols(resource)
                                .into_iter()
                                .flat_map(|s| &s.variable_imports)
                                .filter(|i| {
                                    i.name == symbol.name && self.is_library_path(&i.target)
                                });
                            let module = segments[..at].join(".");
                            for import in imports {
                                if import.several
                                    && let Some(object) = self.object_of(&import.target, symbols)
                                {
                                    // Default deny: the name may hold it.
                                    w.values.push(object);
                                }
                                // What the path reads if the name holds it; the
                                // path itself ends at the variable.
                                let ambiguous = w.ambiguous;
                                w.side += 1;
                                let shared = shared.min(rest);
                                self.reference_follow(
                                    resource,
                                    module.clone(),
                                    &import.target,
                                    tail,
                                    shared,
                                    symbols,
                                    w,
                                );
                                w.side -= 1;
                                w.ambiguous = ambiguous;
                            }
                        }
                        _ => {}
                    }
                }
                &LibraryLookup::Module { resource, .. }
                | &LibraryLookup::MissingAttribute { resource, .. } => {
                    push_unique(&mut w.modules, resource);
                }
                LibraryLookup::Unknown => {
                    if let Some((resource, at)) = dynamic_miss {
                        push_unique(&mut w.modules, resource);
                        let stars: Vec<&String> = symbols(resource)
                            .into_iter()
                            .flat_map(|s| &s.star_imports)
                            .filter(|s| {
                                self.is_library_path(s) && self.exports(s, &segments[at], symbols)
                            })
                            .collect();
                        if !stars.is_empty() {
                            w.ambiguous = true;
                            let module = segments[..at].join(".");
                            let tail = &segments[at..];
                            for star in stars {
                                let shared = shared.min(tail.len());
                                self.reference_follow(
                                    resource,
                                    module.clone(),
                                    star,
                                    tail,
                                    shared,
                                    symbols,
                                    w,
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
            w.ends.push(End {
                lookup,
                path: segments,
                shared,
                side: w.side > 0,
            });
        }

        #[allow(clippy::too_many_arguments)]
        fn reference_follow<'i>(
            &self,
            resource: ResourceId,
            module: String,
            target: &str,
            tail: &[String],
            shared: usize,
            symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
            w: &mut Walk<'i>,
        ) {
            let next: Vec<String> = target
                .split('.')
                .map(str::to_owned)
                .chain(tail.iter().cloned())
                .collect();
            let key = next.join(".");
            if w.followed >= MAX_IMPORT_HOPS || !w.seen.insert(key.clone()) {
                // Default deny: what the path reaches is not known.
                w.ambiguous = true;
                w.values.push(LibraryObject::Module {
                    resource,
                    dotted: module,
                });
                if let Some(object) = self.object_of(target, symbols) {
                    w.values.push(object);
                }
                return;
            }
            w.followed += 1;
            w.paths.push(key);
            w.hops.push(Hop {
                resource,
                module,
                tail: tail.to_vec(),
            });
            self.reference_walk(next, shared, symbols, w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str) -> Symbol {
        Symbol {
            name: name.into(),
            kind: SymbolKind::Function(Signature::default()),
            range: TextRange::default(),
        }
    }

    /// The resolver of review round 29 (M2), on generated libraries with
    /// imports, star imports (chains, diamonds, every earlier module, hubs,
    /// cycles), names bound several times and dynamic modules, for paths
    /// asked in any order: one memo for every path gives what a new memo
    /// for each gives (what the memo keeps does not depend on the path that
    /// asks); a path that meets no star import is what the resolver of
    /// round 28 ([`reference`]) gives, objects it reaches again kept once;
    /// through star imports, the end is unknown in both, and every name
    /// the old resolver found is found (it stopped after 64 star-imported
    /// modules, and on modules it reached twice).
    #[test]
    fn memoised_paths_match_the_reference_resolver() {
        fn dedup(values: &[LibraryObject]) -> Vec<LibraryObject> {
            let mut out: Vec<LibraryObject> = Vec::new();
            for v in values {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            out
        }
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move |n: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % n as u64) as usize
        };
        // Whether the lookup of `path` misses in a module with star imports.
        fn lookup_star<'i>(
            tree: &LibraryTree,
            path: &[String],
            symbols: &dyn Fn(ResourceId) -> Option<&'i ModuleSymbols>,
        ) -> bool {
            tree.lookup(path, symbols)
                .1
                .is_some_and(|(r, _)| symbols(r).is_some_and(|s| !s.star_imports.is_empty()))
        }
        let mut checked = 0;
        let mut stars = 0;
        for round in 0..60 {
            let n = 3 + next(14);
            let names: Vec<String> = (0..n)
                .map(|i| match i % 3 {
                    0 => format!("m{i}"),
                    1 => format!("p.m{i}"),
                    _ => format!("p.q.m{i}"),
                })
                .collect();
            let paths: Vec<String> = names.iter().map(|m| m.replace('.', "/")).collect();
            let tree = LibraryTree::new(
                paths
                    .iter()
                    .enumerate()
                    .map(|(i, p)| (p.as_str(), ResourceId(i as u32))),
            );
            let mut modules: Vec<ModuleSymbols> = Vec::new();
            for i in 0..n {
                let mut m = ModuleSymbols::default();
                for k in 0..next(4) {
                    m.symbols.push(Symbol {
                        name: format!("f{}", next(5) + k),
                        kind: SymbolKind::Function(Signature::default()),
                        range: TextRange::default(),
                    });
                }
                for _ in 0..next(3) {
                    let t = &names[next(n)];
                    let target = match next(3) {
                        0 => t.clone(),
                        1 => format!("{t}.f{}", next(5)),
                        _ => "json".to_owned(),
                    };
                    m.symbols.push(Symbol {
                        name: format!("a{}", next(4)),
                        kind: SymbolKind::Import {
                            target: Some(target),
                        },
                        range: TextRange::default(),
                    });
                }
                let stars = match round % 4 {
                    // Every earlier module.
                    0 => (0..i).collect::<Vec<_>>(),
                    // A few others, cycles included.
                    1 => (0..next(4)).map(|_| next(n)).collect(),
                    // Two hubs.
                    2 => vec![0, 1].into_iter().filter(|h| *h != i).collect(),
                    // The previous few.
                    _ => (i.saturating_sub(3)..i).collect(),
                };
                for s in stars {
                    m.star_imports.push(names[s].clone());
                }
                m.dynamic = !m.star_imports.is_empty() || next(6) == 0;
                if next(5) == 0 {
                    m.symbols.push(Symbol {
                        name: "v".into(),
                        kind: SymbolKind::Variable,
                        range: TextRange::default(),
                    });
                    m.variable_imports.push(VariableImport {
                        name: "v".into(),
                        target: names[next(n)].clone(),
                        several: next(2) == 0,
                    });
                }
                if next(4) == 0 {
                    m.star_exports = StarExports::Listed(vec![format!("f{}", next(5))]);
                }
                modules.push(m);
            }
            let lookup = |r: ResourceId| modules.get(r.0 as usize);
            let mut memo = ResolveMemo::default();
            let mut queries: Vec<String> = Vec::new();
            for m in &names {
                for tail in ["f0", "f1.x", "a0.f2", "a1.a2.f0", "v.f1", "nope", "a3"] {
                    queries.push(format!("{m}.{tail}"));
                }
            }
            queries.push("p".into());
            queries.push("p.q".into());
            for _ in 0..2 {
                for q in &queries {
                    let new = tree.resolve_path_with(q, &lookup, &memo);
                    let alone = tree.resolve_path_with(q, &lookup, &ResolveMemo::default());
                    assert_eq!(new, alone, "{q}");
                    let old = tree.reference_resolve_path(q, lookup);
                    let starred = |p: &LibraryPath<'_>, main: bool| {
                        p.ends.iter().any(|e| {
                            (!main || !e.side)
                                && e.lookup == LibraryLookup::Unknown
                                && lookup_star(&tree, &e.path, &lookup)
                        })
                    };
                    if starred(&old, false) {
                        if starred(&old, true) {
                            assert_eq!(new.end, LibraryLookup::Unknown, "{q}");
                            assert_eq!(old.end, LibraryLookup::Unknown, "{q}");
                        } else {
                            assert_eq!(new.end, old.end, "{q}");
                        }
                        for (r, symbol) in &old.names {
                            assert!(
                                new.names
                                    .iter()
                                    .any(|(r2, s2)| r2 == r && s2.name == symbol.name),
                                "{q}: {}",
                                symbol.name
                            );
                        }
                        stars += 1;
                    } else {
                        assert_eq!(new.end, old.end, "{q}");
                        assert_eq!(new.names, old.names, "{q}");
                        assert_eq!(new.modules, old.modules, "{q}");
                        assert_eq!(new.paths, old.paths, "{q}");
                        assert_eq!(new.values, dedup(&old.values), "{q}");
                        assert_eq!(new.ends, old.ends, "{q}");
                        assert_eq!(new.hops, old.hops, "{q}");
                    }
                    checked += 1;
                }
                // The other way round, with a new memo.
                memo = ResolveMemo::default();
                queries.reverse();
            }
        }
        assert!(checked > 5_000, "{checked}");
        assert!(stars > 1_000, "{stars}");
    }

    #[test]
    fn resolution() {
        let tree = LibraryTree::new([
            ("core/format", ResourceId(1)),
            ("core/tags", ResourceId(2)),
            ("legacy", ResourceId(3)),
            ("legacy/helpers", ResourceId(4)),
        ]);
        let format = ModuleSymbols {
            symbols: vec![sym("withUnit")],
            dynamic: false,
            constants: Vec::new(),
            ..ModuleSymbols::default()
        };
        let legacy = ModuleSymbols::default();
        let lookup = |r: ResourceId| match r.0 {
            1 => Some(&format),
            3 => Some(&legacy),
            _ => None,
        };
        assert!(matches!(
            tree.resolve("core.format.withUnit", lookup),
            LibraryLookup::Symbol { rest: 0, .. }
        ));
        assert!(matches!(
            tree.resolve("core.format.withUnit.x", lookup),
            LibraryLookup::Symbol { rest: 1, .. }
        ));
        assert!(matches!(
            tree.resolve("core.format.nope", lookup),
            LibraryLookup::MissingAttribute { index: 2, .. }
        ));
        assert!(matches!(
            tree.resolve("core.nope", lookup),
            LibraryLookup::Missing { index: 1, .. }
        ));
        assert!(matches!(
            tree.resolve("core", lookup),
            LibraryLookup::Package { .. }
        ));
        assert!(matches!(
            tree.resolve("core.tags", lookup),
            LibraryLookup::Module { .. }
        ));
        assert!(matches!(
            tree.resolve("core.tags.read", lookup),
            LibraryLookup::Unknown
        ));
        assert!(matches!(
            tree.resolve("legacy.helpers", lookup),
            LibraryLookup::HiddenByModule { index: 1, .. }
        ));
        assert_eq!(
            tree.resolve("system.tag", lookup),
            LibraryLookup::NotLibrary
        );
        assert_eq!(
            tree.shadowed_packages(),
            vec![("legacy", vec!["legacy.helpers"])]
        );
    }

    /// Review round 27: every segment of a dotted library reference is
    /// judged, default deny.
    #[test]
    fn value_uses_are_judged_at_every_segment() {
        let tree = LibraryTree::new([
            ("core/format", ResourceId(1)),
            ("core/tags", ResourceId(2)),
            ("m", ResourceId(3)),
        ]);
        let format = ModuleSymbols {
            symbols: vec![sym("withUnit")],
            ..ModuleSymbols::default()
        };
        let symbol = |name: &str, kind: SymbolKind| Symbol {
            name: name.into(),
            kind,
            range: TextRange::default(),
        };
        let m = ModuleSymbols {
            symbols: vec![
                sym("f"),
                symbol("C", SymbolKind::Class),
                symbol("V", SymbolKind::Variable),
                symbol("K", SymbolKind::Variable),
                symbol(
                    "re",
                    SymbolKind::Import {
                        target: Some("re".into()),
                    },
                ),
                symbol(
                    "fmt",
                    SymbolKind::Import {
                        target: Some("core.format".into()),
                    },
                ),
            ],
            constants: vec!["K".into()],
            ..ModuleSymbols::default()
        };
        let lookup = |r: ResourceId| match r.0 {
            1 => Some(&format),
            3 => Some(&m),
            _ => None,
        };
        let module = |r: u32, dotted: &str| {
            Some(LibraryObject::Module {
                resource: ResourceId(r),
                dotted: dotted.into(),
            })
        };
        for (dotted, expected) in [
            ("m.f", None),
            ("m.f.__name__", None),
            ("m.f.func_name", None),
            ("m.f.func_globals", module(3, "m")),
            ("m.f.func_globals.has_key", module(3, "m")),
            ("m.f.__globals__.get", module(3, "m")),
            ("m.f.__call__.__self__", module(3, "m")),
            ("m.f.func_code", module(3, "m")),
            ("m.f.cache", module(3, "m")),
            ("m.C.meth", None),
            ("m.C.X.upper", None),
            ("m.C.meth.im_func", module(3, "m")),
            ("m.C.__dict__", module(3, "m")),
            ("m.V.get", None),
            ("m.V.dict", None),
            ("m.V.clear", None),
            ("m.V.x.gi_frame", module(3, "m")),
            ("m.V.__class__", module(3, "m")),
            ("m.K.clear", None),
            ("m.K.__class__", module(3, "m")),
            ("m.re.compile", None),
            ("m.re.__dict__", module(3, "m")),
            ("m.fmt.withUnit", None),
            ("m.fmt.withUnit.func_globals", module(1, "core.format")),
            ("m.fmt.dict", module(1, "core.format")),
            ("m.nope", None),
            ("m.nope.func_globals", module(3, "m")),
            ("m.dict", module(3, "m")),
            ("m.__name__", None),
            ("core.tags.read", None),
            ("core.tags.read.func_globals", module(2, "core.tags")),
            ("core.tags.dict", module(2, "core.tags")),
            (
                "core.dict",
                Some(LibraryObject::Package {
                    dotted: "core".into(),
                }),
            ),
            (
                "core.nope.__dict__",
                Some(LibraryObject::Package {
                    dotted: "core".into(),
                }),
            ),
            ("core.format.withUnit.__code__", module(1, "core.format")),
            ("system.util.func_globals", None),
        ] {
            assert_eq!(tree.object_member(dotted, lookup), expected, "{dotted}");
        }
    }

    /// Review round 28: one resolver follows the imports of library
    /// modules, records every name it reads on the way, keeps segment
    /// indexes in the path as written, and denies by default where it
    /// cannot follow.
    #[test]
    fn paths_follow_library_imports() {
        let mut modules: Vec<(String, ResourceId)> = vec![
            ("lib/util".into(), ResourceId(0)),
            ("m0".into(), ResourceId(1)),
            ("m1".into(), ResourceId(2)),
            ("s".into(), ResourceId(3)),
            ("v".into(), ResourceId(4)),
        ];
        for i in 0..=MAX_IMPORT_HOPS + 1 {
            modules.push((format!("c{i}"), ResourceId(10 + i as u32)));
        }
        let tree = LibraryTree::new(modules.iter().map(|(p, r)| (p.as_str(), *r)));
        let symbol = |name: &str, kind: SymbolKind| Symbol {
            name: name.into(),
            kind,
            range: TextRange::default(),
        };
        let import = |name: &str, target: &str| {
            symbol(
                name,
                SymbolKind::Import {
                    target: Some(target.into()),
                },
            )
        };
        let util = ModuleSymbols {
            symbols: vec![import("json", "json"), sym("f")],
            ..ModuleSymbols::default()
        };
        let m0 = ModuleSymbols {
            symbols: vec![import("util", "lib.util"), import("x", "m1.x")],
            ..ModuleSymbols::default()
        };
        let m1 = ModuleSymbols {
            symbols: vec![import("m0", "m0"), import("x", "m0.x")],
            ..ModuleSymbols::default()
        };
        let s = ModuleSymbols {
            dynamic: true,
            star_imports: vec!["m0".into()],
            ..ModuleSymbols::default()
        };
        let v = ModuleSymbols {
            symbols: vec![symbol("util", SymbolKind::Variable)],
            variable_imports: vec![VariableImport {
                name: "util".into(),
                target: "lib.util".into(),
                several: true,
            }],
            ..ModuleSymbols::default()
        };
        let chain: Vec<ModuleSymbols> = (0..=MAX_IMPORT_HOPS + 1)
            .map(|i| ModuleSymbols {
                symbols: vec![
                    import("a", &format!("c{}", i + 1)),
                    import("util", "lib.util"),
                ],
                ..ModuleSymbols::default()
            })
            .collect();
        let lookup = |r: ResourceId| match r.0 {
            0 => Some(&util),
            1 => Some(&m0),
            2 => Some(&m1),
            3 => Some(&s),
            4 => Some(&v),
            n => chain.get(n as usize - 10),
        };
        let names = |p: &LibraryPath<'_>| -> Vec<(u32, String)> {
            p.names.iter().map(|(r, s)| (r.0, s.name.clone())).collect()
        };
        let module = |r: u32, dotted: &str| LibraryObject::Module {
            resource: ResourceId(r),
            dotted: dotted.into(),
        };

        let p = tree.resolve_path("m0.util.json.dumps", lookup);
        assert!(matches!(
            p.end,
            LibraryLookup::Symbol { resource: ResourceId(0), rest: 1, symbol } if symbol.name == "json"
        ));
        assert_eq!(names(&p), [(1, "util".into()), (0, "json".into())]);
        assert_eq!(p.modules, [ResourceId(1), ResourceId(0)]);
        assert_eq!(p.paths, ["lib.util.json.dumps"]);
        assert!(p.values.is_empty());
        // `m0.util` itself is the module the import binds.
        assert!(matches!(
            tree.resolve("m0.util", lookup),
            LibraryLookup::Module {
                resource: ResourceId(0),
                ..
            }
        ));
        // A miss in the target, at its index in the path as written.
        assert!(matches!(
            tree.resolve("m0.util.nope", lookup),
            LibraryLookup::MissingAttribute {
                index: 2,
                resource: ResourceId(0)
            }
        ));
        // Through module objects that import each other.
        let p = tree.resolve_path("m0.x.m0.util.f", lookup);
        assert!(matches!(p.end, LibraryLookup::Unknown), "{:?}", p.end);
        let p = tree.resolve_path("m1.m0.util.f", lookup);
        assert!(matches!(
            p.end,
            LibraryLookup::Symbol {
                resource: ResourceId(0),
                rest: 0,
                ..
            }
        ));
        // A cycle of names (`m0.x` is `m1.x` is `m0.x`): unknown, and both
        // modules count as used as values.
        let p = tree.resolve_path("m0.x.json", lookup);
        assert_eq!(p.end, LibraryLookup::Unknown);
        assert!(p.values.contains(&module(1, "m0")), "{:?}", p.values);
        assert!(p.values.contains(&module(2, "m1")), "{:?}", p.values);
        // Longer than the bound: unknown, values.
        let long = format!("c0{}.util.json", ".a".repeat(MAX_IMPORT_HOPS + 1));
        let p = tree.resolve_path(&long, lookup);
        assert_eq!(p.end, LibraryLookup::Unknown);
        assert!(!p.values.is_empty());
        let within = format!("c0{}.util.json", ".a".repeat(MAX_IMPORT_HOPS - 1));
        assert!(matches!(
            tree.resolve(&within, lookup),
            LibraryLookup::Symbol {
                resource: ResourceId(0),
                ..
            }
        ));
        // A star import: the name may come from the module it imports.
        let p = tree.resolve_path("s.util.json", lookup);
        assert_eq!(p.end, LibraryLookup::Unknown);
        assert!(names(&p).contains(&(0, "json".into())), "{:?}", names(&p));
        // A name with several bindings: ends there, reads through the
        // import, and the import's target counts as used as a value.
        let p = tree.resolve_path("v.util.json", lookup);
        assert!(matches!(
            p.end,
            LibraryLookup::Symbol {
                resource: ResourceId(4),
                rest: 1,
                ..
            }
        ));
        assert!(names(&p).contains(&(0, "json".into())));
        assert_eq!(p.values, [module(0, "lib.util")]);
        // Value uses are judged where the path ends.
        assert_eq!(
            tree.object_member("m0.util.f.func_globals", lookup),
            Some(module(0, "lib.util"))
        );
        assert_eq!(
            tree.object_member("m0.util.dict", lookup),
            Some(module(0, "lib.util"))
        );
        assert_eq!(tree.object_member("m0.util.f", lookup), None);
    }

    #[test]
    fn signatures() {
        let p = |name: &str, has_default: bool| Parameter {
            name: name.into(),
            kind: ParameterKind::Positional,
            has_default,
        };
        let sig = Signature {
            params: vec![p("value", false), p("digits", true)],
            ..Signature::default()
        };
        assert!(sig.check_call(1, &[], false, false).is_ok());
        assert!(sig.check_call(1, &["digits"], false, false).is_ok());
        assert!(matches!(
            sig.check_call(0, &[], false, false),
            Err(CallMismatch::TooFewArguments { .. })
        ));
        assert!(matches!(
            sig.check_call(3, &[], false, false),
            Err(CallMismatch::TooManyArguments { .. })
        ));
        assert!(matches!(
            sig.check_call(1, &["x"], false, false),
            Err(CallMismatch::UnknownKeyword(_))
        ));
        assert!(matches!(
            sig.check_call(1, &["value"], false, false),
            Err(CallMismatch::DuplicateArgument(_))
        ));
        assert!(sig.check_call(0, &[], true, false).is_ok());
    }

    #[test]
    fn module_names() {
        assert!(is_valid_module_path("core/format"));
        assert!(is_valid_module_path("_x/y1"));
        assert!(!is_valid_module_path("core/1format"));
        assert!(!is_valid_module_path("my-module"));
    }
}
