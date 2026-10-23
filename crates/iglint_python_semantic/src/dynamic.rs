//! Ways code can reach the namespaces of running frames other than by
//! naming variables: the builtins that read or write the caller's frame
//! (`eval`, `locals`, ...) used other than by a direct call, the builtins
//! module, frame objects and function globals.
//!
//! A direct call of a frame builtin by its bare name (`locals()`,
//! `eval(s)`) is handled precisely by the scope that makes it
//! ([`Scope::reads_locals`](crate::Scope::reads_locals)). Every other
//! route is an *escape*: the builtin (or a frame) may be called from any
//! scope under any name, so analyses that need to know which names are
//! read must assume all of them are, in every scope of the module. The
//! routes, verified on Jython 2.7 (Ignition 8.3): a builtin used under
//! another name reads the frame of whoever calls it, also across modules
//! (`ev = eval` in one module, `m.ev('x')` in another reads the caller's
//! `x`).
//!
//! | route | example |
//! |---|---|
//! | a frame builtin as a value | `e = eval`, `map(locals, ...)`, `f(vars)` |
//! | a frame builtin called with `*args` / `**kwargs` | `vars(*a)` |
//! | the builtins module | `__builtin__`, `__builtins__`, `import __builtin__`, `from __builtin__ import locals as l` |
//! | modules that hand out modules or builtins | `import importlib`, `imp` |
//! | functions of `sys`, `inspect` and `gc` that reach frames, every module or trace hooks, and these modules used as values | `sys._getframe`, `sys.modules`, `sys.settrace`, `from sys import modules`, `inspect.currentframe()`, `gc.get_referrers`, `getattr(sys, name)` |
//! | frame and function attributes | `.f_locals`, `.f_globals`, `.f_back`, `.tb_frame`, `.gi_frame`, `.func_globals`, `.__globals__` |

use iglint_python::ast::{Expr, ExprContext, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

/// Builtins that read or write the namespaces of the frame that calls
/// them.
pub const FRAME_BUILTINS: &[&str] = &[
    "eval",
    "execfile",
    "locals",
    "globals",
    "vars",
    "dir",
    "input",
    "__import__",
];

/// Modules whose every use hands out other modules or the builtins.
const FRAME_MODULES: &[&str] = &["__builtin__", "importlib", "imp"];

/// Modules with some functions that reach frames, every module or trace
/// hooks: the names that do. Any other use of the module object (as a
/// value, `getattr(sys, name)`) counts too.
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

/// Attributes of frames, tracebacks, generators and functions that reach
/// another frame or a module namespace.
pub const FRAME_ATTRIBUTES: &[&str] = &[
    "_getframe",
    "_current_frames",
    "f_locals",
    "f_globals",
    "f_builtins",
    "f_back",
    "tb_frame",
    "gi_frame",
    "func_globals",
    "__globals__",
    "__builtins__",
    "settrace",
    "setprofile",
    "currentframe",
    "getouterframes",
    "getinnerframes",
];

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
                    "__import__" => false,
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
}

impl<'a> Visitor<'a> for Escapes {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(i) => {
                for a in &i.names {
                    let root = a.name.id.split('.').next().unwrap_or("");
                    if FRAME_MODULES.contains(&root) {
                        self.found(a.range);
                    }
                }
            }
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref().map_or("", |m| &*m.id);
                let root = module.split('.').next().unwrap_or("");
                if i.level == 0 && FRAME_MODULES.contains(&root) {
                    self.found(i.range);
                }
                if i.level == 0
                    && let Some(names) = module_frame_names(module)
                {
                    for a in &i.names {
                        if &*a.name.id == "*" || names.contains(&&*a.name.id) {
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
                    || (n.ctx == ExprContext::Load && self.modules.contains_key(&*n.id))
                {
                    // Direct calls and attribute reads are taken apart
                    // below; a bare name reaching here is a value.
                    self.found(n.range);
                }
                return;
            }
            Expr::Call(c) => {
                if let Expr::Name(f) = &*c.func
                    && FRAME_BUILTINS.contains(&&*f.id)
                {
                    // A direct call: `__import__` hands out any module
                    // (the builtins too), the others are handled by the
                    // calling scope unless the arguments are unpacked.
                    if &*f.id == "__import__" || c.starargs.is_some() || c.kwargs.is_some() {
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
                if FRAME_ATTRIBUTES.contains(&attr)
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
            "f.func_globals['a'] = 1\n",
            "f.__globals__\n",
            "tb.tb_frame.f_locals\n",
            "__import__('os')\n",
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
            "import sys\nsys.exc_info()\nsys.path.append('a')\n",
            "import os.path\nfrom java.lang import System\n",
            "x.modules\n",
            "import inspect\ninspect.getargspec(f)\n",
            "import gc\ngc.collect()\n",
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
        assert!(!reads("dir(x)\nvars(y)\n"));
    }
}
