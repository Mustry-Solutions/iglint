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

use iglint_python::ast::{Expr, ExprContext, Stmt};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_text::TextRange;
use rustc_hash::FxHashMap;

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

/// What a name bound by an import refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    /// A vetted Python module (its key in [`VETTED_MODULES`], or `os`,
    /// `sys`, `system`).
    Module(&'static str),
    /// A vetted Java class (qualified name).
    JavaClass(&'static str),
    /// Something else that is vetted (a function or class of a vetted
    /// module).
    Value,
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
    /// The first use of something that is not vetted and not an import of
    /// a module that may be a project library module.
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

/// Every use in `body` (nested scopes included) of a module, Java class,
/// builtin or module attribute that is not vetted. See the module
/// documentation.
pub fn unknown_code(body: &[Stmt]) -> UnknownCode {
    let mut v = Walk {
        bound: FxHashMap::default(),
        out: UnknownCode::default(),
    };
    v.bound.insert("sys".to_owned(), Bound::Module("sys"));
    v.bound.insert("system".to_owned(), Bound::Module("system"));
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.out
}

struct Walk {
    /// Names bound by vetted imports.
    bound: FxHashMap<String, Bound>,
    out: UnknownCode,
}

impl Walk {
    fn other(&mut self, range: TextRange) {
        if self.out.other.is_none() {
            self.out.other = Some(range);
        }
    }

    /// Checks an attribute chain rooted at a name bound to a vetted module
    /// or class; returns `false` when the root is not such a name.
    fn chain(&mut self, expr: &Expr) -> bool {
        let mut attrs = Vec::new();
        let mut root = expr;
        while let Expr::Attribute(a) = root {
            attrs.push((a.attr.as_str(), a.range));
            root = &a.value;
        }
        let Expr::Name(n) = root else { return false };
        let Some(mut bound) = self.bound.get(&*n.id).copied() else {
            return false;
        };
        for (attr, range) in attrs.into_iter().rev() {
            bound = match bound {
                Bound::Module(m) => match module_attribute(m, attr) {
                    Some(next) => next,
                    None => {
                        self.other(range);
                        return true;
                    }
                },
                Bound::JavaClass(q) => {
                    if JAVA_DENIED_MEMBERS
                        .iter()
                        .any(|(c, members)| *c == q && members.contains(&attr))
                    {
                        self.other(range);
                    }
                    return true;
                }
                Bound::Value => return true,
            };
        }
        true
    }
}

impl<'a> Visitor<'a> for Walk {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Import(i) => {
                for a in &i.names {
                    let name = a.name.as_str();
                    let root = name.split('.').next().unwrap_or(name);
                    // `import a.b` binds `a`; `import a.b as c` binds `c`
                    // to `a.b`.
                    let (bound_name, target) = match &a.asname {
                        Some(n) => (n.as_str(), name),
                        None if name == "os.path" => ("os", "os"),
                        None if root == "system" => ("system", "system"),
                        None if name.contains('.') => (root, ""),
                        None => (name, name),
                    };
                    match vetted_module(target) {
                        Some(m) => {
                            self.bound.insert(bound_name.to_owned(), Bound::Module(m));
                            self.out.vetted_imports.push(root.to_owned());
                        }
                        None => {
                            self.bound.remove(bound_name);
                            if root == "java" || root == "javax" || vetted_module(name).is_some() {
                                self.other(a.range);
                            } else {
                                self.out.imports.push((name.to_owned(), a.range));
                            }
                        }
                    }
                }
            }
            Stmt::ImportFrom(i) => {
                let module = i.module.as_ref().map_or("", |m| m.as_str());
                if i.level != 0 {
                    self.out.imports.push((module.to_owned(), i.range));
                } else if let Some(m) = vetted_module(module) {
                    self.out
                        .vetted_imports
                        .push(module.split('.').next().unwrap_or(module).to_owned());
                    for a in &i.names {
                        let bound_name = a.asname.as_ref().map_or(a.name.as_str(), |n| n.as_str());
                        match module_attribute(m, a.name.as_str()) {
                            Some(b) if a.name.as_str() != "*" => {
                                self.bound.insert(bound_name.to_owned(), b);
                            }
                            _ => {
                                self.bound.remove(bound_name);
                                self.other(a.range);
                            }
                        }
                    }
                } else if module == "java"
                    || module.starts_with("java.")
                    || module.starts_with("javax")
                {
                    for a in &i.names {
                        let bound_name = a.asname.as_ref().map_or(a.name.as_str(), |n| n.as_str());
                        match java_class(module, a.name.as_str()) {
                            Some(q) => {
                                self.bound
                                    .insert(bound_name.to_owned(), Bound::JavaClass(q));
                            }
                            None => {
                                self.bound.remove(bound_name);
                                self.other(a.range);
                            }
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
                        && matches!(self.bound.get(&*n.id), Some(Bound::JavaClass(_)))
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
            Expr::Attribute(_) => {
                if self.chain(expr) {
                    return;
                }
            }
            Expr::Name(n) if n.ctx == ExprContext::Load => {
                match self.bound.get(&*n.id) {
                    // A module object used as a value: what is read from
                    // it later is not checked.
                    Some(Bound::Module(_)) => self.other(n.range),
                    Some(_) => {}
                    None => {
                        if iglint_stubs::is_jython_builtin(&n.id) && !builtin_vetted(&n.id) {
                            self.other(n.range);
                        }
                    }
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
        unknown_code(&parsed.module.body)
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
