//! Ways code can observe a running frame's namespace, a module's
//! namespace or the identity of a function other than by naming
//! variables.
//!
//! # Role
//!
//! These are the routes that the analyses of iglint *understand*: when a
//! module takes one, every scope of the module is treated as reading its
//! locals and writing its globals (the module **fails closed**). They are
//! not the safety argument for fixes on their own: an enumerated list of
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
//!    Every string literal of the module is scanned, matching whole
//!    identifiers so that ordinary prose does not trigger.
//! 2. **A reflective accessor with a computed name or used as a value.**
//!    `getattr`, `setattr`, `delattr`, `hasattr`, `attrgetter`,
//!    `methodcaller`, `__getattribute__` ([`REFLECTIVE_ACCESSORS`]) are
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
//! # Line numbers
//!
//! Line numbers are not something these routes protect: safe fixes never
//! move a line (the engine rejects any that would), so whatever reads them
//! (tracebacks, `warnings`, `logging`, Java stack traces) sees the same
//! numbers after a safe fix. Routes that only expose line numbers are kept
//! because they also reach frames.

use iglint_python::ast::{Expr, ExprContext, Stmt, StringValue};
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

/// Frame builtins that are an escape even when called directly by their
/// bare name: `__import__` hands out any module (the builtins too) and
/// `compile` builds code for `exec` / `eval`.
const INTERP_BUILTINS: &[&str] = &["__import__", "compile"];

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
    // generic introspection
    "__dict__",
    "__getattribute__",
    "__builtins__",
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

/// Whether `name`, a whole identifier, is an introspection attribute or a
/// frame-reaching method name.
fn is_introspection_name(name: &str) -> bool {
    INTROSPECTION_ATTRS.contains(&name) || FRAME_METHOD_NAMES.contains(&name)
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
fn string_names_introspection(value: &StringValue) -> bool {
    value
        .to_latin1()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(is_introspection_name)
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

/// The first escape route of `body` (nested scopes included), if any.
pub fn frame_escape(body: &[Stmt]) -> Option<TextRange> {
    let mut v = Escapes {
        modules: module_names(body),
        found: None,
    };
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.found
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

struct Escapes {
    /// Names bound to modules of [`PARTLY_FRAME_MODULES`].
    modules: FxHashMap<String, &'static str>,
    found: Option<TextRange>,
}

impl Escapes {
    fn found(&mut self, range: TextRange) {
        if self.found.is_none() {
            self.found = Some(range);
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
            "methodcaller" | "__getattribute__" | "__getattr__" | "__setattr__" | "__delattr__" => {
                c.args.first().is_none_or(|a| !is_literal(a))
            }
            // `getattr(o, name)`, `setattr(o, name, v)`: the second.
            _ => c.args.get(1).is_none_or(|a| !is_literal(a)),
        }
    }
}

impl<'a> Visitor<'a> for Escapes {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(i) => {
                for a in &i.names {
                    let root = a.name.id.split('.').next().unwrap_or("");
                    if FRAME_MODULES.contains(&root) || is_reflection_import(&a.name.id) {
                        self.found(a.range);
                    }
                }
            }
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref().map_or("", |m| &*m.id);
                let root = module.split('.').next().unwrap_or("");
                if i.level == 0 && (FRAME_MODULES.contains(&root) || is_reflection_import(module)) {
                    self.found(i.range);
                }
                if i.level == 0 {
                    if let Some(names) = module_frame_names(module) {
                        for a in &i.names {
                            if &*a.name.id == "*" || names.contains(&&*a.name.id) {
                                self.found(a.range);
                            }
                        }
                    }
                    for a in &i.names {
                        if is_reflection_from_java_lang(module, &a.name.id) {
                            self.found(a.range);
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
                if matches!(&*n.id, "__builtin__" | "__builtins__")
                    || (n.ctx == ExprContext::Load && FRAME_BUILTINS.contains(&&*n.id))
                    || (n.ctx == ExprContext::Load && REFLECTIVE_ACCESSORS.contains(&&*n.id))
                    || (n.ctx == ExprContext::Load && self.modules.contains_key(&*n.id))
                {
                    // Direct calls and attribute reads are taken apart
                    // below; a bare name reaching here is a value.
                    self.found(n.range);
                }
                return;
            }
            Expr::Str(s) => {
                if string_names_introspection(&s.s) {
                    self.found(s.range);
                }
                return;
            }
            Expr::Call(c) => {
                if Escapes::reflective_escape(c) {
                    self.found(c.range);
                }
                let format = match &*c.func {
                    Expr::Attribute(a) if FORMAT_METHODS.contains(&a.attr.as_str()) => Some(a),
                    _ => None,
                };
                if Escapes::reflective_accessor(&c.func).is_some() || format.is_some() {
                    // A direct call: the name argument was checked above
                    // (a literal is scanned like every string literal).
                    // `str.format` reads the attributes its format string
                    // names: a computed format string is a computed name.
                    if let Some(a) = format
                        && !matches!(&*a.value, Expr::Str(_))
                    {
                        self.found(c.range);
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
                    // (the builtins too) and `compile` builds code; the
                    // others are handled by the calling scope unless the
                    // arguments are unpacked.
                    if INTERP_BUILTINS.contains(&&*f.id)
                        || c.starargs.is_some()
                        || c.kwargs.is_some()
                    {
                        self.found(c.range);
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
                if REFLECTIVE_ACCESSORS.contains(&attr)
                    || (FORMAT_METHODS.contains(&attr) && !matches!(&*a.value, Expr::Str(_)))
                {
                    self.found(a.range);
                }
                if is_introspection_name(attr)
                    || module
                        .and_then(module_frame_names)
                        .is_some_and(|names| names.contains(&attr))
                {
                    self.found(a.range);
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
        ] {
            assert!(escape(src), "{src}");
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
            // ordinary prose and names that merely contain a word
            "msg = 'the frame could not be found'\n",
            "label = 'function code'\n",
            "x = co_worker\n",
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
        assert!(!reads("dir(x)\nvars(y)\n"));
    }
}
