//! SCR102 unused-import.

use iglint_project::ScriptKind;
use iglint_python::ast::Expr;
use iglint_python::ast::{Stmt, StmtImport, StmtImportFrom};
use iglint_python_semantic::{BindingId, BindingKind, ScopeId, ScopeKind, SemanticModel};
use iglint_text::Ranged;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::refs::{ExternalUse, external_use};
use crate::rules::script::{
    StatementEdit, containing_body, delete_statement, report, semantic, source, statement_fix,
};

pub static META: RuleMeta = RuleMeta {
    code: "SCR102",
    name: "unused-import",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "Imported name is never used",
    explanation: r#"## What it does

Reports imports whose name is never read. Usage is flow-sensitive: an
import that is rebound before any read is unused even if the name is read
later.

Not reported:

* `from __future__ import ...`;
* names listed in the module's `__all__` (and every module-level import when
  `__all__` is built dynamically);
* imports inside a `try` whose handlers catch `ImportError` (availability
  checks such as `try: import json except ImportError: json = None`);
* scopes that read their names dynamically (`locals()`, `vars()`, `eval`,
  `exec`), and module-level imports when any code of the module may read
  globals by name (`globals()`, `eval`, `execfile`, `exec`, `sys.modules`,
  frames, `__dict__`, `func_globals`);
* imports in a class body: they are attributes of the class (`self.os`,
  `C.os`), used in ways iglint cannot follow (pyflakes and pylint do not
  report them either);
* `from x import y` at module level of a **library module**: other scripts
  may use it as `library.module.y` (re-export). Plain `import x` is still
  reported there.

## Why is this bad?

Unused imports slow down module loading (library modules are imported in
every scope that uses them) and hide which dependencies a script really
has. They also often point at a typo where the import was meant to be used.

## Example

```python
import json
from java.lang import Thread

def onMessageReceived(self, payload):
    self.view.custom.data = system.util.jsonDecode(payload)
```

Use instead:

```python
def onMessageReceived(self, payload):
    self.view.custom.data = system.util.jsonDecode(payload)
```

## Fix

Removes the unused names from the import statement, or the statement when
none of its names is used (replaced by `pass` when it is the only
statement of its block; when every statement of a block is removed, the
last one becomes `pass`).

Safe only when importing cannot do anything outside the importing
module, so that removing the import cannot change what runs:

* Java classes and packages of the JDK (`java.*`, `javax.*`);
* plain `import x` of a standard library module that is known to have no
  import-time effects in Jython 2.7 (`os`, `re`, `json`, `datetime`,
  `math`, `time`, ...), and `from x import y` of such a module when it is
  not a package;

and, for both, when no library package of the project (or of a project
inheriting from it) has the same top-level name, and the module calls no
library name that is not a plain function, class or literal constant
(it may be `eval` or `locals` under another name, which reads the
caller's names). Unsafe otherwise:
importing a project library module runs its top level, other Java
classes run static initializers, some standard modules start threads
(`socket`) or configure logging (`logging`), and a dotted import makes the
submodule an attribute of its package for every other script. Also
unsafe for top-level imports of library modules when iglint cannot see
every use of the module's names (the module is used as a value,
references are computed at run time, the project is inheritable or has
scripts iglint does not model, or the file is linted outside its
project). The one remaining effect of a safe fix: an import that would
have failed (a class missing from the JDK) no longer fails.

Top-level names of a library module are attributes of the module: an
import that another resource uses as `pkg.mod.name` is not reported, nor
one that `from pkg.mod import *` in another resource binds there (every
name not starting with `_`, or the names of `__all__`).

## pylint

Corresponds to `W0611 unused-import`. Differences: iglint also reports an
import that is rebound before any read (pylint counts any read of the
name), and does not report availability checks in `try`/`except
ImportError` or `from x import y` re-exports in library modules (pylint
does).
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnusedImport)],
    options: None,
};

struct UnusedImport;

/// The unused import bindings of a model, by statement.
pub(crate) fn unused_imports<'a>(
    model: &SemanticModel<'a>,
    library_module: bool,
) -> Vec<(&'a Stmt, Vec<BindingId>)> {
    let mut by_stmt: FxHashMap<usize, (&'a Stmt, Vec<BindingId>)> = FxHashMap::default();
    for (id, b) in model.bindings() {
        if !matches!(b.kind, BindingKind::Import | BindingKind::FromImport)
            || b.is_used()
            || b.import_guarded
        {
            continue;
        }
        let Some(stmt) = b.stmt else { continue };
        let scope = model.scope(b.scope);
        if scope.reads_locals || scope.is_dynamic() {
            continue;
        }
        // Names bound in a class body are attributes of the class
        // (`self.os`, `C.os`): uses cannot be followed.
        if scope.kind == ScopeKind::Class {
            continue;
        }
        if b.scope == ScopeId::MODULE {
            if model.dunder_all_is_dynamic()
                || model.dunder_all().iter().any(|n| n == b.name)
                || reads_globals_dynamically(model)
            {
                continue;
            }
            if library_module && b.kind == BindingKind::FromImport {
                continue;
            }
        }
        // `import a.b` and `import a.c` both bind `a`: any read of `a`
        // uses every submodule import of it.
        if b.kind == BindingKind::Import
            && b.alias
                .is_some_and(|a| a.asname.is_none() && a.name.id.contains('.'))
            && model
                .scope(b.scope)
                .bindings_of(b.name)
                .iter()
                .any(|other| model.binding(*other).is_used())
        {
            continue;
        }
        by_stmt
            .entry(stmt as *const Stmt as usize)
            .or_insert_with(|| (stmt, Vec::new()))
            .1
            .push(id);
    }
    let mut out: Vec<_> = by_stmt.into_values().collect();
    out.sort_by_key(|(s, _)| s.range().start());
    out
}

/// Whether some code of the module may read its globals by name: `eval`,
/// `execfile`, `exec`, `globals`, `locals()`, `vars()` or `dir()` (in any
/// scope), or the module object through `sys.modules`, a frame,
/// `__dict__` or `getattr` with a computed name.
fn reads_globals_dynamically(model: &SemanticModel<'_>) -> bool {
    const READERS: &[&str] = &["eval", "execfile", "globals"];
    model.references().any(|(_, r)| READERS.contains(&r.name))
        || model
            .scopes()
            .any(|(_, s)| s.unqualified_exec.is_some() || s.reads_locals)
        || {
            let mut found = false;
            crate::rules::script::walk_stmts(&model.module.body, &mut |s| {
                found |= matches!(s, Stmt::Exec(_));
            });
            found
        }
        || {
            // `getattr(m, name)` with a computed name on a module object
            // (an imported name or a library package, possibly the module
            // itself) reads its globals by name.
            let mut found = false;
            crate::rules::script::walk_exprs(&model.module.body, &mut |e| {
                if let Expr::Call(c) = e
                    && let Expr::Name(f) = &*c.func
                    && matches!(&*f.id, "getattr" | "hasattr" | "setattr" | "delattr")
                    && c.args.get(1).is_none_or(|a| !matches!(a, Expr::Str(_)))
                    && c.args.first().is_some_and(|m| may_be_module(model, m))
                {
                    found = true;
                }
            });
            found
        }
        || {
            let mut found = false;
            crate::rules::script::walk_exprs(&model.module.body, &mut |e| {
                if let Expr::Attribute(a) = e {
                    let sys = matches!(&*a.value, Expr::Name(n) if &*n.id == "sys");
                    found |= matches!(
                        a.attr.as_str(),
                        "_getframe" | "f_globals" | "__dict__" | "func_globals"
                    ) || (sys && a.attr.as_str() == "modules");
                }
            });
            found
        }
}

/// Whether `e` may be a module object: a chain of attributes on a name
/// bound by an import or bound nowhere in the module (a library package).
fn may_be_module(model: &SemanticModel<'_>, e: &Expr) -> bool {
    let mut root = e;
    while let Expr::Attribute(a) = root {
        root = &a.value;
    }
    let Expr::Name(n) = root else { return false };
    let Some(r) = model.reference_at(n.range) else {
        return true;
    };
    r.bindings.is_empty()
        || r.bindings.iter().any(|b| {
            matches!(
                model.binding(*b).kind,
                BindingKind::Import | BindingKind::FromImport
            )
        })
}

/// Python standard library modules of Jython 2.7 whose import has no
/// effect outside the module (verified on Ignition 8.3's Jython: no
/// threads, loggers, handlers or output). Excluded on purpose: `socket`
/// and everything that imports it (it starts Netty event loops), `logging`
/// (root logger, `atexit` handler), `signal`, `subprocess`, `platform`,
/// `uuid`, `this`, `antigravity`, `site`, `user`.
const PURE_STDLIB_MODULES: &[&str] = &[
    "HTMLParser",
    "Queue",
    "StringIO",
    "UserDict",
    "UserList",
    "abc",
    "array",
    "base64",
    "binascii",
    "bisect",
    "cPickle",
    "cStringIO",
    "calendar",
    "cgi",
    "cmath",
    "codecs",
    "collections",
    "contextlib",
    "copy",
    "copy_reg",
    "csv",
    "datetime",
    "decimal",
    "difflib",
    "errno",
    "fnmatch",
    "fractions",
    "functools",
    "getopt",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "htmlentitydefs",
    "inspect",
    "itertools",
    "keyword",
    "math",
    "md5",
    "numbers",
    "operator",
    "os",
    "pickle",
    "posixpath",
    "pprint",
    "random",
    "re",
    "repr",
    "sets",
    "sha",
    "shlex",
    "shutil",
    "stat",
    "string",
    "struct",
    "sys",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "traceback",
    "types",
    "unicodedata",
    "urlparse",
    "warnings",
    "weakref",
    "zipfile",
    "zlib",
];

/// Standard library packages whose plain `import` has no effect outside
/// the package. `from pkg import name` may load a submodule (and make it
/// an attribute of the package for every other script), so it is not
/// listed.
const PURE_STDLIB_PACKAGES: &[&str] = &["json", "xml"];

/// Whether removing the import of `binding` cannot change what the
/// program does: Java classes of the JDK (`java.*`, `javax.*`), and the
/// standard library modules above when no library package of the project
/// (or of a project inheriting from it) has the same name. Importing
/// anything else runs code (project library modules and third-party
/// packages execute their top level, other Java classes run static
/// initializers) or, for dotted imports, makes the submodule an attribute
/// of its package for every other script.
fn removal_is_pure(cx: &Checker<'_, '_>, model: &SemanticModel<'_>, binding: BindingId) -> bool {
    let b = model.binding(binding);
    let Some(stmt) = b.stmt else { return false };
    let (module, from) = match stmt {
        Stmt::Import(_) => match b.alias {
            Some(a) => (a.name.as_str(), false),
            None => return false,
        },
        Stmt::ImportFrom(s) if s.level == 0 => match &s.module {
            Some(m) => (m.as_str(), true),
            None => return false,
        },
        _ => return false,
    };
    let top = module.split('.').next().unwrap_or(module);
    let shadowed = || match cx.project_index() {
        Some((index, project)) => {
            let (globals, complete) = index.script_library_globals(project);
            !complete || globals.contains(&top)
        }
        None => true,
    };
    if top == "java" || top == "javax" {
        return !shadowed();
    }
    let pure = if module.contains('.') {
        false
    } else if from {
        PURE_STDLIB_MODULES.contains(&module)
    } else {
        PURE_STDLIB_MODULES.contains(&module) || PURE_STDLIB_PACKAGES.contains(&module)
    };
    pure && !shadowed()
}

impl ScriptRule for UnusedImport {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let library = unit.script.kind == ScriptKind::LibraryModule;
        let text = source(unit);
        // A library name the module calls may be `eval` or `locals` under
        // another name: it reads the caller's names, imports included.
        let mut frames = None;
        for (stmt, mut unused) in unused_imports(model, library) {
            // Top-level names of a library module are its attributes: other
            // resources may use them as `pkg.mod.name`.
            let mut applicability = if unused.iter().all(|id| removal_is_pure(cx, model, *id))
                && !*frames.get_or_insert_with(|| {
                    crate::rules::view::unused_custom_property::library_names_may_read_frames(
                        cx,
                        &unit.parsed.module.body,
                    )
                }) {
                Applicability::Safe
            } else {
                Applicability::Unsafe
            };
            if library {
                unused.retain(|id| {
                    let b = model.binding(*id);
                    if b.scope != ScopeId::MODULE {
                        return true;
                    }
                    // `pkg.mod.name` in the module itself (textual, so
                    // only ever a reason not to report).
                    let dotted = unit.script.node_path.replace('/', ".");
                    if text.contains(&format!("{dotted}.{}", b.name)) {
                        return false;
                    }
                    // `from pkg.mod import *` elsewhere binds the name there.
                    if star_imported(cx) && exported_to_star_imports(model, unit, b.name) {
                        return false;
                    }
                    match external_use(cx, b.name) {
                        ExternalUse::Used => false,
                        ExternalUse::Unused => true,
                        ExternalUse::Unknown => {
                            applicability = Applicability::Unsafe;
                            true
                        }
                    }
                });
                if unused.is_empty() {
                    continue;
                }
            }
            let names: Vec<&str> = unused
                .iter()
                .filter_map(|id| model.binding(*id).alias.map(|a| a.range().slice(text)))
                .collect();
            let message = if names.len() == 1 {
                format!("Remove unused import `{}`", names[0])
            } else {
                format!(
                    "Remove unused imports {}",
                    names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let edit = removal(model, text, stmt, &unused);
            let fix = edit.and_then(|e| statement_fix(cx, unit, message, applicability, e));
            for id in &unused {
                let b = model.binding(*id);
                // The bound name, with the module when it differs
                // (`import json as j`).
                let shown = match b.alias {
                    Some(a) if a.asname.is_some() => format!("{} as {}", a.name.as_str(), b.name),
                    Some(a) => a.name.as_str().to_owned(),
                    None => b.name.to_owned(),
                };
                let r = report(cx, unit, b.range, format!("`{shown}` imported but unused"));
                let _ = r.fix_opt(fix.clone());
            }
        }
    }
}

/// Whether a resource of the project star-imports the library module being
/// checked.
fn star_imported(cx: &Checker<'_, '_>) -> bool {
    match (cx.project_index(), cx.resource()) {
        (Some((index, project)), Some(resource)) => {
            index.module_star_imported(project, resource.id)
        }
        // Unknown context: `external_use` makes the fix unsafe.
        _ => false,
    }
}

/// Whether `from module import *` binds `name` in the importer (Python
/// 2.7): every top-level name not starting with `_`, or only the names of
/// `__all__` when the module assigns `__all__` once in its own statement
/// list (names listed in `__all__` are never reported).
fn exported_to_star_imports(
    model: &SemanticModel<'_>,
    unit: &ScriptUnit<'_, '_>,
    name: &str,
) -> bool {
    let all = model.scope(ScopeId::MODULE).bindings_of("__all__");
    let unconditional_all = match all {
        [only] => model.binding(*only).stmt.is_some_and(|stmt| {
            unit.parsed
                .module
                .body
                .iter()
                .any(|s| std::ptr::eq(s, stmt))
        }),
        _ => false,
    };
    if unconditional_all && !model.dunder_all_is_dynamic() {
        model.dunder_all().iter().any(|n| n == name)
    } else {
        !name.starts_with('_')
    }
}

/// The edit that removes the unused aliases of an import statement.
fn removal(
    model: &SemanticModel<'_>,
    text: &str,
    stmt: &Stmt,
    unused: &[BindingId],
) -> Option<StatementEdit> {
    let aliases = match stmt {
        Stmt::Import(StmtImport { names, .. }) => names,
        Stmt::ImportFrom(StmtImportFrom { names, .. }) => names,
        _ => return None,
    };
    let unused_aliases: Vec<_> = unused
        .iter()
        .filter_map(|id| model.binding(*id).alias)
        .collect();
    let kept: Vec<&str> = aliases
        .iter()
        .filter(|a| !unused_aliases.iter().any(|u| std::ptr::eq(*u, *a)))
        .map(|a| a.range().slice(text))
        .collect();
    let range = stmt.range();
    if range.slice(text).contains('#') || range.slice(text).contains('\\') {
        return None;
    }
    if kept.is_empty() {
        let body = containing_body(&model.module.body, stmt)?;
        return delete_statement(text, body, stmt);
    }
    let new = match stmt {
        Stmt::Import(_) => format!("import {}", kept.join(", ")),
        Stmt::ImportFrom(s) => {
            let mut module = ".".repeat(s.level as usize);
            if let Some(m) = &s.module {
                module.push_str(&m.id);
            }
            format!("from {module} import {}", kept.join(", "))
        }
        _ => return None,
    };
    Some(StatementEdit::replace(range, new))
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_unused_imports() {
        let d = lint_module("SCR102", "import os\nimport json\nprint json.dumps({})\n");
        assert_eq!(flagged(&d), ["os"]);
        assert_eq!(messages(&d), ["`os` imported but unused"]);
    }

    #[test]
    fn usage_is_flow_sensitive_and_by_name_for_closures() {
        assert_eq!(
            flagged(&lint_module("SCR102", "import os\nos = 1\nprint os\n")),
            ["os"]
        );
        assert!(lint_module("SCR102", "import os\ndef f():\n    return os.sep\n").is_empty());
        assert!(lint_module("SCR102", "import os.path\nimport os.sep\nprint os.path\n").is_empty());
    }

    #[test]
    fn exemptions() {
        assert!(lint_module("SCR102", "from __future__ import division\n").is_empty());
        assert!(lint_module("SCR102", "import os\n__all__ = ['os']\n").is_empty());
        assert!(
            lint_module(
                "SCR102",
                "try:\n    import json\nexcept ImportError:\n    json = None\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR102", "def f():\n    import os\n    return locals()\n").is_empty());
        // Re-exports from library modules.
        assert!(lint_module("SCR102", "from core.util import helper\n").is_empty());
        // ... but not from other scripts.
        let d =
            view(ViewScript::Action, "\tfrom java.lang import Thread\n\tpass").lint(&["SCR102"]);
        assert_eq!(decoded_flagged(&d), ["Thread"]);
    }

    /// A module that reaches a frame (reflection import, introspection
    /// string, `sys.exc_info`) may read its imports by name out of the
    /// frame's locals, so no import is reported. (Reviewer repro R5-C.)
    #[test]
    fn frame_access_keeps_imports() {
        let code = "from org.python.core import Py\ndef f():\n    from java.lang import System\n    return 'System' in getattr(Py.getFrame(), 'f_locals')\ndef g():\n    import json\n    try:\n        raise ValueError()\n    except ValueError:\n        tb = sys.exc_info()[2]\n    return getattr(getattr(tb, 'tb_frame'), 'f_locals')['json'].dumps(1)\n";
        let f = library("m", code);
        assert!(
            f.lint(&["SCR102"]).is_empty(),
            "{:?}",
            flagged(&f.lint(&["SCR102"]))
        );
        // Each route on its own keeps a module-level import.
        for frame in [
            "from org.python.core import Py\n",
            "def h():\n    return getattr(x, 'f_locals')\n",
            "def h():\n    return x.tb_lineno\n",
        ] {
            let f = library("m", &format!("import json\n{frame}"));
            assert!(f.lint(&["SCR102"]).is_empty(), "{frame}");
        }
    }

    #[test]
    fn fix_removes_names_or_statements() {
        let f = library("m", "import os, json\nimport sys\nprint json\n");
        let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
        assert_eq!(fixed, "import json\nprint json\n");
        let f = library("m", "def f():\n    import os\n");
        let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
        assert_eq!(fixed, "def f():\n    pass\n");
    }

    #[test]
    fn fix_inside_view_json() {
        let f = view(
            ViewScript::Action,
            "\timport os, re\n\tx \u{3d} re.compile('a')\n\tself.props.text \u{3d} x",
        );
        let fixed = f.fixed(&[("SCR102", "")], false, &view_path(&f));
        assert!(
            fixed.contains("\"script\": \"\\timport re\\n\\tx \\u003d re.compile(\\u0027a\\u0027)"),
            "{fixed}"
        );
    }

    /// Every deletion alone leaves its suite non-empty; together they must
    /// not empty it (the last statement becomes `pass`).
    #[test]
    fn removing_every_statement_of_a_suite_leaves_pass() {
        let cases = [
            (
                "def f():\n    import os\n    import sys\n",
                "def f():\n    pass\n",
            ),
            (
                "def f(x):\n    if x:\n        import os\n        import sys\n    else:\n        import re\n        import json\n    return x\n",
                "def f(x):\n    if x:\n        pass\n    else:\n        pass\n    return x\n",
            ),
            (
                "def f():\n    try:\n        import os\n        import sys\n    except ValueError:\n        import re\n        import json\n    finally:\n        import glob\n        import math\n",
                "def f():\n    try:\n        pass\n    except ValueError:\n        pass\n    finally:\n        pass\n",
            ),
            (
                "def f(xs, l):\n    for x in xs:\n        import os\n        import sys\n    else:\n        import re\n        import json\n    while xs:\n        import glob\n        import math\n    with l:\n        import time\n        import string\n",
                "def f(xs, l):\n    for x in xs:\n        pass\n    else:\n        pass\n    while xs:\n        pass\n    with l:\n        pass\n",
            ),
            (
                "def f():\n    class C:\n        import os\n        import sys\n    def g():\n        import re\n        import json\n        import glob\n    return C, g\n",
                // Class bodies are left alone: their names are attributes.
                "def f():\n    class C:\n        import os\n        import sys\n    def g():\n        pass\n    return C, g\n",
            ),
        ];
        for (code, expected) in cases {
            let f = library("m", code);
            let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
            assert_eq!(fixed, expected, "{code}");
            let result = f.run(
                &[("SCR001", ""), ("SCR002", "")],
                crate::engine::FixMode::None,
            );
            assert!(result.diagnostics.is_empty(), "{code}");
        }
    }

    #[test]
    fn removing_every_statement_of_a_view_script_leaves_pass() {
        let f = view(ViewScript::Action, "\timport os\n\timport sys\n\timport re");
        let fixed = f.fixed(&[("SCR102", "")], false, &view_path(&f));
        assert!(fixed.contains(r#""script": "\tpass""#), "{fixed}");
        let f = view(
            ViewScript::Method,
            "\tif a:\n\t\timport os\n\t\timport sys\n\treturn b",
        );
        let fixed = f.fixed(&[("SCR102", "")], false, &view_path(&f));
        assert!(
            fixed.contains(r#""script": "\tif a:\n\t\tpass\n\treturn b""#),
            "{fixed}"
        );
    }

    /// `pkg.mod.name` in another resource uses a top-level name of a
    /// library module, imports included.
    #[test]
    fn library_imports_used_by_other_modules_are_used() {
        let f = with_library(
            library("sem/unusedimp", "import os\nimport sys\n"),
            "sem/user",
            "def f():\n    return sem.unusedimp.os.getcwd()\n",
        );
        let d = f.lint(&["SCR102"]);
        assert_eq!(flagged(&d), ["sys"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Safe)
        );
        let fixed = f.fixed(
            &[("SCR102", ""), ("REF002", "")],
            false,
            &library_path(&f, "sem/unusedimp"),
        );
        assert_eq!(fixed, "import os\n");
    }

    /// `from pkg.mod import *` binds every name the module exports in the
    /// importer (Jython: `os`, `json` and `sep` are bound, `_sys` is not;
    /// with `__all__` only its names are).
    #[test]
    fn library_imports_star_imported_by_other_modules_are_used() {
        let user = "from sem.unusedimp import *\ndef g():\n    return os.sep + json.dumps(1)\n";
        let f = with_library(
            library(
                "sem/unusedimp",
                "import os\nimport json\nimport sys as _sys\nfrom os import path as _p, sep\ndef f():\n    return 1\n",
            ),
            "sem/user",
            user,
        );
        let d = f.lint(&["SCR102"]);
        // Not exported: still reported, unsafe because the module escapes
        // (`from` imports of library modules are never reported).
        assert_eq!(flagged(&d), ["_sys"]);
        assert_eq!(
            applicability(&d),
            [Some(crate::diagnostic::Applicability::Unsafe)]
        );
        // `__all__` decides what is exported.
        let f = with_library(
            library(
                "sem/unusedimp",
                "import os\nimport json\n__all__ = ['json']\n",
            ),
            "sem/user",
            user,
        );
        assert_eq!(flagged(&f.lint(&["SCR102"])), ["os"]);
        // An `__all__` that may not be assigned does not.
        let f = with_library(
            library(
                "sem/unusedimp",
                "import os\nimport json\nif os.sep == '/':\n    __all__ = ['json']\n",
            ),
            "sem/user",
            user,
        );
        assert!(f.lint(&["SCR102"]).is_empty());
    }

    /// When the module is used as a value, any of its names may be used:
    /// still reported, but the fix is unsafe.
    #[test]
    fn library_imports_of_escaping_modules_have_unsafe_fixes() {
        let f = with_library(
            library("sem/unusedimp", "import os\n"),
            "sem/user",
            "def f(name):\n    return getattr(sem.unusedimp, name)\n",
        );
        let d = f.lint(&["SCR102"]);
        assert_eq!(flagged(&d), ["os"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Unsafe)
        );
        // Function-level imports are not attributes of the module.
        let f = with_library(
            library("sem/unusedimp", "def g():\n    import os\n"),
            "sem/user",
            "def f(name):\n    return getattr(sem.unusedimp, name)\n",
        );
        let d = f.lint(&["SCR102"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Safe)
        );
    }

    fn applicability(d: &[crate::Diagnostic]) -> Vec<Option<crate::diagnostic::Applicability>> {
        d.iter()
            .map(|d| d.fix.as_ref().map(|f| f.applicability))
            .collect()
    }

    /// Names bound in a class body are class attributes (`self.os`,
    /// `C.os`), used in ways no reference shows. Jython: removing them
    /// raises AttributeError.
    #[test]
    fn class_body_imports_are_attributes() {
        let code = "class C(object):\n    import os\n    from java.lang import System\n    def f(self):\n        return self.os.getcwd(), self.System.currentTimeMillis()\nclass D:\n    import json as j\ndef use():\n    return D.j.dumps({})\nclass E:\n    import os\n\nprint E.os.name\n";
        let f = library("m", code);
        assert!(f.lint(&["SCR102"]).is_empty());
    }

    #[test]
    fn message_names_the_binding() {
        let d = lint_module(
            "SCR102",
            "def f():\n    import json as j\n    from os import path as p\n    import os.path\n",
        );
        assert_eq!(
            messages(&d),
            [
                "`json as j` imported but unused",
                "`path as p` imported but unused",
                "`os.path` imported but unused"
            ]
        );
    }

    /// Code that reads the module's globals by name may use any import.
    #[test]
    fn dynamic_global_reads_keep_module_imports() {
        for reader in [
            "def f(n):\n    return globals()[n]\n",
            "def f(n):\n    return eval(n)\n",
            "def f(code):\n    exec code\n",
            "import sys\ndef f(n):\n    return getattr(sys.modules[__name__], n)\n",
            "def f():\n    return f.func_globals\n",
        ] {
            let d = lint_module("SCR102", &format!("import os\n{reader}"));
            assert!(!flagged(&d).contains(&"os".to_owned()), "{reader}: {d:?}");
        }
        // Function imports are still checked.
        let d = lint_module(
            "SCR102",
            "def f(n):\n    import re\n    return globals()[n]\n",
        );
        assert_eq!(flagged(&d), ["re"]);
    }

    /// A frame builtin under another name reads the locals of whatever
    /// scope calls it (verified on Jython: `capture()["os"].name` is
    /// `java`): no import of the module counts as unused.
    #[test]
    fn frame_builtins_under_other_names_keep_imports() {
        for code in [
            "def f():\n    import os\n    from __builtin__ import locals as capture\n    return capture()['os'].name\n",
            "import __builtin__\ndef f():\n    import os\n    return __builtin__.locals()['os']\n",
            "e = eval\ndef f():\n    import os\n    return e('os')\n",
            "def f():\n    import os\n    import sys\n    return sys._getframe(0).f_locals['os']\n",
            "import sys\ndef f():\n    import os\n    return sys.modules['__builtin__'].eval('os')\n",
        ] {
            let d = lint_module("SCR102", code);
            assert!(!flagged(&d).contains(&"os".to_owned()), "{code}: {d:?}");
        }
    }

    /// `eval` or `locals` under a library name reads the caller's names:
    /// the removal is unsafe in a module that calls one, and a module that
    /// reads its own globals with a computed `getattr` uses every import.
    #[test]
    fn library_frame_builtins_and_computed_getattr() {
        let fixes = |code: &str| -> Vec<(String, crate::Applicability)> {
            with_library(
                with_library(library("m", code), "lib/util", "ev = eval\n"),
                "lib/plain",
                "def log(m):\n    pass\n",
            )
            .lint(&["SCR102"])
            .iter()
            .map(|d| (d.message.clone(), d.fix.as_ref().unwrap().applicability))
            .collect()
        };
        assert_eq!(
            fixes("def f():\n    import os\n    return lib.util.ev('os')\n"),
            [(
                "`os` imported but unused".to_owned(),
                crate::Applicability::Unsafe
            )]
        );
        assert_eq!(
            fixes("def f():\n    import os\n    lib.plain.log(1)\n"),
            [(
                "`os` imported but unused".to_owned(),
                crate::Applicability::Safe
            )]
        );
        assert!(
            fixes("import os\nimport m as me\ndef f(n):\n    return getattr(me, n)\n").is_empty()
        );
    }

    /// Removing an import is safe only when importing has no effect
    /// outside the module: JDK classes and listed standard library
    /// modules. Everything else runs code when imported.
    #[test]
    fn imports_with_possible_side_effects_have_unsafe_fixes() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        let cases: &[(&str, crate::diagnostic::Applicability)] = &[
            ("import os", Safe),
            ("import json", Safe),
            ("from java.util import ArrayList", Safe),
            ("import javax.swing", Safe),
            ("from re import compile", Safe),
            // Runs project code or registers things.
            ("import helpers", Unsafe),
            ("from helpers import thing", Unsafe),
            (
                "from com.inductiveautomation.ignition.common import BasicDataset",
                Unsafe,
            ),
            ("import socket", Unsafe),
            ("import logging", Unsafe),
            // Loads a submodule for every other script.
            ("import xml.dom.minidom", Unsafe),
            ("from json import decoder", Unsafe),
            ("from . import sibling", Unsafe),
        ];
        for (import, expected) in cases {
            let f = library("m", &format!("def f():\n    {import}\n    return 1\n"));
            let d = f.lint(&["SCR102"]);
            assert_eq!(applicability(&d), [Some(*expected)], "{import}");
        }
        // A project library package with a standard name shadows it.
        let f = with_library(
            library("m", "def f():\n    import json\n    return 1\n"),
            "json/codec",
            "x = 1\n",
        );
        assert_eq!(applicability(&f.lint(&["SCR102"])), [Some(Unsafe)]);
        // A statement with a pure and an impure name: unsafe.
        let f = library("m", "def f():\n    import os, helpers\n    return 1\n");
        assert_eq!(
            applicability(&f.lint(&["SCR102"])),
            [Some(Unsafe), Some(Unsafe)]
        );
    }
}
