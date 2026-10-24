//! Ways code can observe a running frame's namespace, a module's
//! namespace or the identity of a function other than by naming
//! variables.
//!
//! # Role
//!
//! These are the routes that the analyses of iglint *understand*. Each
//! route is one of two kinds ([`Route`]): one that reads or writes a
//! frame's or a module's names by itself ([`Route::Observes`],
//! [`frame_observer`]) makes every scope of the module read its locals and
//! write its globals, which silences findings; any route
//! ([`frame_escape`]), a first hop into reflection included (a measured
//! reflective Java member such as `.type`, a computed `getattr`, a line
//! number), makes the safe fixes that depend on what code reads unsafe
//! (fixes **fail closed** on every route, findings only on the routes
//! that make them likely false). The routes are not the safety argument
//! for fixes on their own: an enumerated list of
//! routes was defeated in six review rounds in a row. Safe fixes that
//! depend on what code can read also require the code to be **vetted**
//! ([`crate::vetted`]: default deny over modules, Java classes, builtins
//! and module attributes), and the engine's post-fix net checks that safe
//! fixes keep every line, compile status, docstring and name resolution.
//!
//! # Routes
//!
//! 1. **Naming an introspection attribute in source.** Frame, traceback,
//!    code, generator, function and class objects expose namespaces only
//!    through named attributes ([`INTROSPECTION_ATTRS`]: `f_locals`,
//!    `f_globals`, `f_back`, `tb_frame`, `gi_frame`, `func_globals`,
//!    `__globals__`, `co_*`, `__dict__`, `__subclasses__`, `__mro__`, ...)
//!    and Java reflection methods ([`FRAME_METHOD_NAMES`]: `getClass`,
//!    `forName`, `loadClass`, `getContextClassLoader`, ...). The name can
//!    be an attribute access or any string literal, including a dotted or
//!    sub-segment of one (`getattr(tb, 'tb_frame')`,
//!    `attrgetter('tb_frame.f_back.f_locals')`, `'{0.gi_frame}'.format`).
//!    String literals are scanned in sink positions only (see "String
//!    sinks"), matching whole identifiers so that ordinary prose does not
//!    trigger.
//! 2. **A reflective accessor with a computed name or used as a value.**
//!    `getattr`, `setattr`, `delattr`, `hasattr`, `attrgetter`,
//!    `methodcaller`, `__getattribute__`, and Jython's `__findattr__` and
//!    `invoke` ([`REFLECTIVE_ACCESSORS`]) are
//!    understood only as direct calls with a literal name (and the
//!    literal `class`, Jython's bean property for `getClass()`, is a
//!    route). Any other use (`get = getattr`, `map(getattr, ...)`,
//!    `f = operator.attrgetter`) is a route: the attribute read is
//!    unknown. So is `str.format` on a format string that is not a literal
//!    (`fmt.format(g)`): format strings name attributes.
//! 3. **An interpreter or reflection API.** `eval`, `exec`, `execfile`,
//!    `compile`, `locals`, `globals`, `vars`, `dir`, `input`,
//!    `__import__`; the builtins module and the module-handing modules
//!    (`__builtin__`, `importlib`, `imp`); the frame-, module- and
//!    trace-reaching functions of `sys`, `inspect` and `gc`
//!    ([`PARTLY_FRAME_MODULES`]); `sys.exc_info` and the traceback
//!    module; and Java / Jython reflection ([`is_reflection_import`]).
//!
//! A frame builtin used under another name reads the frame of whoever
//! calls it, also across modules (`ev = eval` in one module, `m.ev('x')`
//! in another reads the caller's `x`): verified on Jython 2.7. So a bare
//! frame builtin used as a value, or handed out of a module, is a route
//! for every scope.
//!
//! # String sinks
//!
//! Where a string literal can name a member at run time, verified on
//! Jython (the literal is scanned for introspection names only there):
//!
//! * the name arguments of a direct reflective-accessor call
//!   ([`REFLECTIVE_ACCESSORS`]: `getattr`, `hasattr`, `setattr`, `delattr`,
//!   `__getattribute__`, `__getattr__`, `__findattr__`, `__findattr_ex__`,
//!   `invoke`, `operator.attrgetter` (every argument, dotted names
//!   included), `operator.methodcaller`);
//! * the receiver of `format` / `vformat` (`'{0.f_back}'.format(g)`, also
//!   as a value, `str` and `unicode` alike);
//! * code: the literal argument of `eval`, `execfile`, `input` and the
//!   body of an `exec` statement.
//!
//! Everything else that could look a member up from a string is an escape
//! whatever its argument: a sink with a computed name or format string, or
//! used as a value (`get = getattr`, `f = fmt.format`, `str.format(fmt,
//! x)`, `object.__getattribute__(o, n)`), `string.Formatter` (its receiver
//! is not a literal), `vars(o)` (a member dictionary), `__dict__` (named),
//! `compile` and `__import__`. Not sinks (verified): `%` formatting and
//! `string.Template` look keys up with `__getitem__` (`'%(x)s' % o` needs
//! a mapping), and `format(o, spec)` takes a format spec, not field names.
//! A literal that reaches a sink through a variable, a concatenation,
//! `join`, `%`, slicing, a container or a function return is not a
//! literal there, so the sink is computed and an escape.
//!
//! # Line numbers
//!
//! Line numbers are not something these routes protect: safe fixes never
//! move a line (the engine rejects any that would), so whatever reads them
//! (tracebacks, `warnings`, `logging`, Java stack traces) sees the same
//! numbers after a safe fix. Routes that only expose line numbers are kept
//! because they also reach frames.

use iglint_python::ast::{Expr, ExprContext, ExprName, Stmt, StringValue};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

/// Builtins that read or write the namespaces of the frame that calls
/// them, or that build or import code. `compile` and `__import__` are
/// escapes on any use; the rest read the caller's frame and are handled
/// precisely by the scope that calls them directly.
pub const FRAME_BUILTINS: &[&str] = &[
    "eval",
    "execfile",
    "locals",
    "globals",
    "vars",
    "dir",
    "input",
    "__import__",
    "compile",
];

/// Reflective accessors: understood only as direct calls with a literal
/// name (route 2); a string-literal name is handled by the string-literal
/// scan of route 1, any other use is a route.
pub const REFLECTIVE_ACCESSORS: &[&str] = &[
    "getattr",
    "setattr",
    "delattr",
    "hasattr",
    "attrgetter",
    "methodcaller",
    "__getattribute__",
    "__getattr__",
    "__setattr__",
    "__delattr__",
    // Jython's `PyObject` lookups by name, visible as Java members on
    // values whose class does not declare `@ExposedType`: `__findattr__`
    // and `__findattr_ex__` read an attribute, `invoke` calls a method.
    "__findattr__",
    "__findattr_ex__",
    "invoke",
];

/// String methods that read the attributes and items their format string
/// names (`'{0.f_back}'.format(frame)`).
const FORMAT_METHODS: &[&str] = &["format", "vformat"];

/// Modules whose every use hands out other modules or the builtins, or
/// renders tracebacks (line numbers) into values.
const FRAME_MODULES: &[&str] = &["__builtin__", "importlib", "imp", "traceback"];

/// Modules with some functions that reach frames, every module, trace
/// hooks or tracebacks: the names that do. Any other use of the module
/// object (as a value, `getattr(sys, name)`) counts too.
const PARTLY_FRAME_MODULES: &[(&str, &[&str])] = &[
    (
        "sys",
        &[
            "_getframe",
            "_current_frames",
            "modules",
            "settrace",
            "setprofile",
            "gettrace",
            "getprofile",
            "exc_info",
            "exc_traceback",
            "last_traceback",
            // Jython's `PySystemState` hands out the builtins dictionary
            // and the modules being reloaded (measured on 8.3.3:
            // `sys.builtins['locals']()` and `sys.getDefaultBuiltins()`
            // read the caller's frame; review round 25, M2).
            "builtins",
            "getBuiltins",
            "getBuiltin",
            "getDefaultBuiltins",
            "setBuiltins",
            "modules_reloading",
        ],
    ),
    // Module loaders: `importlib.import_module` hands out any module, the
    // builtins too (`importlib.import_module('__builtin__').locals()` reads
    // the caller's frame, measured on 8.3.3; review round 25, M2); a
    // direct call with a literal name is judged like `__import__`
    // ([`import_call_route`]). `imp` runs and hands out the code of any
    // file or package.
    ("importlib", &["import_module"]),
    (
        "imp",
        &[
            "load_module",
            "load_source",
            "load_compiled",
            "load_dynamic",
            "load_package",
        ],
    ),
    (
        "inspect",
        &[
            "currentframe",
            "stack",
            "trace",
            "getouterframes",
            "getinnerframes",
            "getframeinfo",
            "getargvalues",
            "formatargvalues",
            "getmembers",
            "getlineno",
        ],
    ),
    ("gc", &["get_referrers", "get_referents", "get_objects"]),
];

/// The frame-reaching names of a module of [`PARTLY_FRAME_MODULES`].
fn module_frame_names(module: &str) -> Option<&'static [&'static str]> {
    PARTLY_FRAME_MODULES
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, names)| *names)
}

/// Attributes of frames, tracebacks, generators, functions, methods and
/// code objects that reach another frame or a namespace, or a source line
/// number. Matched as attribute accesses and (route 1) as identifier or
/// dotted segments of any string literal. The `co_*` set is every code
/// object attribute of Jython 2.7.
pub const INTROSPECTION_ATTRS: &[&str] = &[
    // frame
    "f_back",
    "f_locals",
    "f_globals",
    "f_builtins",
    "f_lineno",
    "f_code",
    "f_trace",
    "f_lasti",
    "f_exc_type",
    "f_exc_value",
    "f_exc_traceback",
    "f_restricted",
    // traceback
    "tb_frame",
    "tb_next",
    "tb_lineno",
    "tb_lasti",
    // generator
    "gi_frame",
    "gi_code",
    "gi_running",
    // function
    "func_globals",
    "func_code",
    "func_closure",
    "func_defaults",
    "func_dict",
    "__globals__",
    "__code__",
    "__closure__",
    "__defaults__",
    // method
    "im_func",
    "im_self",
    "im_class",
    "__func__",
    "__self__",
    // code object (every co_* of Jython 2.7)
    "co_argcount",
    "co_cellvars",
    "co_code",
    "co_consts",
    "co_filename",
    "co_firstlineno",
    "co_flags",
    "co_freevars",
    "co_lnotab",
    "co_name",
    "co_names",
    "co_nlocals",
    "co_stacksize",
    "co_varnames",
    // generic introspection; `__reduce__` and `__reduce_ex__` hand out
    // the object's `__dict__` as its state (measured on 8.3.3:
    // `m.__reduce_ex__(2)[2]` is a library module's namespace)
    "__dict__",
    "__getattribute__",
    "__builtins__",
    "__reduce__",
    "__reduce_ex__",
    // classes: every class of the interpreter, and the classes a class
    // derives from (each leads to functions and their globals)
    "__subclasses__",
    "__mro__",
    "__bases__",
    "__base__",
    "mro",
];

/// Module functions that reach frames or tracebacks, matched as attribute
/// accesses and in string literals (`getattr(inspect, 'currentframe')`).
/// Kept separate from [`INTROSPECTION_ATTRS`] because these are method
/// names rather than data attributes.
const FRAME_METHOD_NAMES: &[&str] = &[
    "_getframe",
    "_current_frames",
    "currentframe",
    "getouterframes",
    "getinnerframes",
    "getframeinfo",
    "settrace",
    "setprofile",
    // Java / Jython reflection entry points
    "getClass",
    "forName",
    "getDeclaredField",
    "getDeclaredFields",
    "getDeclaredMethod",
    "getDeclaredMethods",
    "getDeclaredConstructor",
    "getDeclaredConstructors",
    "getMethod",
    "getMethods",
    "getField",
    "getFields",
    "getConstructor",
    "getConstructors",
    "getClassLoader",
    "getContextClassLoader",
    "getSystemClassLoader",
    "loadClass",
    "defineClass",
];

/// Members of Ignition's project library module and package objects
/// (`ProjectScriptModule`, `ScriptPackage`, measured on 8.3.3 with
/// `oracle/jython_library_routes_oracle.py`) that hand out the object's
/// namespace, the `__builtins__` of a module included (`m.dict`,
/// `m.getDict()`: `m.dict['__builtins__']['locals']()` reads the caller's
/// frame), or replace its code with code from data (`m.setCode(s)`).
/// Matched on any value, like [`INTROSPECTION_ATTRS`]: an alias or a
/// parameter may hold a library module (review round 26).
pub const LIBRARY_OBJECT_ROUTES: &[&str] = &["dict", "getDict", "setCode"];

/// Attributes that hand out the namespace of the object they are read
/// from: on a value that may be a module (a module's namespace holds its
/// `__builtins__`), [`Route::Observes`] (review round 26, M2:
/// `vars(json)['__builtins__']['locals']()` reads the caller's frame).
const NAMESPACE_ATTRS: &[&str] = &["__dict__", "dict", "getDict", "__reduce__", "__reduce_ex__"];

/// Whether `name`, a whole identifier, is an introspection attribute, a
/// frame-reaching method name, or a Java member whose declared type
/// mentions a reflection or interpreter type, on any type a script can
/// hold (the machine-measured
/// [`crate::generated::JAVA_REFLECTIVE_MEMBERS`]: `class`, `classLoader`,
/// `methods`, `module`, `argslist`, `getScriptManager`, ...). Naming any
/// of these, on a value or in a string literal, is the first hop out of
/// plain data into Java reflection or the interpreter, from which a script
/// reaches `org.python.core.Py`, the frame and `eval`; we fail closed.
fn is_introspection_name(name: &str) -> bool {
    INTROSPECTION_ATTRS.contains(&name)
        || FRAME_METHOD_NAMES.contains(&name)
        || LIBRARY_OBJECT_ROUTES.contains(&name)
        || crate::generated::JAVA_REFLECTIVE_MEMBERS
            .binary_search(&name)
            .is_ok()
}

/// Introspection names that reach a frame's namespace, a function's or
/// frame's globals, the builtins or a trace hook (which receives frames)
/// by themselves: [`Route::Observes`]. Every other introspection name
/// ([`is_introspection_name`]: line numbers, code objects, class
/// hierarchies, method internals, Java reflection, the measured
/// [`crate::generated::JAVA_REFLECTIVE_MEMBERS`]) is [`Route::FixOnly`]:
/// a first hop that only further routes turn into a read of names.
const OBSERVING_NAMES: &[&str] = &[
    "f_back",
    "f_locals",
    "f_globals",
    "f_builtins",
    "f_trace",
    "tb_frame",
    "gi_frame",
    "func_globals",
    "__globals__",
    "__builtins__",
    "_getframe",
    "_current_frames",
    "currentframe",
    "getouterframes",
    "getinnerframes",
    "getframeinfo",
    "settrace",
    "setprofile",
];

/// The route an introspection name takes, if it is one.
fn introspection_route(name: &str) -> Option<Route> {
    if OBSERVING_NAMES.contains(&name) {
        Some(Route::Observes)
    } else if is_introspection_name(name) {
        Some(Route::FixOnly)
    } else {
        None
    }
}

/// Whether a string literal names an introspection attribute in one of its
/// identifier / dotted segments (`'tb_frame'`,
/// `'tb_frame.f_back.f_locals'`). Splits on every character that cannot be
/// part of a Python identifier, so whole identifiers are matched and
/// ordinary prose (`"the frame"`) is not.
///
/// The literal is decoded as Latin-1 ([`StringValue::to_latin1`]), not as
/// UTF-8: a byte string that is not valid UTF-8 (`'{0.f_locals}\xff'`)
/// must not be read as empty, which would miss the name and leave a fix
/// unsafely applied. Non-ASCII bytes are not identifier characters, so
/// they only ever act as separators here.
fn string_names_introspection(value: &StringValue) -> Option<Route> {
    value
        .to_latin1()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter_map(introspection_route)
        .max()
}

/// Whether an import of `module` is a Java or Jython reflection API:
/// anything under `org.python.*` (`Py`, `PyFrame`, `ThreadState`,
/// `PySystemState`, `__builtin__`), `java.lang.reflect`, `java.lang.Class`,
/// `java.lang.invoke`, `javax.script`.
fn is_reflection_import(module: &str) -> bool {
    module == "org.python"
        || module.starts_with("org.python.")
        || module == "java.lang.reflect"
        || module.starts_with("java.lang.reflect.")
        || module == "java.lang.invoke"
        || module.starts_with("java.lang.invoke.")
        || module == "javax.script"
        || module.starts_with("javax.script.")
}

/// Whether a `from java.lang import X` names a reflection class.
fn is_reflection_from_java_lang(module: &str, imported: &str) -> bool {
    module == "java.lang" && matches!(imported, "Class" | "ClassLoader")
}

/// The route an import of `module` takes ([`FRAME_MODULES`] and
/// [`is_reflection_import`]), and for `from module import name`, of
/// `name`. The builtins module hands out `eval` and `locals`, and the
/// interpreter's own classes (`org.python.core.Py`, `PyFrame`,
/// `ThreadState`, `PySystemState`, a package of them as a value) hand out
/// the current frame: they observe. Module loaders, the traceback module
/// (text and line numbers), Java reflection, `javax.script` (an engine
/// with its own namespace) and Jython's value classes (`PyString`,
/// `PyList`) reach frames only through further routes: fix only.
fn import_route(module: &str, name: Option<&str>) -> Route {
    let root = module.split('.').next().unwrap_or("");
    if root == "__builtin__" {
        return Route::Observes;
    }
    if module == "org.python" || module.starts_with("org.python.") {
        return match name {
            Some(n) if !OBSERVING_PYTHON_CLASSES.contains(&n) => Route::FixOnly,
            _ => Route::Observes,
        };
    }
    Route::FixOnly
}

/// Classes of `org.python.*` that hand out the running frame, the module
/// table or the builtins (`Py.getFrame()`, `ThreadState.frame`,
/// `PySystemState.modules`, `__builtin__.locals`), and the star import.
const OBSERVING_PYTHON_CLASSES: &[&str] = &[
    "*",
    "Py",
    "PyFrame",
    "ThreadState",
    "PySystemState",
    "PyTraceback",
    "__builtin__",
    "imp",
    "core",
];

/// The route of a name imported from a module of
/// [`PARTLY_FRAME_MODULES`] (`from sys import _getframe`), or of the
/// attribute read from it (`sys._getframe`). Frame, module table and
/// trace hook functions observe; the exception state (`sys.exc_info`,
/// whose traceback reaches a frame only through `tb_frame`, itself a
/// route), member listings and line numbers are fix only.
fn module_member_route(module: &str, name: &str) -> Route {
    match (module, name) {
        ("sys", "exc_info" | "exc_traceback" | "last_traceback")
        | ("inspect", "getmembers" | "getlineno") => Route::FixOnly,
        _ => Route::Observes,
    }
}

/// How far an escape route reaches, for the two uses of the routes.
///
/// Fixes trust no route: any route makes the safe fixes that depend on
/// what code can read unsafe ([`frame_escape`]). Findings are silenced
/// only by a route that reads or writes a frame's or a module's names by
/// itself, so that names the code does not visibly use may be used
/// ([`frame_observer`]): a finding there would likely be false. A route
/// that is only a first hop (a reflective Java member such as `.type`, a
/// computed `getattr`, a line number) keeps the findings and makes the
/// fixes unsafe. The table is in `docs/internal/fix-safety.md`, "Frame
/// routes: findings and fixes".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Route {
    /// A first hop into reflection or the interpreter: fixes are unsafe,
    /// findings stay.
    FixOnly,
    /// Reads or writes a frame's or module's names: the module's scopes
    /// are treated as reading their locals and writing their globals.
    Observes,
}

/// The first escape route of `body` (nested scopes included) of either
/// kind, if any: what makes fixes unsafe.
pub fn frame_escape(body: &[Stmt]) -> Option<TextRange> {
    escapes(body).0
}

/// The first escape route of `body` that observes names
/// ([`Route::Observes`]), if any: what silences findings.
pub fn frame_observer(body: &[Stmt]) -> Option<TextRange> {
    escapes(body).1
}

/// [`frame_escape`] and [`frame_observer`] in one walk.
pub fn escapes(body: &[Stmt]) -> (Option<TextRange>, Option<TextRange>) {
    escapes_with(body, &|_| false)
}

/// [`escapes`], where `may_be_module` says which names may stand for a
/// module object (the caller resolves them: a name bound by an import or
/// bound nowhere, such as a library package). The namespace of such a
/// value (`vars(m)`, `m.__dict__`, `m.dict`, `getattr(m, n)` with a
/// computed name, [`NAMESPACE_ATTRS`]) holds the module's `__builtins__`:
/// [`Route::Observes`] instead of [`Route::FixOnly`]. Which routes there
/// are does not depend on it, only how far they reach.
pub fn escapes_with(
    body: &[Stmt],
    may_be_module: &dyn Fn(&ExprName) -> bool,
) -> (Option<TextRange>, Option<TextRange>) {
    let mut v = Escapes {
        modules: module_names(body),
        may_be_module,
        found: None,
        observed: None,
    };
    for stmt in body {
        v.visit_stmt(stmt);
    }
    (v.found, v.observed)
}

/// The first place where `body` (nested scopes included) reads frames
/// dynamically: an escape route ([`frame_escape`]), a direct call of a
/// frame builtin (`locals()`, `eval(s)`, `vars()`, `dir()`) or an `exec`
/// statement.
pub fn reads_frames(body: &[Stmt]) -> Option<TextRange> {
    if let Some(r) = frame_escape(body) {
        return Some(r);
    }
    struct Calls(Option<TextRange>);
    impl<'a> Visitor<'a> for Calls {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if let Stmt::Exec(e) = stmt
                && self.0.is_none()
            {
                self.0 = Some(e.range);
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(c) = expr
                && let Expr::Name(n) = &*c.func
                && self.0.is_none()
            {
                let no_args = c.args.is_empty() && c.keywords.is_empty();
                let reads = match &*n.id {
                    "vars" | "dir" => no_args,
                    "__import__" | "compile" => false,
                    name => FRAME_BUILTINS.contains(&name),
                };
                if reads {
                    self.0 = Some(c.range);
                }
            }
            walk_expr(self, expr);
        }
    }
    let mut v = Calls(None);
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.0
}

/// The route of a direct `__import__(...)` or
/// `importlib.import_module(...)` call: it hands out any module, the
/// builtins too. A literal name of a module that hands out frames, the
/// builtins or the interpreter (or a name that is not a literal, or a
/// relative one, which a package argument resolves) observes; a literal
/// name of another module is fix only (whatever it hands out is used
/// through further routes).
fn import_call_route(c: &iglint_python::ast::ExprCall) -> Route {
    if c.starargs.is_some() || c.kwargs.is_some() {
        return Route::Observes;
    }
    match c.args.first() {
        Some(Expr::Str(s)) => {
            let name = s.s.to_latin1();
            let root = name.split('.').next().unwrap_or("").to_owned();
            if root.is_empty()
                || FRAME_MODULES.contains(&root.as_str())
                || module_frame_names(&root).is_some()
                || is_reflection_import(&name)
            {
                Route::Observes
            } else {
                Route::FixOnly
            }
        }
        _ => Route::Observes,
    }
}

/// Names bound to a module of [`PARTLY_FRAME_MODULES`] (`sys`, which
/// Ignition injects, and `import inspect as i`), with the module.
fn module_names(body: &[Stmt]) -> FxHashMap<String, &'static str> {
    struct Imports(FxHashMap<String, &'static str>);
    impl<'a> Visitor<'a> for Imports {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if let Stmt::Import(i) = stmt {
                for a in &i.names {
                    if let Some(&(module, _)) =
                        PARTLY_FRAME_MODULES.iter().find(|(m, _)| *m == &*a.name.id)
                    {
                        let name: &str = a.asname.as_ref().map_or(module, |n| &n.id);
                        self.0.insert(name.to_owned(), module);
                    }
                }
            }
            walk_stmt(self, stmt);
        }
    }
    let mut v = Imports(FxHashMap::default());
    v.0.insert("sys".to_owned(), "sys");
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.0
}

struct Escapes<'p> {
    /// Names bound to modules of [`PARTLY_FRAME_MODULES`].
    modules: FxHashMap<String, &'static str>,
    /// Whether a name may stand for a module object ([`escapes_with`]).
    may_be_module: &'p dyn Fn(&ExprName) -> bool,
    /// The first route of either kind.
    found: Option<TextRange>,
    /// The first [`Route::Observes`] route.
    observed: Option<TextRange>,
}

impl Escapes<'_> {
    fn found(&mut self, range: TextRange, route: Route) {
        if self.found.is_none_or(|f| range.start() < f.start()) {
            self.found = Some(range);
        }
        if route == Route::Observes && self.observed.is_none_or(|f| range.start() < f.start()) {
            self.observed = Some(range);
        }
    }

    /// Whether `e` may be a module object: an attribute chain on a name
    /// that may stand for one ([`escapes_with`]).
    fn module_value(&self, e: &Expr) -> bool {
        let mut root = e;
        while let Expr::Attribute(a) = root {
            root = &a.value;
        }
        matches!(root, Expr::Name(n) if (self.may_be_module)(n))
    }

    /// The object whose attribute a direct reflective-accessor call reads
    /// (`getattr(o, ...)`, `o.__getattribute__(...)`), if it names one.
    fn accessor_object<'e>(name: &str, c: &'e iglint_python::ast::ExprCall) -> Option<&'e Expr> {
        match name {
            "getattr" | "setattr" | "delattr" | "hasattr" => c.args.first(),
            "attrgetter" | "methodcaller" | "invoke" => None,
            _ => match &*c.func {
                Expr::Attribute(a) => Some(&a.value),
                _ => None,
            },
        }
    }

    /// The module of [`PARTLY_FRAME_MODULES`] that `e` names.
    fn module(&self, e: &Expr) -> Option<&'static str> {
        match e {
            Expr::Name(n) => self.modules.get(&*n.id).copied(),
            _ => None,
        }
    }

    /// The name of a reflective accessor ([`REFLECTIVE_ACCESSORS`]) that
    /// `func` calls, by a bare name or as an attribute (`operator.attrgetter`,
    /// `x.__getattribute__`).
    fn reflective_accessor(func: &Expr) -> Option<&str> {
        let name = match func {
            Expr::Name(n) => &*n.id,
            Expr::Attribute(a) => a.attr.as_str(),
            _ => return None,
        };
        REFLECTIVE_ACCESSORS.contains(&name).then_some(name)
    }

    /// Whether a reflective-accessor call reads an attribute whose name is
    /// not a literal (route 2). The name argument is read positionally.
    fn reflective_escape(c: &iglint_python::ast::ExprCall) -> bool {
        let Some(name) = Self::reflective_accessor(&c.func) else {
            return false;
        };
        // Unpacked arguments: the name could be anything.
        if c.starargs.is_some() || c.kwargs.is_some() {
            return true;
        }
        // A literal name is scanned like every string literal, except
        // `class`: Jython exposes `getClass()` as the bean property
        // `class`, which only a reflective accessor can name.
        let is_literal = |e: &Expr| match e {
            Expr::Str(s) => s.s.to_latin1() != "class",
            _ => false,
        };
        match name {
            // `attrgetter('a', 'b')`: every argument is an attribute name.
            "attrgetter" => !c.args.iter().all(is_literal),
            // The name is the first argument.
            "methodcaller" | "__getattribute__" | "__getattr__" | "__setattr__" | "__delattr__"
            | "__findattr__" | "__findattr_ex__" | "invoke" => {
                c.args.first().is_none_or(|a| !is_literal(a))
            }
            // `getattr(o, name)`, `setattr(o, name, v)`: the second.
            _ => c.args.get(1).is_none_or(|a| !is_literal(a)),
        }
    }
}

impl Escapes<'_> {
    /// A string literal in a sink position (see "String sinks" in the
    /// module documentation): it names members, so it is scanned for
    /// introspection names.
    fn sink(&mut self, e: &Expr) {
        if let Expr::Str(s) = e
            && let Some(route) = string_names_introspection(&s.s)
        {
            self.found(s.range, route);
        }
    }

    /// The name arguments of a direct reflective-accessor call.
    fn accessor_names<'e>(name: &str, c: &'e iglint_python::ast::ExprCall) -> Vec<&'e Expr> {
        match name {
            "attrgetter" => c.args.iter().collect(),
            "methodcaller" | "__getattribute__" | "__getattr__" | "__setattr__" | "__delattr__"
            | "__findattr__" | "__findattr_ex__" | "invoke" => c.args.first().into_iter().collect(),
            _ => c.args.get(1).into_iter().collect(),
        }
    }
}

impl<'a> Visitor<'a> for Escapes<'_> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            // Code in a literal names whatever it reads.
            Stmt::Exec(e) => self.sink(&e.body),
            Stmt::Import(i) => {
                for a in &i.names {
                    let root = a.name.id.split('.').next().unwrap_or("");
                    if FRAME_MODULES.contains(&root) || is_reflection_import(&a.name.id) {
                        self.found(a.range, import_route(&a.name.id, None));
                    }
                }
            }
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref().map_or("", |m| &*m.id);
                let root = module.split('.').next().unwrap_or("");
                if i.level == 0 && (FRAME_MODULES.contains(&root) || is_reflection_import(module)) {
                    // The statement is the site; the route is the strongest
                    // of its names.
                    let route = i
                        .names
                        .iter()
                        .map(|a| import_route(module, Some(&a.name.id)))
                        .max()
                        .unwrap_or(Route::Observes);
                    self.found(i.range, route);
                }
                if i.level == 0 {
                    if let Some(names) = module_frame_names(module) {
                        for a in &i.names {
                            if &*a.name.id == "*" {
                                self.found(a.range, Route::Observes);
                            } else if names.contains(&&*a.name.id) {
                                self.found(a.range, module_member_route(module, &a.name.id));
                            }
                        }
                    }
                    for a in &i.names {
                        if is_reflection_from_java_lang(module, &a.name.id) {
                            self.found(a.range, Route::FixOnly);
                        }
                    }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(n) => {
                // Direct calls and attribute reads are taken apart below; a
                // bare name reaching here is a value. A frame builtin under
                // another name reads the frame of whoever calls it; the
                // builtins module and a frame-reaching module as values
                // hand out such builtins and functions; a reflective
                // accessor under another name hides every call of it
                // (`get = getattr`, then `get(g, 'gi_' + 'frame')`).
                let load = n.ctx == ExprContext::Load;
                if matches!(&*n.id, "__builtin__" | "__builtins__")
                    || (load && FRAME_BUILTINS.contains(&&*n.id))
                    || (load && self.modules.contains_key(&*n.id))
                    || (load && REFLECTIVE_ACCESSORS.contains(&&*n.id))
                {
                    self.found(n.range, Route::Observes);
                }
                return;
            }
            // A string literal names a member only in a sink position
            // (scanned there); anywhere else it is data. See
            // "String sinks" in the module documentation.
            Expr::Str(_) => return,
            Expr::Call(c) => {
                // `importlib.import_module(name)`, called directly: judged
                // like `__import__` (any other use of it observes).
                if let Expr::Attribute(a) = &*c.func
                    && a.attr.as_str() == "import_module"
                    && self.module(&a.value) == Some("importlib")
                {
                    self.found(c.range, import_call_route(c));
                    for a in &c.args {
                        self.visit_expr(a);
                    }
                    for k in &c.keywords {
                        self.visit_expr(&k.value);
                    }
                    return;
                }
                // A computed name (or Jython's `class` bean property): the
                // attribute read is not known, a first hop. A literal name
                // of the namespace of a module object
                // (`getattr(m, '__dict__')`) hands out its `__builtins__`,
                // like `m.__dict__` (review round 26).
                if Escapes::reflective_escape(c) {
                    self.found(c.range, Route::FixOnly);
                }
                if let Some(name) = Escapes::reflective_accessor(&c.func)
                    && Escapes::accessor_object(name, c).is_some_and(|o| self.module_value(o))
                    && Escapes::accessor_names(name, c).iter().any(|a| {
                        matches!(a, Expr::Str(s) if NAMESPACE_ATTRS.contains(&&*s.s.to_latin1()))
                    })
                {
                    self.found(c.range, Route::Observes);
                }
                let format = match &*c.func {
                    Expr::Attribute(a) if FORMAT_METHODS.contains(&a.attr.as_str()) => Some(a),
                    _ => None,
                };
                if let Some(name) = Escapes::reflective_accessor(&c.func) {
                    for arg in Escapes::accessor_names(name, c) {
                        self.sink(arg);
                    }
                }
                if let Some(a) = format {
                    self.sink(&a.value);
                }
                if Escapes::reflective_accessor(&c.func).is_some() || format.is_some() {
                    // A direct call: the name argument was checked above
                    // (a literal is scanned like every string literal).
                    // `str.format` reads the attributes its format string
                    // names: a computed format string is a computed name.
                    if let Some(a) = format
                        && !matches!(&*a.value, Expr::Str(_))
                    {
                        self.found(c.range, Route::FixOnly);
                    }
                    if let Expr::Attribute(a) = &*c.func {
                        self.visit_expr(&a.value);
                    }
                    for a in &c.args {
                        self.visit_expr(a);
                    }
                    for k in &c.keywords {
                        self.visit_expr(&k.value);
                    }
                    if let Some(s) = &c.starargs {
                        self.visit_expr(s);
                    }
                    if let Some(k) = &c.kwargs {
                        self.visit_expr(k);
                    }
                    return;
                }
                if let Expr::Name(f) = &*c.func
                    && FRAME_BUILTINS.contains(&&*f.id)
                {
                    // A direct call: `__import__` hands out any module
                    // (the builtins too, [`import_call_route`]) and
                    // `compile` builds code, escapes whatever their
                    // arguments; the others are handled by the calling
                    // scope unless the arguments are unpacked.
                    // `vars(o)` is the namespace of `o` as a dictionary:
                    // its keys are member names (verified:
                    // `vars(Object)['getClass'](x)` is `x.getClass()`).
                    let unpacked = c.starargs.is_some() || c.kwargs.is_some();
                    if &*f.id == "__import__" {
                        self.found(c.range, import_call_route(c));
                    } else if &*f.id == "compile" {
                        // Builds code that only `exec` and `eval` run, which
                        // are routes of their own.
                        self.found(c.range, Route::FixOnly);
                    } else if unpacked {
                        // `eval(*a)`, `vars(*a)`: may read the caller's frame.
                        self.found(c.range, Route::Observes);
                    } else if &*f.id == "vars" && !c.args.is_empty() {
                        // A member dictionary of a value; of a module, its
                        // namespace with its `__builtins__`.
                        let route = if self.module_value(&c.args[0]) {
                            Route::Observes
                        } else {
                            Route::FixOnly
                        };
                        self.found(c.range, route);
                    }
                    // `eval('tb.tb_frame')`: the code names members.
                    if matches!(&*f.id, "eval" | "execfile" | "input")
                        && let Some(code) = c.args.first()
                    {
                        self.sink(code);
                    }
                    for a in &c.args {
                        self.visit_expr(a);
                    }
                    for k in &c.keywords {
                        self.visit_expr(&k.value);
                    }
                    if let Some(s) = &c.starargs {
                        self.visit_expr(s);
                    }
                    if let Some(k) = &c.kwargs {
                        self.visit_expr(k);
                    }
                    return;
                }
            }
            Expr::Attribute(a) => {
                let attr = &*a.attr.id;
                let module = self.module(&a.value);
                // A reflective accessor or `format` method used as a value
                // (direct calls returned above): `get = getattr`,
                // `f = operator.attrgetter`, `fmt = template.format`.
                // As a value, the accessor or format method hides every
                // call of it, like a bare accessor under another name.
                if REFLECTIVE_ACCESSORS.contains(&attr)
                    || (FORMAT_METHODS.contains(&attr) && !matches!(&*a.value, Expr::Str(_)))
                {
                    self.found(a.range, Route::Observes);
                }
                // `f = '{0.gi_frame}'.format`: the literal is a format string.
                if FORMAT_METHODS.contains(&attr) {
                    self.sink(&a.value);
                }
                let member = module
                    .filter(|m| module_frame_names(m).is_some_and(|names| names.contains(&attr)));
                let route = match member {
                    Some(m) => Some(module_member_route(m, attr)),
                    // The namespace of a module that hands out frames,
                    // modules or the builtins (`sys.__dict__`,
                    // `importlib.__getattribute__`) observes, like the
                    // module as a value.
                    None if module.is_some() && is_introspection_name(attr) => {
                        Some(Route::Observes)
                    }
                    // A module's `sys` attribute is the `sys` module
                    // (`re.sys`, `os.sys`, `importlib.sys`), which hands out
                    // frames, every module and the builtins. On any
                    // receiver (review round 26, N1): an alias or parameter
                    // may hold such a module, and any object's attribute
                    // may have been set to `sys`; only a literal could not.
                    None if attr == "sys" && a.ctx == ExprContext::Load => Some(Route::Observes),
                    // The namespace of a value that may be a module holds
                    // its `__builtins__` (`m.__dict__`, `m.dict`).
                    None if NAMESPACE_ATTRS.contains(&attr) && self.module_value(&a.value) => {
                        Some(Route::Observes)
                    }
                    None => introspection_route(attr),
                };
                if let Some(route) = route {
                    self.found(a.range, route);
                }
                if module.is_some() {
                    // `sys.path`, `inspect.getargspec(f)`: the module is
                    // not a value.
                    return;
                }
            }
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escape(src: &str) -> bool {
        let parsed = iglint_python::parse_module(src);
        assert!(parsed.errors.is_empty(), "{src}");
        frame_escape(&parsed.module.body).is_some()
    }

    #[test]
    fn routes_are_escapes() {
        for src in [
            // route 3: interpreter / module escapes
            "e = eval\n",
            "def f():\n    return map(locals, [1])\n",
            "f(vars)\n",
            "vars(*a)\n",
            "import __builtin__\n",
            "import __builtin__ as b\n",
            "from __builtin__ import locals as capture\n",
            "x = __builtins__\n",
            "__builtin__.eval('x')\n",
            "import inspect\ninspect.currentframe()\n",
            "from inspect import stack\n",
            "import inspect as i\nx = i\n",
            "import importlib\n",
            "from imp import load_source\n",
            "import gc\ngc.get_referrers(x)\n",
            "sys._getframe(0)\n",
            "import sys as s\ns.modules['a']\n",
            "from sys import modules\n",
            "from sys import *\n",
            "getattr(sys, name)\n",
            "sys.settrace(t)\n",
            "sys.exc_info()\n",
            "__import__('os')\n",
            "compile(s, '<s>', 'exec')\n",
            "import traceback\ntraceback.format_exc()\n",
            // route 3: Java / Jython reflection
            "from org.python.core import Py\n",
            "import org.python.core as core\n",
            "from java.lang.reflect import Field\n",
            "from java.lang import Class\n",
            "import java.lang.invoke\n",
            "from javax.script import ScriptEngineManager\n",
            "x = obj.getClass()\n",
            // route 1: introspection attributes
            "f.func_globals['a'] = 1\n",
            "f.__globals__\n",
            "tb.tb_frame.f_locals\n",
            "x = tb.tb_lineno\n",
            "n = c.co_firstlineno\n",
            "d = o.__dict__\n",
            // route 1: introspection names in string literals
            "getattr(tb, 'tb_frame')\n",
            "getattr(Py.getFrame(), 'f_locals')\n",
            "operator.attrgetter('tb_frame.f_back.f_back.f_locals')(tb)\n",
            "tb.__getattribute__('tb_frame')\n",
            "setattr(f, 'func_code', g)\n",
            // route 2: reflective accessor, computed name
            "getattr(tb, 'tb_' + 'frame')\n",
            "getattr(o, name)\n",
            "setattr(o, name, v)\n",
            "operator.attrgetter(name)(o)\n",
            "o.__getattribute__(name)\n",
            // route 2: a reflective accessor under another name or as a
            // value, and the `class` bean property (reviewer repro R6-E3)
            "get = getattr\nframe = get(g, 'gi_' + 'frame')\n",
            "f = operator.attrgetter\n",
            "map(getattr, objs, names)\n",
            "getattr(o, 'class')\n",
            // route 2: format strings name attributes
            "s = ('{0.gi_' + 'frame}').format(g)\n",
            "s = fmt.format(g)\n",
            "f = template.format\n",
            "s = str.format(fmt, g)\n",
            // route 1: class hierarchies, Java class loaders
            "c = object.__subclasses__()\n",
            "b = C.__mro__\n",
            "l = Thread.currentThread().getContextClassLoader()\n",
            "c = loader.loadClass(name)\n",
            // route 1: a byte string that is not valid UTF-8 must still be
            // scanned (decoded as Latin-1), not read as empty (H2).
            "s = '{0.gi_frame.f_back.f_locals}\\xff'.format(it)\n",
            "getattr(tb, 'tb_frame\\xff')\n",
            // route 1: Java bean-property reflection spellings (the
            // machine-measured JAVA_REFLECTIVE_MEMBERS). Reviewer round 7
            // finding 1: `.class` / `.classLoader` / `.methods` reach Java
            // reflection.
            "loader = obj.class.classLoader\n",
            "ms = loader.class.methods\n",
            "c = x.declaredMethods\n",
            "getattr(o, 'classLoader')\n",
            "s = '{0.class.classLoader}'.format(o)\n",
            // Reviewer round 8 finding 1: the closure over every type a
            // script can hold. A class proxy exposes java.lang.Class
            // (`ArrayList.interfaces`, `.module`); a reflected function
            // its `argslist`; Perspective wrappers reach the gateway's
            // script manager; Jython's lookups by name.
            "from java.util import ArrayList\nm = ArrayList.module\n",
            "from java.util import ArrayList\ni = ArrayList.getInterfaces()\n",
            "m = system.tag.readBlocking.argslist\n",
            "sm = self.session.getGatewayContext().getScriptManager()\n",
            "t = ds.getColumnTypes()\n",
            "c = qv.value.getClass()\n",
            "a = self.props.invoke(name)\n",
            "a = self.props.__findattr__(name)\n",
            "f = self.props.invoke\n",
        ] {
            assert!(escape(src), "{src}");
        }
    }

    /// Review round 24 (recall): every route makes fixes unsafe
    /// ([`frame_escape`]), only routes that observe names by themselves
    /// silence findings ([`frame_observer`]). The table in
    /// `docs/internal/fix-safety.md` ("Frame routes: findings and fixes")
    /// lists the same decisions.
    #[test]
    fn routes_are_classified() {
        let route = |src: &str| {
            let parsed = iglint_python::parse_module(src);
            assert!(parsed.errors.is_empty(), "{src}");
            match escapes(&parsed.module.body) {
                (None, None) => None,
                (Some(_), None) => Some(Route::FixOnly),
                (Some(_), Some(_)) => Some(Route::Observes),
                (None, Some(_)) => panic!("an observer is an escape: {src}"),
            }
        };
        for src in [
            // frame builtins under another name, the builtins module
            "e = eval\n",
            "f(vars)\n",
            "vars(*a)\n",
            "eval(*a)\n",
            "import __builtin__\n",
            "from __builtin__ import locals as capture\n",
            "x = __builtins__\n",
            "__import__('__builtin__')\n",
            "__import__('sys')\n",
            "__import__(name)\n",
            // frames, tracebacks reaching frames, trace hooks, globals
            "sys._getframe(0)\n",
            "sys._getframe(0).f_locals\n",
            "from sys import _getframe\n",
            "from sys import *\n",
            "sys.modules['a']\n",
            "sys.settrace(t)\n",
            "import inspect\ninspect.currentframe()\n",
            "from inspect import stack\n",
            "import inspect as i\nx = i\n",
            "x = sys\n",
            "import gc\ngc.get_referrers(x)\n",
            "tb.tb_frame\n",
            "f = frame.f_back\n",
            "g.gi_frame\n",
            "f.func_globals['a'] = 1\n",
            "f.__globals__\n",
            "getattr(tb, 'tb_frame')\n",
            "'{0.f_locals}'.format(frame)\n",
            "x = eval('frame.f_globals')\n",
            // the interpreter's own classes
            "from org.python.core import Py\n",
            "import org.python.core as core\n",
            "from org.python import core\n",
            // an accessor or format method under another name hides its calls
            "get = getattr\n",
            "map(getattr, objs, names)\n",
            "f = operator.attrgetter\n",
            "f = template.format\n",
            // module loaders that hand out the builtins or any module, and
            // the builtins of Jython's `sys` (review round 25, M2)
            "import importlib\nm = importlib.import_module('__builtin__')\n",
            "import importlib as il\nm = il.import_module(name)\n",
            "import importlib\nm = importlib.import_module('.x', p)\n",
            "import importlib\nm = importlib.import_module(*a)\n",
            "import importlib\nf = importlib.import_module\n",
            "from importlib import import_module\n",
            "import importlib\nf(importlib)\n",
            "from imp import load_source\n",
            "import imp\nm = imp.load_module(n, f, p, d)\n",
            "b = sys.builtins\n",
            "b = sys.getBuiltins()\n",
            "b = sys.getDefaultBuiltins()\n",
            "d = sys.__dict__\n",
            "import re\nm = re.sys.modules\n",
            "import importlib\nm = importlib.sys\n",
        ] {
            assert_eq!(route(src), Some(Route::Observes), "{src}");
        }
        for src in [
            // measured reflective Java members and Java reflection
            "def g(o):\n    return o.type\n",
            "t = ds.getColumnType(0)\n",
            "n = o.__class__.__name__\n",
            "x = o.fields\n",
            "x = o.base\n",
            "x = e.traceback\n",
            "c = obj.getClass()\n",
            "c = loader.loadClass(name)\n",
            "from java.lang import Class\n",
            "from java.lang.reflect import Field\n",
            "from javax.script import ScriptEngineManager\n",
            "getattr(o, 'classLoader')\n",
            "getattr(o, 'class')\n",
            // computed names and format strings
            "getattr(o, name)\n",
            "setattr(o, name, v)\n",
            "s = fmt.format(g)\n",
            "s = str.format(fmt, g)\n",
            "f = vars(Object)['getClass']\n",
            // line numbers, code objects, class hierarchies, method internals
            "x = tb.tb_lineno\n",
            "x = sys.exc_info()[2].tb_lineno\n",
            "t, v, tb = sys.exc_info()\n",
            "from sys import exc_info\n",
            "n = c.co_firstlineno\n",
            "c = f.func_code\n",
            "d = o.__dict__\n",
            "b = C.__mro__\n",
            "s = m.im_self\n",
            "import inspect\ninspect.getmembers(o)\n",
            // module loaders, rendering tracebacks, Jython value classes
            "import traceback\ntraceback.format_exc()\n",
            "import importlib\n",
            "import imp\nm = imp.new_module('m')\n",
            "import importlib\nm = importlib.import_module('json')\n",
            "from org.python.core import PyString\n",
            "__import__('os')\n",
            "compile(s, '<s>', 'exec')\n",
            // namespaces of values that may be library modules or packages
            // (an alias or a parameter may hold one), code from data
            // (review round 26)
            "x = o.dict\n",
            "d = o.getDict()\n",
            "o.setCode(s)\n",
            "s = o.__reduce_ex__(2)\n",
            "s = o.__reduce__()\n",
            "getattr(o, 'dict')\n",
            "x = o.__getattribute__('getDict')\n",
        ] {
            assert_eq!(route(src), Some(Route::FixOnly), "{src}");
        }
        for src in [
            "x = locals()\n",
            "eval('1')\n",
            "x.modules\n",
            "getattr(o, 'value')\n",
        ] {
            assert_eq!(route(src), None, "{src}");
        }
    }

    /// Review round 26 (M2): the namespace of a module holds its
    /// `__builtins__` (`vars(json)['__builtins__']['locals']()` and
    /// `m.dict['__builtins__']['locals']()` read the caller's frame,
    /// measured on 8.3.3), so on a value that may be a module it observes;
    /// on any other value it stays a first hop, as does a computed name.
    #[test]
    fn module_namespaces_observe() {
        let route = |src: &str| {
            let parsed = iglint_python::parse_module(src);
            assert!(parsed.errors.is_empty(), "{src}");
            let modules = |n: &ExprName| matches!(&*n.id, "m" | "json" | "pkg");
            match escapes_with(&parsed.module.body, &modules) {
                (None, None) => None,
                (Some(_), None) => Some(Route::FixOnly),
                (Some(_), Some(_)) => Some(Route::Observes),
                (None, Some(_)) => panic!("an observer is an escape: {src}"),
            }
        };
        for src in [
            "vars(json)['__builtins__']['locals']()\n",
            "vars(m)\n",
            "d = m.__dict__\n",
            "d = pkg.m.__dict__\n",
            "m.dict['__builtins__']['locals']()\n",
            "d = m.getDict()\n",
            "d = pkg.dict\n",
            "s = m.__reduce_ex__(2)[2]\n",
            "getattr(m, '__dict__')\n",
            "getattr(m, 'dict')\n",
            "m.__getattribute__('__dict__')\n",
            "m.__findattr_ex__('getDict')()\n",
        ] {
            assert_eq!(route(src), Some(Route::Observes), "{src}");
        }
        for src in [
            "getattr(m, n)\n",
            "m.__getattribute__(n)\n",
            "vars(o)\n",
            "d = o.__dict__\n",
            "d = o.dict\n",
            "getattr(o, '__dict__')\n",
        ] {
            assert_eq!(route(src), Some(Route::FixOnly), "{src}");
        }
        for src in [
            "x = m.f\n",
            "m.name\n",
            "getattr(m, 'f')\n",
            "n = m.__name__\n",
        ] {
            assert_eq!(route(src), None, "{src}");
        }
    }

    #[test]
    fn plain_code_is_not_an_escape() {
        for src in [
            "x = locals()\n",
            "eval('1')\n",
            "dir(x)\n",
            "import sys\nsys.path.append('a')\n",
            "import os.path\nfrom java.lang import System\n",
            "x.modules\n",
            "import inspect\ninspect.getargspec(f)\n",
            "import gc\ngc.collect()\n",
            // reflective accessor with a plain (non-introspection) literal
            "getattr(o, 'value')\n",
            "setattr(o, 'count', 1)\n",
            "hasattr(o, 'items')\n",
            "s = '{0} of {1}'.format(a, b)\n",
            "s = '%s' % name\n",
            "a = self.props.invoke('refresh')\n",
            // ordinary prose and names that merely contain a word
            "msg = 'the frame could not be found'\n",
            "label = 'function code'\n",
            "x = co_worker\n",
        ] {
            assert!(!escape(src), "{src}");
        }
    }

    /// Reviewer round 8, finding 1 (narrowed string scan): a string literal
    /// is scanned only in a sink position. Every sink with a literal
    /// introspection name is an escape, with a plain literal name it is
    /// not, and with a name that is not a literal at the call (a variable,
    /// concatenation, `join`, `%`, slicing, a container, a function
    /// return) it is an escape whatever the string. Literals outside sinks
    /// are data.
    #[test]
    fn string_sinks() {
        // (sink with `{}` for the name argument, literal plain name works)
        let sinks: &[(&str, bool)] = &[
            ("getattr(o, {})", true),
            ("hasattr(o, {})", true),
            ("setattr(o, {}, 1)", true),
            ("delattr(o, {})", true),
            ("o.__getattribute__({})", true),
            ("o.__getattr__({})", true),
            ("o.__findattr__({})", true),
            ("o.__findattr_ex__({})", true),
            ("o.invoke({})", true),
            ("operator.attrgetter({})(o)", true),
            ("operator.attrgetter('a', {})(o)", true),
            ("operator.methodcaller({})(o)", true),
        ];
        let flows = [
            "n",
            "'tb_' + 'frame'",
            "''.join(['tb_', 'frame'])",
            "'%s' % 'tb_frame'",
            "'xtb_frame'[1:]",
            "['tb_frame'][0]",
            "{'k': 'tb_frame'}['k']",
            "name()",
        ];
        for (sink, plain_ok) in sinks {
            let with = |arg: &str| format!("x = {}\n", sink.replace("{}", arg));
            assert!(escape(&with("'tb_frame'")), "{}", with("'tb_frame'"));
            assert!(
                escape(&with("'f_back.f_locals'")),
                "{}",
                with("'f_back.f_locals'")
            );
            assert!(escape(&with("'classLoader'")), "{}", with("'classLoader'"));
            if *plain_ok {
                assert!(!escape(&with("'value'")), "{}", with("'value'"));
            }
            for flow in flows {
                assert!(escape(&with(flow)), "{}", with(flow));
            }
        }
        // Format strings: a literal receiver is scanned, also as a value;
        // any other receiver is computed.
        for src in [
            "s = '{0.gi_frame}'.format(g)\n",
            "s = u'{0.f_back}'.format(g)\n",
            "f = '{0.f_back}'.format\n",
            "s = '{0.f_back}'.vformat(g)\n",
            "s = str.format('{0.x}', g)\n",
            "s = fmt.format(g)\n",
            "s = ('{0.' + 'x}').format(g)\n",
            "s = string.Formatter().format('{0.x}', g)\n",
            "s = ''.join(['{0.', 'f_back}']).format(g)\n",
        ] {
            assert!(escape(src), "{src}");
        }
        assert!(!escape("s = '{0.value} {1}'.format(a, b)\n"));
        // Code literals.
        for src in [
            "x = eval('tb.tb_frame')\n",
            "exec 'x = tb.tb_frame'\n",
            "def f():\n    exec 'x = o.classLoader' in {}\n",
        ] {
            assert!(escape(src), "{src}");
        }
        // Member dictionaries are escapes whatever the key.
        assert!(escape("f = vars(Object)['getClass']\n"));
        assert!(escape("f = Object.__dict__['value']\n"));
        // Not sinks (verified on Jython): data, `%` mappings, templates,
        // format specs.
        for src in [
            "names = ['tb_frame', 'f_locals', 'classLoader', 'type', 'module']\n",
            "d = {'type': 'popup', 'module': 'x'}\n",
            "log('the frame f_locals of the module')\n",
            "s = '%(f_locals)s' % d\n",
            "s = string.Template('$f_locals').substitute(d)\n",
            "s = format(x, 'f_back')\n",
            "self.props.kind = 'module'\n",
        ] {
            assert!(!escape(src), "{src}");
        }
    }

    #[test]
    fn direct_calls_read_frames() {
        let reads = |src: &str| {
            let parsed = iglint_python::parse_module(src);
            reads_frames(&parsed.module.body).is_some()
        };
        assert!(reads("x = locals()\n"));
        assert!(reads("def f():\n    exec 'a = 1'\n"));
        assert!(reads("dir()\n"));
        assert!(reads("import traceback\nx = traceback.format_exc()\n"));
        assert!(reads(
            "self.props.text = str(sys.exc_info()[2].tb_lineno)\n"
        ));
        assert!(!reads("dir(x)\n"));
        // `vars(o)` is a member dictionary (`vars(Object)['getClass']`).
        assert!(reads("vars(y)\n"));
    }
}
