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

/// The library object (a module or a package) whose own member a dotted
/// name reaches ([`LibraryTree::object_member`]): code that reads it may
/// read and change everything in the object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryObject {
    Module {
        resource: ResourceId,
        dotted: String,
    },
    Package {
        dotted: String,
    },
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

    /// Resolves `pkg.mod.func...`. `symbols` returns the summary of a module
    /// resource, if known.
    pub fn resolve<'i>(
        &self,
        dotted: &str,
        symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> LibraryLookup<'i> {
        let segments: Vec<&str> = dotted.split('.').collect();
        let Some(&root) = segments.first().and_then(|s| self.roots.get(*s)) else {
            return LibraryLookup::NotLibrary;
        };
        let mut at = root;
        for (i, seg) in segments.iter().enumerate().skip(1) {
            let node = &self.nodes[at];
            if let Some(resource) = node.resource {
                return match symbols(resource) {
                    Some(syms) => match syms.get(seg) {
                        Some(symbol) => LibraryLookup::Symbol {
                            resource,
                            symbol,
                            rest: segments.len() - i - 1,
                        },
                        None if node.children.contains_key(*seg) => LibraryLookup::HiddenByModule {
                            index: i,
                            module: node.dotted.clone(),
                        },
                        // A member of the module object itself (`m.dict`,
                        // `m.__dict__`): it exists, and what it reaches is
                        // not a name of the module.
                        None if module_member(seg) || dunder(seg) => LibraryLookup::Unknown,
                        None if syms.dynamic => LibraryLookup::Unknown,
                        None => LibraryLookup::MissingAttribute { index: i, resource },
                    },
                    None if node.children.contains_key(*seg) => LibraryLookup::HiddenByModule {
                        index: i,
                        module: node.dotted.clone(),
                    },
                    None => LibraryLookup::Unknown,
                };
            }
            match node.children.get(*seg) {
                Some(&child) => at = child,
                // A member of the package object (`pkg.dict`, `pkg.clear`).
                None if package_member(seg) || dunder(seg) => return LibraryLookup::Unknown,
                None => {
                    return LibraryLookup::Missing {
                        index: i,
                        parent: node.dotted.clone(),
                    };
                }
            }
        }
        let node = &self.nodes[at];
        match node.resource {
            Some(resource) => LibraryLookup::Module {
                resource,
                dotted: node.dotted.clone(),
            },
            None => LibraryLookup::Package {
                dotted: node.dotted.clone(),
            },
        }
    }

    /// The module or package whose own member `dotted` reaches, if any:
    /// a member of Ignition's module or package object
    /// ([`MODULE_OBJECT_MEMBERS`], [`PACKAGE_OBJECT_MEMBERS`]) that the
    /// module does not bind (or the package has no child of that name),
    /// or a special attribute ([`is_special_attribute`]). A module whose
    /// symbols are not known may bind anything: its members count.
    pub fn object_member<'i>(
        &self,
        dotted: &str,
        symbols: impl Fn(ResourceId) -> Option<&'i ModuleSymbols>,
    ) -> Option<LibraryObject> {
        let mut segments = dotted.split('.');
        let mut at = *self.roots.get(segments.next()?)?;
        for seg in segments {
            let node = &self.nodes[at];
            if let Some(resource) = node.resource {
                let bound = symbols(resource).is_some_and(|s| s.get(seg).is_some());
                return (!bound && !node.children.contains_key(seg) && module_member(seg)).then(
                    || LibraryObject::Module {
                        resource,
                        dotted: node.dotted.clone(),
                    },
                );
            }
            match node.children.get(seg) {
                Some(&child) => at = child,
                None => {
                    return package_member(seg).then(|| LibraryObject::Package {
                        dotted: node.dotted.clone(),
                    });
                }
            }
        }
        None
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
