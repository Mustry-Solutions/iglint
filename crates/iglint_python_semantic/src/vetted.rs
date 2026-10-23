//! Code that iglint understands well enough to keep safe fixes: the
//! vetted modules, Java classes, builtins and attributes.
//!
//! # Threat model
//!
//! Some safe fixes hold only when nothing reads a name dynamically: the
//! module's namespace (`SCR102` removes an import), a caller's locals
//! (`NAME001` renames a component that a script holds in a variable,
//! `VIEW015` removes a property that a script reaches through `self`).
//! Enumerating the ways to read names dynamically failed six review
//! rounds in a row: every round found another module, builtin or Java API
//! that resolves names from data (`cPickle.loads` of a `c__builtin__`
//! opcode calls `eval` in the caller's frame, `str.format` reads
//! attributes named by a computed string, Java reflection, ...).
//!
//! So these fixes are safe only for code that uses nothing but **vetted**
//! facilities and the language constructs the default-deny walkers
//! analyse ([`crate::dynamic`] and the linter's access analysis).
//! [`unknown_code`] finds every use of anything else, which keeps the fix
//! (it becomes unsafe). The allowlists below are default deny: a module,
//! Java class, builtin or module attribute that is not listed is unknown.
//!
//! Each module, Java class and builtin here was checked on Ignition 8.3's
//! Jython (`oracle`): it cannot evaluate code, read a frame, or look up
//! callables or attributes by a name taken from data. The *attributes* of
//! the vetted Python modules are not an allowlist in this file: they are
//! measured inside the Ignition image by `stubs-gen/gen_vetted.py` and
//! emitted into [`crate::generated`] as a positive list of the attributes
//! that are a function, a class or a constant of a vetted immutable type.
//! Everything else is unknown: a module (`os.path.os`), a `PySystemState`
//! (`re.sys`, which Jython does not report as a `types.ModuleType`), a
//! Java class (`base64.binascii`), a private attribute (`random._inst`) or
//! any other object. The generator also measures the Java member names
//! that reach reflection ([`crate::generated::JAVA_REFLECTIVE_MEMBERS`],
//! consumed by [`crate::dynamic`]). Module objects used as values are not
//! vetted either (an attribute read through an alias is not checked), and
//! subclassing a Java class is unknown (its instances are Java-backed).

use iglint_python::ast::{Alias, Expr, ExprContext, ExprName, Operator, Stmt, StmtImportFrom};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_text::TextRange;

use crate::{Binding, BindingKind, Resolution, ScopeId, ScopeKind, SemanticModel};

/// Python modules whose every public, non-module attribute is vetted,
/// with extra attributes that are not.
///
/// * `string`: `Formatter` reads attributes named by its format string;
///   `Template` only looks names up in the mapping it is given.
/// * `collections`: `namedtuple` runs generated code in a namespace of its
///   own, with type and field names it checks to be identifiers.
/// * `json`: `object_hook` and `cls` take values, not names.
/// * `operator`: `attrgetter` and `methodcaller` look attributes up by
///   name; the other functions apply operators and item access.
const VETTED_MODULES: &[(&str, &[&str])] = &[
    ("__future__", &[]),
    ("base64", &[]),
    ("binascii", &[]),
    ("bisect", &[]),
    ("calendar", &[]),
    ("cmath", &[]),
    ("collections", &[]),
    ("datetime", &[]),
    ("decimal", &[]),
    ("fractions", &[]),
    ("hashlib", &[]),
    ("heapq", &[]),
    ("itertools", &[]),
    ("json", &[]),
    ("math", &[]),
    ("operator", &["attrgetter", "methodcaller"]),
    ("os.path", &[]),
    ("random", &[]),
    ("re", &[]),
    ("string", &["Formatter"]),
    ("struct", &[]),
    ("textwrap", &[]),
    ("time", &[]),
    ("uuid", &[]),
];

/// `os` and `sys` hand out frames, every module, the import path or
/// processes: only these attributes are vetted.
const OS_ATTRIBUTES: &[&str] = &[
    "F_OK",
    "R_OK",
    "W_OK",
    "X_OK",
    "access",
    "altsep",
    "curdir",
    "devnull",
    "environ",
    "error",
    "extsep",
    "getcwd",
    "getcwdu",
    "getenv",
    "linesep",
    "listdir",
    "lstat",
    "makedirs",
    "mkdir",
    "name",
    "pardir",
    "path",
    "pathsep",
    "remove",
    "removedirs",
    "rename",
    "renames",
    "rmdir",
    "sep",
    "stat",
    "strerror",
    "unlink",
    "urandom",
    "utime",
    "walk",
];

const SYS_ATTRIBUTES: &[&str] = &[
    "byteorder",
    "exit",
    "float_info",
    "getdefaultencoding",
    "getfilesystemencoding",
    "getrecursionlimit",
    "hexversion",
    "maxint",
    "maxsize",
    "maxunicode",
    "platform",
    "stderr",
    "stdout",
    "version",
    "version_info",
];

/// Java packages whose classes are vetted, except those listed.
/// `ServiceLoader` and the resource bundles load classes by name.
const VETTED_JAVA_PACKAGES: &[(&str, &[&str])] = &[
    (
        "java.util",
        &[
            "ServiceLoader",
            "ResourceBundle",
            "ListResourceBundle",
            "PropertyResourceBundle",
        ],
    ),
    ("java.util.concurrent", &[]),
    ("java.util.concurrent.atomic", &[]),
    ("java.util.concurrent.locks", &[]),
    ("java.util.function", &[]),
    ("java.util.regex", &[]),
    ("java.util.stream", &[]),
    ("java.math", &[]),
    ("java.text", &[]),
    ("java.time", &[]),
    ("java.time.chrono", &[]),
    ("java.time.format", &[]),
    ("java.time.temporal", &[]),
    ("java.nio.charset", &[]),
];

/// Single vetted classes of other Java packages. `java.lang` minus
/// `Class`, `ClassLoader`, `Thread`, `Throwable`, `Runtime`, `Process*`,
/// reflection and the stack walker; `java.io` minus object streams
/// (deserialization instantiates classes named by the data); `java.net`
/// minus class loaders. The exception classes hand out only stack traces
/// (class, method, file and line), which safe fixes never change.
const VETTED_JAVA_CLASSES: &[(&str, &[&str])] = &[
    (
        "java.lang",
        &[
            "ArithmeticException",
            "Boolean",
            "Byte",
            "Character",
            "Double",
            "Exception",
            "Float",
            "IllegalArgumentException",
            "IllegalStateException",
            "IndexOutOfBoundsException",
            "Integer",
            "InterruptedException",
            "Long",
            "Math",
            "NullPointerException",
            "Number",
            "NumberFormatException",
            "RuntimeException",
            "Short",
            "StrictMath",
            "String",
            "StringBuffer",
            "StringBuilder",
            "System",
            "UnsupportedOperationException",
        ],
    ),
    (
        "java.io",
        &[
            "BufferedInputStream",
            "BufferedOutputStream",
            "BufferedReader",
            "BufferedWriter",
            "ByteArrayInputStream",
            "ByteArrayOutputStream",
            "File",
            "FileInputStream",
            "FileNotFoundException",
            "FileOutputStream",
            "FileReader",
            "FileWriter",
            "IOException",
            "InputStreamReader",
            "OutputStreamWriter",
            "PrintWriter",
            "StringReader",
            "StringWriter",
        ],
    ),
    (
        "java.net",
        &[
            "HttpURLConnection",
            "InetAddress",
            "MalformedURLException",
            "SocketTimeoutException",
            "URI",
            "URL",
            "URLDecoder",
            "URLEncoder",
        ],
    ),
];

/// Members of vetted Java classes that are not vetted: `System` loads
/// native libraries by name and installs security managers.
const JAVA_DENIED_MEMBERS: &[(&str, &[&str])] = &[(
    "java.lang.System",
    &[
        "load",
        "loadLibrary",
        "mapLibraryName",
        "getSecurityManager",
        "setSecurityManager",
        "inheritedChannel",
    ],
)];

/// `system.*` functions and objects that store or run code from run-time
/// values, or hand out the script context: never vetted. See the audit in
/// `docs/internal/fix-safety.md` ("`system.*` functions that run code").
const SYSTEM_RUNS_CODE: &[&str] = &[
    "system.alarm.SERIALIZER",
    "system.eam.runTask",
    "system.groups.loadFromFile",
    "system.project.requestScan",
    "system.system.WRAPPED_CONTEXT",
    "system.system.execute",
    "system.tag.addTag",
    "system.tag.configure",
    "system.tag.configureManagedProvider",
    "system.tag.configureManagedTag",
    "system.tag.copy",
    "system.tag.editAlarmConfig",
    "system.tag.editTag",
    "system.tag.editTags",
    "system.tag.importTags",
    "system.tag.loadFromFile",
    "system.util.WRAPPED_CONTEXT",
    "system.util.execute",
    "system.vision.updateProject",
];

/// `system.tag` writes: a path may name a configuration property
/// (`Tag.expression`, `Tag.eventScripts`) and store code. Vetted only as
/// direct calls whose paths are value writes ([`value_write_paths`]).
const SYSTEM_TAG_WRITES: &[&str] = &[
    "system.tag.write",
    "system.tag.writeAll",
    "system.tag.writeAllSynchronous",
    "system.tag.writeAsync",
    "system.tag.writeBlocking",
    "system.tag.writeSynchronous",
    "system.tag.writeToTag",
    "system.tag.writeToTagSynchronous",
    "system.tag.writeToTags",
];

/// Whether `e`, the path argument of a `system.tag` write, can only name
/// tag values: a path or a list or tuple of paths, each [`value_write_path`].
fn value_write_paths(e: &Expr) -> bool {
    match e {
        Expr::List(l) => l.elts.iter().all(value_write_path),
        Expr::Tuple(t) => t.elts.iter().all(value_write_path),
        _ => value_write_path(e),
    }
}

/// Whether the tag path `e` can only name a tag's value: the text after its
/// last `/` (or `]`) is known and names no property other than `Value`,
/// `Quality` or `Timestamp`. A literal; a concatenation whose last operand
/// is such a literal containing `/`; a `%` or `format` string with such a
/// literal tail. Anything else may end in `.expression`.
fn value_write_path(e: &Expr) -> bool {
    /// The known tail of a path text: after the last `/` or `]`, when
    /// there is one (`whole`: the text is the whole path).
    fn tail_ok(text: &str, whole: bool, placeholder: &[char]) -> bool {
        let cut = text.rfind(['/', ']']);
        let tail = match cut {
            Some(i) => &text[i + 1..],
            None if whole => text,
            None => return false,
        };
        if tail.contains(placeholder) {
            return false;
        }
        match tail.split_once('.') {
            None => true,
            Some((_, property)) => {
                ["value", "quality", "timestamp"].contains(&property.to_ascii_lowercase().as_str())
            }
        }
    }
    fn literal(e: &Expr) -> Option<String> {
        match e {
            Expr::Str(s) => Some(s.s.to_latin1().into_owned()),
            _ => None,
        }
    }
    match e {
        Expr::Str(s) => tail_ok(&s.s.to_latin1(), true, &[]),
        Expr::BinOp(b) if b.op == Operator::Add => {
            literal(&b.right).is_some_and(|t| tail_ok(&t, false, &[]))
        }
        Expr::BinOp(b) if b.op == Operator::Mod => {
            literal(&b.left).is_some_and(|t| tail_ok(&t, true, &['%']))
        }
        Expr::Call(c) => match &*c.func {
            Expr::Attribute(a) if a.attr.as_str() == "format" => {
                literal(&a.value).is_some_and(|t| tail_ok(&t, true, &['{', '}']))
            }
            _ => false,
        },
        _ => false,
    }
}

/// Builtins that cannot evaluate code, read a frame or look a name up.
/// Exception classes are vetted too. Not vetted: `eval`, `execfile`,
/// `compile`, `__import__`, `reload`, `input` (evaluates what it reads),
/// `locals`, `globals`, `vars`, `dir`, `help` (imports modules by name).
/// `getattr`, `setattr`, `delattr` and `hasattr` are vetted only as direct
/// calls with a literal name ([`crate::dynamic`] makes every other use an
/// escape).
const VETTED_BUILTINS: &[&str] = &[
    "Ellipsis",
    "False",
    "None",
    "NotImplemented",
    "True",
    "__debug__",
    "__doc__",
    "__name__",
    "abs",
    "all",
    "any",
    "apply",
    "basestring",
    "bin",
    "bool",
    "buffer",
    "bytearray",
    "bytes",
    "callable",
    "chr",
    "classmethod",
    "cmp",
    "coerce",
    "complex",
    "copyright",
    "credits",
    "delattr",
    "dict",
    "divmod",
    "enumerate",
    "exit",
    "file",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "hasattr",
    "hash",
    "hex",
    "id",
    "int",
    "intern",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "license",
    "list",
    "long",
    "map",
    "max",
    "memoryview",
    "min",
    "next",
    "object",
    "oct",
    "open",
    "ord",
    "pow",
    "print",
    "property",
    "quit",
    "range",
    "raw_input",
    "reduce",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "slice",
    "sorted",
    "staticmethod",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "unichr",
    "unicode",
    "xrange",
    "zip",
];

/// Whether a builtin is vetted.
fn builtin_vetted(name: &str) -> bool {
    VETTED_BUILTINS.binary_search(&name).is_ok()
        || iglint_stubs::jython_builtins()
            .iter()
            .any(|b| b.name == name && b.kind == iglint_stubs::BuiltinKind::Exception)
}

/// What a name may be bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    /// A vetted Python module (its key in [`VETTED_MODULES`], or `os`,
    /// `sys`, `system`).
    Module(&'static str),
    /// A vetted Java class (qualified name).
    JavaClass(&'static str),
    /// Something else that is vetted (a function or class of a vetted
    /// module, a vetted builtin).
    Value,
    /// A value the code computes (an assignment, a parameter, a `def`, a
    /// project library module the caller checks): not a vetted facility
    /// and not checked here.
    Plain,
    /// An import, builtin or injected name that is not vetted.
    Unknown,
}

fn vetted_module(name: &str) -> Option<&'static str> {
    match name {
        "os" => Some("os"),
        "sys" => Some("sys"),
        "system" => Some("system"),
        _ => VETTED_MODULES
            .iter()
            .find(|(m, _)| *m == name)
            .map(|(m, _)| *m),
    }
}

/// Whether `attr` of the vetted module `module` is vetted, and what it is.
fn module_attribute(module: &'static str, attr: &str) -> Option<Bound> {
    let dunder = attr.starts_with("__") && attr.ends_with("__");
    if dunder {
        return matches!(attr, "__name__" | "__doc__" | "__file__").then_some(Bound::Value);
    }
    match module {
        // Ignition's scripting API: no function evaluates code, reads the
        // caller's frame or looks a callable up by name (callables passed
        // to `invokeAsynchronous` and friends are called by value).
        "system" => Some(Bound::Module("system")),
        "os" => match attr {
            "path" => Some(Bound::Module("os.path")),
            _ => OS_ATTRIBUTES.contains(&attr).then_some(Bound::Value),
        },
        "sys" => SYS_ATTRIBUTES.contains(&attr).then_some(Bound::Value),
        // `__future__` features are compile-time directives (`_Feature`
        // instances); they do nothing at run time and are always vetted.
        "__future__" => Some(Bound::Value),
        "os.path" if attr == "path" => Some(Bound::Module("os.path")),
        _ => {
            if attr.starts_with('_') {
                return None;
            }
            // The positive, machine-measured allowlist: an attribute is
            // vetted only when the generator (inside the Ignition image)
            // measured it as a function, a class or a constant of a vetted
            // immutable type. A module, a `PySystemState` (`re.sys`), a
            // Java class or anything else is unknown. The per-module
            // denylist still removes members that look up names by data
            // (`string.Formatter`, `operator.attrgetter`).
            let denied = VETTED_MODULES
                .iter()
                .find(|(m, _)| *m == module)
                .map_or(&[][..], |(_, d)| *d);
            if denied.contains(&attr) {
                return None;
            }
            crate::generated::MODULE_VETTED_ATTRS
                .iter()
                .find(|(m, _)| *m == module)
                .is_some_and(|(_, attrs)| attrs.binary_search(&attr).is_ok())
                .then_some(Bound::Value)
        }
    }
}

/// The vetted Java class `package.class`, if it is one.
fn java_class(package: &str, class: &str) -> Option<&'static str> {
    let qualified = |list: &'static [(&'static str, &'static [&'static str])]| {
        list.iter().find(|(p, _)| *p == package)
    };
    if let Some((_, classes)) = qualified(VETTED_JAVA_CLASSES)
        && let Some(c) = classes.iter().find(|c| **c == class)
    {
        return Some(denied_members_key(package, c));
    }
    if let Some((p, denied)) = qualified(VETTED_JAVA_PACKAGES)
        && !denied.contains(&class)
        && class.chars().next().is_some_and(char::is_uppercase)
    {
        return Some(denied_members_key(p, class));
    }
    None
}

/// The key of `package.class` in [`JAVA_DENIED_MEMBERS`], or `java` for
/// a class without denied members.
fn denied_members_key(package: &str, class: &str) -> &'static str {
    JAVA_DENIED_MEMBERS
        .iter()
        .map(|(q, _)| *q)
        .find(|q| q.strip_prefix(package).and_then(|r| r.strip_prefix('.')) == Some(class))
        .unwrap_or("java")
}

/// What [`unknown_code`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnknownCode {
    /// The first use (in source order) of something that is not vetted and
    /// not an import of a module that may be a project library module.
    pub other: Option<TextRange>,
    /// Imports of modules that are not vetted, by dotted name, which the
    /// caller may know to be plain project library modules.
    pub imports: Vec<(String, TextRange)>,
    /// The top-level names of the vetted modules the code imports: a
    /// project library package of the same name would be imported
    /// instead.
    pub vetted_imports: Vec<String>,
}

impl UnknownCode {
    pub fn is_empty(&self) -> bool {
        self.other.is_none() && self.imports.is_empty()
    }
}

/// Every use in the module of `model` (nested scopes included) of a
/// module, Java class, builtin or module attribute that is not vetted. See
/// the module documentation.
///
/// Names are resolved through the semantic model's scopes, not in the
/// order the walk meets them, and flow-insensitively: a name may stand for
/// every binding of it in the scope it resolves to, whatever the textual
/// order (a function defined before `import re` still reads the module
/// `re`), plus, where Python looks further at run time (module and class
/// bodies, `global` names), the injected name or builtin of that spelling.
/// A use is unknown when **any** of those bindings is unvetted (fail
/// closed). A name the model has no reference for, or that resolves
/// dynamically (`from x import *`, `exec`), is unknown. A name bound
/// nowhere is a builtin (vetted builtins only), Ignition's `sys` or
/// `system`, or a name the caller resolves as a project library name
/// (`library_names_used` in the linter), which raises `NameError`
/// otherwise.
pub fn unknown_code(model: &SemanticModel<'_>) -> UnknownCode {
    let bound = model
        .bindings()
        .map(|(_, b)| binding_bound(b))
        .collect::<Vec<_>>();
    let mut v = Walk {
        model,
        bound,
        out: UnknownCode::default(),
    };
    for stmt in &model.module.body {
        v.visit_stmt(stmt);
    }
    v.out
}

/// What the import alias `a` of `import ...` binds.
fn import_bound(a: &Alias) -> Bound {
    let name = a.name.as_str();
    let root = name.split('.').next().unwrap_or(name);
    // `import a.b` binds `a`; `import a.b as c` binds `c` to `a.b`.
    let target = match &a.asname {
        Some(_) => name,
        None if name == "os.path" => "os",
        None if root == "system" => "system",
        None if name.contains('.') => "",
        None => name,
    };
    match vetted_module(target) {
        Some(m) => Bound::Module(m),
        None if root == "java" || root == "javax" || vetted_module(name).is_some() => {
            Bound::Unknown
        }
        // A module that may be a project library module: the caller checks
        // it ([`UnknownCode::imports`]).
        None => Bound::Plain,
    }
}

/// What the alias `a` of `from module import ...` binds.
fn from_import_bound(i: &StmtImportFrom, a: &Alias) -> Bound {
    let module = i.module.as_ref().map_or("", |m| m.as_str());
    let name = a.name.as_str();
    if i.level != 0 {
        return Bound::Plain;
    }
    if let Some(m) = vetted_module(module) {
        return match module_attribute(m, name) {
            Some(b) if name != "*" => b,
            _ => Bound::Unknown,
        };
    }
    if module == "java" || module.starts_with("java.") || module.starts_with("javax") {
        return java_class(module, name).map_or(Bound::Unknown, Bound::JavaClass);
    }
    Bound::Plain
}

/// What a binding binds, whatever the order it runs in.
fn binding_bound(b: &Binding<'_>) -> Bound {
    match b.kind {
        BindingKind::Import => b.alias.map_or(Bound::Unknown, import_bound),
        BindingKind::FromImport => match (b.stmt, b.alias) {
            (Some(Stmt::ImportFrom(i)), Some(a)) => from_import_bound(i, a),
            _ => Bound::Unknown,
        },
        BindingKind::FutureImport => Bound::Value,
        BindingKind::Builtin | BindingKind::Injected => implicit_bound(b.name),
        // Assignments, parameters, definitions, loop and `with` targets,
        // `global` declarations: values the code computes. What is done
        // with them is the default-deny walkers' business.
        _ => Bound::Plain,
    }
}

/// What a name that no binding of the code provides stands for: Ignition's
/// `sys` and `system`, a builtin (vetted or not), or a name the caller
/// resolves (a project library package, Ignition's other injected names;
/// `NameError` otherwise).
fn implicit_bound(name: &str) -> Bound {
    match name {
        "sys" => Bound::Module("sys"),
        "system" => Bound::Module("system"),
        "__builtins__" => Bound::Unknown,
        _ if iglint_stubs::is_jython_builtin(name) => {
            if builtin_vetted(name) {
                Bound::Value
            } else {
                Bound::Unknown
            }
        }
        _ => Bound::Plain,
    }
}

struct Walk<'m, 'a> {
    model: &'m SemanticModel<'a>,
    /// [`binding_bound`] of every binding, by binding id.
    bound: Vec<Bound>,
    out: UnknownCode,
}

impl Walk<'_, '_> {
    fn other(&mut self, range: TextRange) {
        if self.out.other.is_none_or(|o| range.start() < o.start()) {
            self.out.other = Some(range);
        }
    }

    /// Everything the `Load` name `n` may stand for at run time, flow
    /// insensitively (see [`unknown_code`]).
    fn candidates(&self, n: &ExprName) -> Vec<Bound> {
        let Some(r) = self.model.reference_at(n.range) else {
            // No reference: we cannot say what the name is. Fail closed.
            return vec![Bound::Unknown];
        };
        let name = &*n.id;
        let of = |scope: ScopeId| {
            self.model
                .scope(scope)
                .bindings_of(name)
                .iter()
                .map(|id| self.bound[id.index()])
        };
        let mut out: Vec<Bound> = Vec::new();
        let fallback = match r.resolution {
            Resolution::Local => {
                out.extend(of(r.scope));
                // Module and class bodies look names up by name at run
                // time (`LOAD_NAME`): before the local binding runs, a
                // class body reads the module's name, and both read the
                // injected names and builtins.
                match self.model.scope(r.scope).kind {
                    ScopeKind::Module => true,
                    ScopeKind::Class => {
                        out.extend(of(ScopeId::MODULE));
                        true
                    }
                    _ => false,
                }
            }
            Resolution::Free(scope) => {
                out.extend(of(scope));
                false
            }
            Resolution::Global => {
                out.extend(of(ScopeId::MODULE));
                true
            }
            Resolution::Builtin | Resolution::Injected | Resolution::Unresolved => true,
            Resolution::Dynamic => return vec![Bound::Unknown],
        };
        if fallback {
            out.push(implicit_bound(name));
        }
        out
    }

    /// Checks an attribute chain rooted at a name: each binding the root
    /// may stand for that is a vetted module or class must vet the chain;
    /// one that is not vetted makes the use unknown. Returns `false` when
    /// `expr` is not a chain rooted at a name.
    fn chain(&mut self, expr: &Expr) -> bool {
        self.chain_with(expr, false)
    }

    /// [`Walk::chain`]; `callee_checked` when `expr` is the callee of a
    /// call whose arguments [`Walk::visit_expr`] has checked (a
    /// `system.tag` write with value-only paths).
    fn chain_with(&mut self, expr: &Expr, callee_checked: bool) -> bool {
        let mut attrs = Vec::new();
        let mut root = expr;
        while let Expr::Attribute(a) = root {
            attrs.push((a.attr.as_str(), a.range));
            root = &a.value;
        }
        let Expr::Name(n) = root else { return false };
        attrs.reverse();
        for candidate in self.candidates(n) {
            if candidate == Bound::Module("system") {
                self.check_system(&attrs, callee_checked);
            }
            self.check_chain(candidate, n.range, &attrs);
        }
        true
    }

    /// Ignition's scripting API ("`system.*` functions that run code" in
    /// `docs/internal/fix-safety.md`): functions that store or run code
    /// from run-time values are not vetted, `system.tag` writes only with
    /// value-only paths ([`value_write_paths`]), and a package used as a
    /// value is not followed.
    fn check_system(&mut self, attrs: &[(&str, TextRange)], callee_checked: bool) {
        let mut dotted = String::from("system");
        for (i, (attr, range)) in attrs.iter().enumerate() {
            dotted.push('.');
            dotted.push_str(attr);
            let last = i + 1 == attrs.len();
            if SYSTEM_RUNS_CODE.contains(&dotted.as_str())
                || (SYSTEM_TAG_WRITES.contains(&dotted.as_str()) && !(last && callee_checked))
            {
                self.other(*range);
                return;
            }
            if last && iglint_stubs::system_package(&dotted).is_some() {
                // `t = system.tag`: what is read from it later is not
                // checked.
                self.other(*range);
            }
        }
    }

    fn check_chain(&mut self, mut bound: Bound, root: TextRange, attrs: &[(&str, TextRange)]) {
        if bound == Bound::Unknown {
            self.other(root);
            return;
        }
        for (attr, range) in attrs {
            bound = match bound {
                Bound::Module(m) => match module_attribute(m, attr) {
                    Some(next) => next,
                    None => {
                        self.other(*range);
                        return;
                    }
                },
                Bound::JavaClass(q) => {
                    if JAVA_DENIED_MEMBERS
                        .iter()
                        .any(|(c, members)| *c == q && members.contains(attr))
                    {
                        self.other(*range);
                    }
                    return;
                }
                Bound::Value | Bound::Plain | Bound::Unknown => return,
            };
        }
    }
}

impl Walk<'_, '_> {
    /// Whether `func` names a `system.tag` write function through a name
    /// that may be Ignition's `system`.
    fn system_write(&self, func: &Expr) -> bool {
        let mut attrs = Vec::new();
        let mut root = func;
        while let Expr::Attribute(a) = root {
            attrs.push(a.attr.as_str());
            root = &a.value;
        }
        let Expr::Name(n) = root else { return false };
        attrs.reverse();
        let dotted = format!("system.{}", attrs.join("."));
        n.ctx == ExprContext::Load
            && SYSTEM_TAG_WRITES.contains(&dotted.as_str())
            && self.candidates(n).contains(&Bound::Module("system"))
    }
}

impl<'a> Visitor<'a> for Walk<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(i) => {
                for a in &i.names {
                    let name = a.name.as_str();
                    let root = name.split('.').next().unwrap_or(name);
                    match import_bound(a) {
                        Bound::Module(_) => self.out.vetted_imports.push(root.to_owned()),
                        Bound::Plain => self.out.imports.push((name.to_owned(), a.range)),
                        _ => self.other(a.range),
                    }
                }
            }
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref().map_or("", |m| m.as_str());
                if i.level != 0 {
                    self.out.imports.push((module.to_owned(), i.range));
                } else if vetted_module(module).is_some()
                    || module == "java"
                    || module.starts_with("java.")
                    || module.starts_with("javax")
                {
                    if vetted_module(module).is_some() {
                        self.out
                            .vetted_imports
                            .push(module.split('.').next().unwrap_or(module).to_owned());
                    }
                    for a in &i.names {
                        if from_import_bound(i, a) == Bound::Unknown {
                            self.other(a.range);
                        }
                    }
                } else {
                    self.out.imports.push((module.to_owned(), i.range));
                }
            }
            Stmt::ClassDef(c) => {
                for base in &c.bases {
                    // Subclassing a Java class makes the instances
                    // Java-backed: their Java members, reflection included
                    // (`self.class`, `super(...)` reaching Java), are
                    // reachable, so the class is not plain unless analysed.
                    // Fail closed. (A dotted base such as
                    // `java.util.ArrayList` needs an import, already
                    // flagged.)
                    if let Expr::Name(n) = base
                        && self
                            .candidates(n)
                            .iter()
                            .any(|b| matches!(b, Bound::JavaClass(_)))
                    {
                        self.other(n.range);
                    }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(c) if self.system_write(&c.func) => {
                let paths = c.args.first().or_else(|| {
                    c.keywords
                        .iter()
                        .find(|k| matches!(k.arg.as_str(), "tagPaths" | "tagPath"))
                        .map(|k| &k.value)
                });
                let checked = c.starargs.is_none()
                    && c.kwargs.is_none()
                    && paths.is_some_and(value_write_paths);
                if !checked {
                    self.other(c.range);
                }
                self.chain_with(&c.func, checked);
                for a in &c.args {
                    self.visit_expr(a);
                }
                for k in &c.keywords {
                    self.visit_expr(&k.value);
                }
                return;
            }
            Expr::Attribute(_) => {
                if self.chain(expr) {
                    return;
                }
            }
            Expr::Name(n) if n.ctx == ExprContext::Load => {
                // A module object used as a value: what is read from it
                // later is not checked. A binding that is not vetted.
                if self
                    .candidates(n)
                    .iter()
                    .any(|b| matches!(b, Bound::Module(_) | Bound::Unknown))
                {
                    self.other(n.range);
                }
                return;
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown(src: &str) -> UnknownCode {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty(), "{src}");
        let model = SemanticModel::build(
            &parsed.module,
            &crate::SemanticOptions {
                injected: vec!["system", "fpmi", "sys", "__builtins__"],
                unknown_globals: false,
            },
        );
        unknown_code(&model)
    }

    #[test]
    fn lists_are_sorted() {
        assert!(VETTED_BUILTINS.windows(2).all(|w| w[0] < w[1]));
        for name in VETTED_BUILTINS {
            assert!(iglint_stubs::is_jython_builtin(name), "{name}");
        }
    }

    #[test]
    fn vetted_code_is_known() {
        for src in [
            "import math\nx = math.sqrt(2)\n",
            "import json\nd = json.loads(s)\n",
            "from datetime import datetime\nd = datetime.now()\n",
            "import os.path\np = os.path.join('a', 'b')\n",
            "import os\np = os.path.join(os.getcwd(), 'b')\n",
            "x = sys.maxint\n",
            "system.tag.writeBlocking(['a'], [1])\n",
            "from java.util import Date, ArrayList\nd = Date()\n",
            "from java.lang import System\nt = System.currentTimeMillis()\n",
            "from java.lang import Exception as JException\ntry:\n    pass\nexcept JException, e:\n    pass\n",
            "import string\nt = string.Template('$a')\n",
            "x = len([1]) + int('2')\ny = getattr(o, 'value')\n",
            "try:\n    pass\nexcept ValueError:\n    pass\n",
            "from __future__ import division\n",
            "import operator\nx = operator.itemgetter(1)\n",
            "import system.perspective\nsystem.perspective.print(1)\n",
        ] {
            assert!(unknown(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn everything_else_is_unknown() {
        for src in [
            "import cPickle\n",
            "import pickle as p\n",
            "from cPickle import loads\n",
            "import warnings\n",
            "import logging\n",
            "import traceback\n",
            "import code\n",
            "from org.python.core import Py\n",
            "from com.inductiveautomation.ignition.common import BasicDataset\n",
            "from java.lang import Thread\n",
            "from java.lang import Throwable\n",
            "from java.lang import Class\n",
            "from java.lang.reflect import Method\n",
            "from java.io import ObjectInputStream\n",
            "from java.util import ServiceLoader\n",
            "import java.util\n",
            "x = sys._getframe\n",
            "x = sys.modules\n",
            "x = sys.path\n",
            "import os\nx = os.system('ls')\n",
            "import re\nx = re.sre_compile\n",
            "import calendar\nx = calendar.datetime\n",
            "import os.path\nx = os.path.os\n",
            "import uuid\nx = uuid.ctypes\n",
            "import random\nx = random._inst\n",
            "import json\nm = json\n",
            "import string\nf = string.Formatter()\n",
            "from string import Formatter\n",
            "from json import decoder\n",
            "from java.lang import System\nSystem.loadLibrary('x')\n",
            "x = eval('1')\n",
            "x = reload(m)\n",
            "x = input()\n",
            "help('modules')\n",
            "x = vars(o)\n",
            "import operator\nf = operator.attrgetter(n)\n",
            "from operator import methodcaller\n",
            // reviewer round 7, finding 1: subclassing a Java class is not
            // plain, and the reviewer's reflection exploit uses only
            // allowlisted imports.
            "from java.util import ArrayList\nclass Bridge(ArrayList):\n    pass\n",
            "from java.util import ArrayList\nclass B(ArrayList):\n    pass\nl = B().class.classLoader\n",
            // reviewer H1: a vetted module hands out `sys` (a PySystemState
            // in Jython), which reaches `modules` and `__builtin__`.
            "import re\nx = re.sys.modules\n",
            "import calendar\nx = calendar.sys\n",
            "import os.path\nx = os.path.sys\n",
        ] {
            assert!(!unknown(src).is_empty(), "{src}");
        }
    }

    /// All orders of `stmts`, at module level and inside a function.
    fn arrangements(stmts: &[&str]) -> Vec<String> {
        fn permute(rest: Vec<&str>, done: Vec<&str>, out: &mut Vec<Vec<String>>) {
            if rest.is_empty() {
                out.push(done.iter().map(|s| (*s).to_owned()).collect());
                return;
            }
            for i in 0..rest.len() {
                let mut r = rest.clone();
                let s = r.remove(i);
                let mut d = done.clone();
                d.push(s);
                permute(r, d, out);
            }
        }
        let mut orders = Vec::new();
        permute(stmts.to_vec(), Vec::new(), &mut orders);
        let mut out = Vec::new();
        for order in orders {
            let module = order.join("\n") + "\n";
            let mut nested = String::from("def wrapper():\n");
            for line in module.lines() {
                nested.push_str("    ");
                nested.push_str(line);
                nested.push('\n');
            }
            nested.push_str("wrapper()\n");
            out.push(module);
            out.push(nested);
        }
        out
    }

    /// Reviewer round 8 (Codex) blocker: the verdict must not depend on the
    /// order in which the walk meets bindings. A function defined before
    /// `import re` reads the module `re` when it runs. Every permutation of
    /// the top-level statements, and the same code moved into a function,
    /// must stay unknown.
    #[test]
    fn verdict_does_not_depend_on_statement_order() {
        let exploits: &[&[&str]] = &[
            &[
                "def capture():\n    return re.sys.modules['__builtin__']",
                "import re",
                "def target():\n    import math\n    print capture().eval('math.pi')",
                "target()",
            ],
            &["def f():\n    return json", "import json"],
            &[
                "def f():\n    return calendar.sys",
                "import calendar",
                "f()",
            ],
            &["def f():\n    return r.sys", "import re as r"],
            &["def f():\n    return s.Formatter()", "import string as s"],
            &["def f():\n    return os.system('x')", "import os"],
            &[
                "def f():\n    S.loadLibrary('x')",
                "from java.lang import System as S",
            ],
            &[
                "def f():\n    class B(L):\n        pass\n    return B",
                "from java.util import ArrayList as L",
            ],
            &[
                "def h():\n    global re\n    import re",
                "def g():\n    return re.sys",
            ],
            &["x = re.sys", "import re", "re = 1"],
            &["class C:\n    y = re.sys\n    import re", "pass"],
            &[
                "def outer():\n    def inner():\n        return re.sys\n    import re\n    return inner",
                "pass",
            ],
            &[
                "def f():\n    return capture()",
                "capture = re.sys",
                "import re",
            ],
        ];
        for stmts in exploits {
            for src in arrangements(stmts) {
                assert!(!unknown(&src).is_empty(), "not unknown:\n{src}");
            }
        }
        // A module-level rebinding of Ignition's injected `sys` does not
        // hide it from code that may run first (inside a function, `sys`
        // would be a closure variable instead).
        for src in [
            "def f():\n    return sys.modules\nsys = 1\n",
            "sys = 1\ndef f():\n    return sys.modules\n",
        ] {
            assert!(!unknown(src).is_empty(), "{src}");
        }
        let plain: &[&[&str]] = &[
            &["def f():\n    return math.sqrt(2)", "import math", "f()"],
            &[
                "def f():\n    return D(1)",
                "from decimal import Decimal as D",
                "x = f()",
            ],
            &["def f():\n    re = 1\n    return re.sys", "import math"],
        ];
        for stmts in plain {
            for src in arrangements(stmts) {
                assert!(unknown(&src).is_empty(), "unknown:\n{src}");
            }
        }
    }

    /// The `system.*` audit: functions that store or run code from run-time
    /// values are unknown code; `system.tag` writes only with value paths.
    #[test]
    fn system_functions_that_run_code() {
        for src in [
            "system.tag.configure('[default]', [{'name': 'x'}], 'o')\n",
            "system.tag.importTags(path, '[default]')\n",
            "system.tag.editTag('[default]T', attributes={})\n",
            "system.tag.copy(['[other]T'], '[default]')\n",
            "system.util.execute(['ls'])\n",
            "system.project.requestScan()\n",
            "system.eam.runTask('t')\n",
            "c = system.util.WRAPPED_CONTEXT.get()\n",
            "f = system.tag.configure\n",
            "t = system.tag\nt.configure('[default]', [], 'o')\n",
            "w = system.tag.writeBlocking\n",
            "system.tag.writeBlocking(['[default]T.expression'], ['runScript(\"x\")'])\n",
            "system.tag.writeBlocking([p], [v])\n",
            "system.tag.writeBlocking(paths, values)\n",
            "system.tag.writeBlocking([base + 'n'], [v])\n",
            "system.tag.writeBlocking([base + '/T.eventScripts'], [v])\n",
            "system.tag.writeBlocking(['%s.expression' % base], [v])\n",
            "system.tag.writeAsync([b + '/' + n], [v])\n",
            "system.tag.writeBlocking(*args)\n",
            "system.tag.write('{}'.format(p), v)\n",
            "def f():\n    return system.tag.writeBlocking([p], [1])\n",
        ] {
            assert!(!unknown(src).is_empty(), "{src}");
        }
        for src in [
            "system.tag.writeBlocking(['[default]Folder/Tag'], [1])\n",
            "system.tag.writeBlocking(['[default]T.Value', '[default]U.quality'], [1, 2])\n",
            "system.tag.writeBlocking([base + '/Speed'], [1])\n",
            "system.tag.writeAsync(('%s/Speed' % base,), [1])\n",
            "system.tag.writeBlocking(['{}/Speed'.format(base)], [1])\n",
            "system.tag.writeBlocking(tagPaths=[base + '/Speed'], values=[1])\n",
            "system.tag.readBlocking([p])\n",
            "system.util.sendMessage('proj', handler, payload)\n",
            "system.db.runQuery(sql)\n",
        ] {
            assert!(unknown(src).is_empty(), "{src}");
        }
    }

    #[test]
    fn library_imports_are_left_to_the_caller() {
        let u = unknown("import core.util\nfrom shared.db import run\nfrom . import x\n");
        assert_eq!(u.other, None);
        let names: Vec<&str> = u.imports.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["core.util", "shared.db", ""]);
    }

    /// The generated tables (`src/generated/vetted_generated.rs`) were
    /// produced from these exact vetted lists. If the lists change, the
    /// generator must be rerun (`stubs-gen/run.sh --vetted`): this test
    /// fails until `GENERATED_FROM` matches again, so the machine-measured
    /// facts can never silently drift from the vetted inputs.
    #[test]
    fn generated_tables_match_vetted_inputs() {
        let mut mods: Vec<String> = VETTED_MODULES
            .iter()
            .map(|(m, _)| (*m).to_owned())
            .collect();
        mods.push("os.path".to_owned());
        mods.sort();
        mods.dedup();
        let mut sig: Vec<String> = mods.iter().map(|m| format!("mod:{m}")).collect();
        let mut classes: Vec<String> = Vec::new();
        for (pkg, cs) in VETTED_JAVA_CLASSES {
            for c in *cs {
                classes.push(format!("{pkg}.{c}"));
            }
        }
        classes.sort();
        sig.extend(classes.iter().map(|c| format!("java:{c}")));
        let mut packages: Vec<String> = VETTED_JAVA_PACKAGES
            .iter()
            .map(|(p, _)| format!("pkg:{p}"))
            .collect();
        packages.sort();
        sig.extend(packages);
        // The Java closure is seeded with the types `system.*` returns,
        // from the stubs: regenerate when they change.
        sig.push(format!("stubs:{}", iglint_stubs::IGNITION_BUILD));
        let generated: Vec<&str> = crate::generated::GENERATED_FROM.to_vec();
        assert_eq!(
            sig, generated,
            "vetted lists changed; rerun `stubs-gen/run.sh --vetted` to regenerate \
             src/generated/vetted_generated.rs"
        );
    }

    /// The generated name and attribute lists are sorted for the binary
    /// searches that consume them.
    #[test]
    fn generated_lists_are_sorted() {
        assert!(
            crate::generated::JAVA_REFLECTIVE_MEMBERS
                .windows(2)
                .all(|w| w[0] < w[1])
        );
        for (_, attrs) in crate::generated::MODULE_VETTED_ATTRS {
            assert!(attrs.windows(2).all(|w| w[0] < w[1]));
        }
    }
}
