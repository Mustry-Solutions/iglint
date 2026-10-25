//! SCR102 unused-import.

use iglint_project::ScriptKind;
use iglint_python::ast::Expr;
use iglint_python::ast::{Stmt, StmtImport, StmtImportFrom};
use iglint_python_semantic::{BindingId, BindingKind, ScopeId, ScopeKind, SemanticModel};
use iglint_text::Ranged;
use rustc_hash::FxHashMap;

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity, UnsafeReason};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::refs::{ExternalUse, external_use};
use crate::rules::script::{
    Bodies, StatementEdit, delete_statement, report, semantic, source, statement_fix,
};

pub static META: RuleMeta = RuleMeta {
    code: "SCR102",
    name: "unused-import",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    fix_safety: crate::registry::FixSafety::Depends,
    summary: "Imported name is never used",
    explanation: r#"## What it does

Reports imports whose name is never read. Usage is flow-sensitive: an
import that is rebound before any read is unused even if the name is read
later.

Reads are resolved the way Jython compiles them: a class body reads a
name it binds from the class namespace and, before the class binds it,
from the module (`x = m.pi` before `m = 1` in the class reads the
module's `m`); private names in class bodies are mangled (`__m` in class
`D` reads `_D__m`); and the names the interpreter reads by itself count:
`__metaclass__` for a class without bases, `AssertionError` for `assert`,
`__name__` for class and function definitions.

Not reported:

* `from __future__ import ...`;
* names listed in the module's `__all__` (and every module-level import when
  `__all__` is built dynamically);
* imports inside a `try` whose handlers catch `ImportError` (availability
  checks such as `try: import json except ImportError: json = None`);
* scopes that read their names dynamically (`locals()`, `vars()`, `eval`,
  `exec`), every scope of a module with code that reads or writes a
  frame's or a module's names by itself (`sys._getframe()`, `.f_locals`,
  `.tb_frame`, `e = eval`, `import __builtin__`; the table is in
  `docs/internal/fix-safety.md`, "Frame routes: findings and fixes"), and
  module-level imports when any code of the module may read globals by
  name (`globals()`, `eval`, `execfile`, `exec`, `sys.modules`, frames,
  `__dict__`, `func_globals`). Reflection or introspection that is only a
  first hop (a Java member that leads to reflection such as `.type`,
  `getattr` with a computed name, `sys.exc_info()`, line numbers) does
  not silence the rule; it makes the fix unsafe;
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
last one becomes `pass`). The safe fix empties the statement's lines and
keeps them, so no line number changes; the unsafe fix removes the lines.

Safe only when all of these hold:

* the import is known to succeed and to do nothing outside the importing
  module, so that removing it cannot change what runs: a Java class or
  package of the vetted JDK packages that was measured to exist in the
  Ignition image, or a plain `import x` of a standard library module that
  is known to have no import-time effects in Jython 2.7 (`os`, `re`,
  `json`, `datetime`, `math`, `time`, ...) and was measured to import, or
  `from x import y` of such a module when it is not a package and `y` was
  measured to exist; and no library package of the project (or of a
  project inheriting from it) has the same top-level name;
* the module has no reflection or introspection route of any kind
  (`iglint_python_semantic::dynamic`: the routes that silence the rule,
  and the first hops that do not, such as `.type` or `getattr(o, name)`):
  code reached through one may read the import by name. The reason names
  the route;
* the module uses only vetted code: every module, Java class, builtin
  and module attribute it uses is on iglint's allowlist, and every
  library module it imports or calls is vetted too (see
  `docs/internal/fix-safety.md`). Anything else may read the module's
  names in ways iglint does not know (`cPickle.loads` can call
  `globals()` in the caller's frame);
* the module calls no library name that is not a plain function, class
  or literal constant (it may be `eval` or `locals` under another name);
* the import is not the only binding of a name that is still read in its
  scope or a scope nested in it (`print len(x)` before `import math as
  len` in a function raises `UnboundLocalError`; without the import it
  calls the builtin);
* the import does not come before a `from __future__` import;
* when the whole statement goes from a library module, nothing may read
  `__package__`: there every import that runs binds it (to `None`, in
  Ignition 8.3), so removing the only one turns a read into a
  `NameError`. A read or binding of `__package__` or
  `__warningregistry__` in the module, a binding of `__name__`, code that
  reads the module's globals by name, or another resource that reads
  them, reads the module's `__dict__` or uses the module as a value makes
  the fix unsafe. (Event scripts run without `__name__`: their imports
  bind nothing.)

Unsafe otherwise: importing a project library module runs its top level,
other Java classes run static initializers, some standard modules start
threads (`socket`) or configure logging (`logging`), and a dotted import
makes the submodule an attribute of its package for every other script.
Also unsafe for top-level imports of library modules when iglint cannot
see every use of the module's names (the module is used as a value,
references are computed at run time, a resource of the project uses code
that is not vetted, the project is inheritable or has scripts iglint does
not model, or the file is linted outside its project), and for an import
that may fail (a Java class or module attribute not measured to exist):
removing it turns a failing import into a successful one.

Like every fix, it is unsafe while code in the workspace may read the
files a fix edits (any file access, read, write, rename or delete, whose path
is not a literal outside the
workspace, or code that is not vetted); the project setting `[fix]
assume-scripts-do-not-read-workspace-files = true` declares that no script
does (see `docs/internal/fix-safety.md`, "File reads").

Top-level names of a library module are attributes of the module: an
import that another resource uses as `pkg.mod.name` is not reported (also
through another library module's import: `m0.util.name` when `m0` does
`from pkg import util`, at any depth; a module that another module's name
may hold, `try: from pkg import util` / `except: util = None`, counts as
used as a value, and so does every library module that a module used as a
value imports), nor
one that `from pkg.mod import *` in another resource binds there (every
name not starting with `_`, or the names of `__all__`; any name when
`__all__` is changed in a way not read, such as `__all__[:] = [...]`,
`__all__[1] = 'json'`, `__all__.insert(...)`, `__all__ += [...]`, an
alias of it or a change from a function, and when its one binding is not
an assignment to the bare name: `from x import __all__`, `for __all__ in
...`, `with c as __all__`, `__all__, y = ...`), nor one that another resource
deletes (`del pkg.mod.name`: without the binding, a `KeyError` on 8.3.3)
or changes with an augmented assignment (`pkg.mod.name += 1` reads it
first), also through aliases and other modules' imports. An assignment
from another resource (`pkg.mod.name = 5`, also as a `for`, `except` or
tuple target) or from a function of the module through `global`
replaces what the import binds: the import is reported, but its fix is
unsafe. A resource that
reaches the module object other than by its names, with a special
attribute (`pkg.mod.__dict__`, `pkg.mod.__findattr_ex__(n)`,
`pkg.mod.__getattribute__('name')` counts as `pkg.mod.name`) or a member
of Ignition's module object (`pkg.mod.dict`, `pkg.mod.getDict()`), uses
the module as a value: none of its imports is reported (see
`docs/internal/fix-safety.md`, "Library module objects"). While a script
of the project does not parse, what it will use is not known: the fix of
a top-level import of a library module is unsafe.

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
/// program does: it needs positive evidence that the import succeeds and
/// has no effect. Java classes measured to exist in the image (public
/// top-level classes of the vetted Java packages and of the packages of
/// the vetted classes, `iglint_python_semantic::vetted::java_class_exists`)
/// and those packages; the standard library modules above that were
/// measured to import in the image, and for `from m import n` a name `n`
/// measured to be an attribute of `m`; in both cases only when no library
/// package of the project (or of a project inheriting from it) has the
/// same name. Importing anything else may fail (a removed import that
/// failed changes what the script does, even when the caller catches the
/// error), runs code (project library modules and third-party packages
/// execute their top level, other Java classes run static initializers)
/// or, for dotted imports, makes the submodule an attribute of its package
/// for every other script.
fn removal_is_pure(cx: &Checker<'_, '_>, model: &SemanticModel<'_>, binding: BindingId) -> bool {
    use iglint_python_semantic::vetted::{
        java_class_exists, java_package_exists, module_has_attribute, module_imports,
    };
    let b = model.binding(binding);
    let Some(stmt) = b.stmt else { return false };
    let Some(alias) = b.alias else { return false };
    let (module, from) = match stmt {
        Stmt::Import(_) => (alias.name.as_str(), false),
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
        // The full name of what is imported: a class or a package.
        let full = if from {
            format!("{module}.{}", alias.name.as_str())
        } else {
            module.to_owned()
        };
        let exists = java_package_exists(&full)
            || full
                .rsplit_once('.')
                .is_some_and(|(package, class)| java_class_exists(package, class));
        return exists && !shadowed();
    }
    let pure = if module.contains('.') {
        false
    } else if from {
        PURE_STDLIB_MODULES.contains(&module)
            && module_imports(module)
            && module_has_attribute(module, alias.name.as_str())
    } else {
        (PURE_STDLIB_MODULES.contains(&module) || PURE_STDLIB_PACKAGES.contains(&module))
            && module_imports(module)
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
        // another name: it reads the caller's names, imports included. Code
        // that is not vetted may read them in ways iglint does not know.
        let mut frames: Option<Option<UnsafeReason>> = None;
        let bodies = std::cell::OnceCell::new();
        // Where the last `from __future__` import of the module starts.
        let last_future = last_future_import(&unit.parsed.module.body);
        for (stmt, mut unused) in unused_imports(model, library) {
            // Top-level names of a library module are its attributes: other
            // resources may use them as `pkg.mod.name`.
            let impure = !unused.iter().all(|id| removal_is_pure(cx, model, *id));
            let referenced = unused.iter().any(|id| still_referenced(model, *id));
            let before_future = last_future.is_some_and(|f| stmt.range().start() < f);
            let plain = !impure && !referenced && !before_future;
            let mut reason = if impure {
                Some(UnsafeReason::new(IMPURE_REASON))
            } else if referenced {
                Some(UnsafeReason::new(
                    "the name is read in the import's scope, so without the import the read \
                     finds another binding (a global or a builtin) instead of raising",
                ))
            } else if before_future {
                Some(UnsafeReason::new(
                    "the import comes before a `from __future__` import, which removing it may \
                     make valid, changing how the script compiles",
                ))
            } else {
                frames
                    .get_or_insert_with(|| {
                        let body = &unit.parsed.module.body;
                        // A route that does not silence the finding
                        // (`o.type`, a computed `getattr`) still keeps the
                        // fix: names may be read through it.
                        if let Some(at) = model.frame_escape() {
                            return Some(crate::rules::vetting::escape_reason(
                                cx,
                                unit.file,
                                unit.script,
                                at,
                            ));
                        }
                        if crate::rules::view::unused_custom_property::library_names_may_read_frames(
                            cx, model, body,
                        ) {
                            return Some(UnsafeReason {
                                message: "the script calls a library name that may read frames"
                                    .to_owned(),
                                site: None,
                                workspace: false,
                                store: None,
                            });
                        }
                        if let Some(at) = crate::rules::vetting::unknown_code_at(cx, model, body) {
                            return Some(crate::rules::vetting::script_reason(
                                cx,
                                unit.file,
                                unit.script,
                                at,
                            ));
                        }
                        // Closed world: a frame reader anywhere in the shared
                        // runtime can observe this import, even a
                        // function-local one through a callback or
                        // `sys._current_frames`.
                        crate::rules::vetting::workspace_has_unknown_code(cx)
                            .then(|| crate::rules::vetting::workspace_reason(cx))
                    })
                    .clone()
            };
            let mut applicability = if plain && reason.is_none() {
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
                    // A function replaces the module attribute through
                    // `global`, like an assignment from another resource.
                    if let Some(other) = model
                        .scope(ScopeId::MODULE)
                        .bindings_of(b.name)
                        .iter()
                        .find(|o| model.binding(**o).site != ScopeId::MODULE)
                    {
                        applicability = Applicability::Unsafe;
                        if reason.is_none() {
                            // Named at the binding in the function
                            // (review round 30, Opus nit 3).
                            let mut r = crate::rules::vetting::script_reason(
                                cx,
                                unit.file,
                                unit.script,
                                crate::rules::vetting::UnknownAt::Here(model.binding(*other).range),
                            );
                            r.message = STORED_REASON.to_owned();
                            reason = Some(r);
                        }
                    }
                    match external_use(cx, b.name) {
                        ExternalUse::Used => false,
                        ExternalUse::Unused => {
                            // Another resource replaces the binding
                            // (`lib.util.json = 5`, review round 29).
                            if crate::rules::refs::name_stored_elsewhere(cx, b.name) {
                                applicability = Applicability::Unsafe;
                                if reason.is_none() {
                                    // Named at the first store, filled in
                                    // after the cache (review round 30,
                                    // Opus nit 3).
                                    let mut r = UnsafeReason::new(STORED_REASON);
                                    r.store = cx.resource().map(|res| {
                                        format!("{}.{}", res.path.replace('/', "."), b.name)
                                    });
                                    reason = Some(r);
                                }
                            }
                            true
                        }
                        ExternalUse::Unknown => {
                            applicability = Applicability::Unsafe;
                            if reason.is_none() {
                                reason = Some(UnsafeReason::new(EXTERNAL_USE_REASON));
                            }
                            true
                        }
                    }
                });
                if unused.is_empty() {
                    continue;
                }
            }
            // Removing the whole statement removes the `__package__` it
            // binds when it runs.
            if applicability == Applicability::Safe
                && removes_statement(stmt, &unused)
                && binds_package(model, stmt)
                && let Some(message) = implicit_name_observed(cx, model, library)
            {
                applicability = Applicability::Unsafe;
                reason = Some(UnsafeReason {
                    message,
                    site: None,
                    workspace: false,
                    store: None,
                });
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
            let keep_lines = applicability == Applicability::Safe;
            let bodies = bodies.get_or_init(|| Bodies::of(&unit.parsed.module.body));
            let edit = removal(model, bodies, text, stmt, &unused, keep_lines);
            let fix = edit
                .and_then(|e| statement_fix(cx, unit, message, applicability, e))
                .map(|f| f.because(reason));
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

/// Why removing an import that is not known to be free of effects is
/// unsafe.
const IMPURE_REASON: &str = "the import is not known to succeed without side effects: only \
     standard modules and Java classes measured in the Ignition image are, and only when no \
     library package has their name";

/// Why removing a top-level import of a library module is unsafe when other
/// resources may use the name.
const EXTERNAL_USE_REASON: &str = "other resources may use the name as an attribute of the \
     module: library references are computed at run time, the project is inheritable or has an \
     incomplete parent chain, or the workspace has scripts iglint does not model";

/// Why removing a top-level import of a library module is unsafe when code
/// assigns the name.
const STORED_REASON: &str = "code assigns the name as an attribute of the module, replacing \
     what the import binds, or another scope assigns it through `global`";

/// Whether `unused` holds every alias of the import statement `stmt`.
fn removes_statement(stmt: &Stmt, unused: &[BindingId]) -> bool {
    let aliases = match stmt {
        Stmt::Import(StmtImport { names, .. }) => names.len(),
        Stmt::ImportFrom(StmtImportFrom { names, .. }) => names.len(),
        _ => return false,
    };
    unused.len() >= aliases
}

/// Whether the import statement `stmt` binds `__package__` when it runs
/// (see [`BindingKind::ImportPackage`]).
fn binds_package(model: &SemanticModel<'_>, stmt: &Stmt) -> bool {
    model.bindings().any(|(_, b)| {
        b.kind == BindingKind::ImportPackage && b.stmt.is_some_and(|s| std::ptr::eq(s, stmt))
    })
}

/// The names an import creates in the module's globals without naming
/// them: `__package__` always (measured, see `BindingKind::ImportPackage`),
/// `__warningregistry__` when the import warns (a relative import in an
/// event script, any import once the code set a dotted `__name__` or
/// `__package__`).
const IMPLICIT_IMPORT_NAMES: &[&str] = &["__package__", "__warningregistry__"];

/// Why code may observe a name an import creates implicitly, if it may:
/// the module reads it (any scope), binds it or `__name__` (which decides
/// what an import binds), reads its globals by name, or (for a library
/// module) another resource may read it as a module attribute, through
/// `__dict__`, or with the module as a value. Fails closed: an unknown
/// project counts as a reader.
fn implicit_name_observed(
    cx: &Checker<'_, '_>,
    model: &SemanticModel<'_>,
    library: bool,
) -> Option<String> {
    let read = |name: &str| format!("the import binds `{name}`, which code may read");
    let bound = |name: &str| {
        model
            .bindings()
            .any(|(_, b)| b.name == name && !b.is_implicit())
    };
    for name in IMPLICIT_IMPORT_NAMES {
        // Import statements read `__package__` themselves (an implicit
        // read): only the code's own reads count here.
        if model
            .references()
            .any(|(_, r)| r.name == *name && !r.is_implicit())
            || bound(name)
        {
            return Some(read(name));
        }
    }
    if bound("__name__") {
        return Some("the script binds `__name__`, which decides what an import binds".to_owned());
    }
    if reads_globals_dynamically(model) {
        return Some(read("__package__"));
    }
    if library {
        for name in ["__package__", "__warningregistry__", "__dict__"] {
            if external_use(cx, name) != ExternalUse::Unused {
                let name = if name == "__dict__" {
                    "__package__"
                } else {
                    name
                };
                return Some(read(name));
            }
        }
    }
    None
}

/// Whether removing the import binding `id` may change what another
/// reference of its name resolves to: it is the only binding of the name
/// in its scope, and the name is read in that scope or a scope nested in
/// it. (A read before the import raises `UnboundLocalError` in a function;
/// without the import the name is a global or a builtin.)
fn still_referenced(model: &SemanticModel<'_>, id: BindingId) -> bool {
    let b = model.binding(id);
    if model.scope(b.scope).bindings_of(b.name).len() > 1 {
        return false;
    }
    let inside = |mut scope: ScopeId| loop {
        if scope == b.scope {
            return true;
        }
        match model.scope(scope).parent {
            Some(parent) => scope = parent,
            None => return false,
        }
    };
    model
        .references()
        .any(|(_, r)| r.name == b.name && inside(r.scope))
}

/// Where the last `from __future__` import of the module starts: an
/// import before it comes before a future import (deleting it could make
/// a misplaced future import valid). Found once per script, not per
/// import (review round 25, M3).
fn last_future_import(body: &[Stmt]) -> Option<iglint_text::TextSize> {
    body.iter()
        .filter(|s| {
            matches!(s, Stmt::ImportFrom(f)
                if f.module.as_ref().is_some_and(|m| m.as_str() == "__future__"))
        })
        .map(|s| s.range().start())
        .max()
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
    crate::script_facts::symbols::exported_to_star_imports(
        &crate::script_facts::symbols::star_exports(model, &unit.parsed.module),
        name,
    )
}

/// The edit that removes the unused aliases of an import statement.
fn removal(
    model: &SemanticModel<'_>,
    bodies: &Bodies<'_>,
    text: &str,
    stmt: &Stmt,
    unused: &[BindingId],
    keep_lines: bool,
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
        let body = bodies.get(stmt)?;
        return delete_statement(text, body, stmt, keep_lines);
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
        // Each route that observes names on its own keeps a module-level
        // import.
        for frame in [
            "from org.python.core import Py\n",
            "def h():\n    return getattr(x, 'f_locals')\n",
            "def h():\n    return x.tb_frame\n",
        ] {
            let f = library("m", &format!("import json\n{frame}"));
            assert!(f.lint(&["SCR102"]).is_empty(), "{frame}");
        }
        // A route that is only a first hop (a line number, a reflective
        // Java member, a computed attribute name) keeps the finding and
        // makes the fix unsafe, with the route as the site (review round
        // 24).
        for frame in [
            "def h():\n    return x.tb_lineno\n",
            "def h(o):\n    return o.type\n",
            "def h(o, n):\n    return getattr(o, n)\n",
            "def h(o):\n    return o.__class__.__name__\n",
        ] {
            let f = library("m", &format!("import json\n{frame}"));
            let d = f.lint(&["SCR102"]);
            assert_eq!(flagged(&d), ["json"], "{frame}");
            let fix = d[0].fix.as_ref().expect("a fix");
            assert_eq!(
                fix.applicability,
                crate::diagnostic::Applicability::Unsafe,
                "{frame}"
            );
            let reason = fix.unsafe_reason.as_ref().expect("a reason");
            assert!(reason.message.contains("reflection"), "{frame}: {reason:?}");
            assert_eq!(
                reason.site.as_ref().map(|s| s.line),
                Some(3),
                "{frame}: {reason:?}"
            );
        }
    }

    /// A name bound only in a nested scope must not hide a library
    /// reference in the enclosing scope: `lib.util.ev` is still seen even
    /// though a nested `dummy(lib)` binds `lib`, so the module may read
    /// `os` out of the frame and the import removal is unsafe. (Reviewer
    /// repro R5-G1.)
    #[test]
    fn nested_scope_does_not_hide_a_library_reference() {
        let code = "def f():\n    import os\n    def dummy(lib):\n        pass\n    return lib.util.ev(\"os.name\")\n";
        let f = with_library(library("demo/code", code), "lib/util", "ev = eval\n");
        let d = f.lint(&["SCR102"]);
        let os = d
            .iter()
            .find(|d| d.message.contains('`') && d.message.contains("os"))
            .unwrap_or_else(|| panic!("no os finding: {d:?}"));
        assert_eq!(
            os.fix.as_ref().unwrap().applicability,
            crate::Applicability::Unsafe
        );
    }

    /// Removing the import before a bare string would promote the string
    /// to the function's docstring. A `pass` is kept so it does not.
    /// (Reviewer repro R5-G3.)
    #[test]
    fn import_removal_does_not_promote_a_docstring() {
        let f = library(
            "m",
            "def f():\n    import os\n    'promoted docstring'\n    return f.__doc__\n",
        );
        let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "def f():\n    pass\n    'promoted docstring'\n    return f.__doc__\n"
        );
    }

    /// When several deletions together would expose a later string as a
    /// docstring, the post-fix safety net keeps one so `__doc__` does not
    /// change. (Reviewer repro R5-G3, multiple deletions.)
    #[test]
    fn import_removal_keeps_an_import_to_avoid_docstring_promotion() {
        let f = library("m", "import os\nimport sys\n'the docstring'\n");
        let result = f.run(
            &[("SCR102", "")],
            crate::engine::FixMode::DiffOnly {
                unsafe_fixes: false,
            },
        );
        let path = library_path(&f, "m");
        let summary = result.fixed.iter().find(|s| s.path == path).unwrap();
        // One import stays, so the string never becomes the docstring.
        assert!(
            !summary.fixed.trim_start().starts_with('\'')
                && summary.fixed.contains("'the docstring'"),
            "{}",
            summary.fixed
        );
    }

    /// Two function-local imports whose joint removal would promote a
    /// bare string to the function docstring. `--fix` must reach a
    /// fixpoint in one run: a second run changes nothing (reviewer round
    /// 7, finding 3). `Fixture::fixed` asserts idempotency and that no
    /// fixable finding is left.
    #[test]
    fn docstring_promotion_reaches_a_fixpoint_in_one_run() {
        let f = library(
            "m",
            "def f():\n    import math\n    import sys\n    \"doc\"\n    return 1\n",
        );
        let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
        // Both unused imports go; a `pass` takes the place of the first so
        // the string is not promoted to the docstring, and the result is
        // stable (one run reaches it).
        assert_eq!(
            fixed, "def f():\n\n    pass\n    \"doc\"\n    return 1\n",
            "{fixed}"
        );
        assert_eq!(fixed.lines().count(), 5, "no line moves: {fixed}");
    }

    /// Reviewer round 7, finding 1: a module that uses only allowlisted
    /// imports but reaches Java reflection (through a bean property, a
    /// subclass of a Java class, or a reflective member named in a string)
    /// must never have an import removed by a safe fix. The route reads the
    /// module's globals (verified on Jython), so the reflection escape
    /// makes the module read its globals dynamically: the import is not
    /// reported, and no finding carries a safe fix.
    #[test]
    fn java_reflection_blocks_safe_import_fix() {
        use crate::diagnostic::Applicability::Safe;
        for code in [
            "import math\nfrom java.util import ArrayList\nclass Bridge(ArrayList):\n    pass\ndef go():\n    return Bridge().class.classLoader\n",
            "import math\ndef go(o):\n    return getattr(o, 'classLoader')\n",
            "import math\ndef go(o):\n    return o.getClass().getDeclaredMethods()\n",
        ] {
            let f = library("m", code);
            let d = f.lint(&["SCR102"]);
            assert!(
                d.iter()
                    .all(|d| d.fix.as_ref().map(|x| x.applicability) != Some(Safe)),
                "a safe import fix survived reflection:\n{code}\n{d:?}"
            );
            let out = f.run(
                &[("SCR102", "")],
                crate::engine::FixMode::DiffOnly {
                    unsafe_fixes: false,
                },
            );
            if let Some(s) = out.fixed.iter().find(|s| s.path == library_path(&f, "m")) {
                assert!(s.fixed.contains("import math"), "{}", s.fixed);
            }
        }
    }

    /// The language server must not offer (or apply through fix-all) a
    /// safe SCR102 quick fix for an import in a module that reaches Java
    /// reflection through a bean property on a subclass of a Java class
    /// (reviewer round 7, finding 1). Before the fix this module reported
    /// `math` with a safe fix the LSP would apply; now it does not.
    #[test]
    fn lsp_offers_no_safe_quick_fix_through_reflection() {
        let code = "import math\nfrom java.util import ArrayList\nclass B(ArrayList):\n    pass\ndef go():\n    return B().class\n";
        let f = library("m", code);
        let path = library_path(&f, "m");
        let settings = crate::rules::test_support::settings(f.root(), &[("SCR102", "")]);
        let mut live = crate::live::LiveWorkspace::new(settings, vec![f.root().to_path_buf()]);
        // Safe fix-all changes nothing (the import stays).
        assert!(
            live.fix_all(&path, false).is_none(),
            "safe fix-all changed the file"
        );
        // No reported finding yields a safe quick fix the LSP would apply.
        let diags = live
            .check(std::slice::from_ref(&path))
            .remove(0)
            .diagnostics;
        for d in &diags {
            if d.fix.as_ref().map(|x| x.applicability) == Some(crate::Applicability::Safe) {
                assert!(
                    live.fix_one(&path, d).is_none(),
                    "a safe quick fix applied: {d:?}"
                );
            }
        }
    }

    /// Reviewer round 7, H3 (closed world): a frame reader in another
    /// library module makes a function-local import unsafe to remove, even
    /// though the import is not a module attribute and the two modules do
    /// not import each other. Through the shared runtime the reader can
    /// reach the frame (as a callback, or via `sys._current_frames`).
    #[test]
    fn a_frame_reader_elsewhere_makes_a_function_local_import_unsafe() {
        let reader =
            "def peek(n):\n    it = (x for x in [])\n    return it.gi_frame.f_back.f_locals[n]\n";
        let f = with_library(
            library(
                "lib/b",
                "def run(cb):\n    import math\n    return cb('math')\n",
            ),
            "lib/a",
            reader,
        );
        let d = f.lint(&["SCR102"]);
        let math = d.iter().find(|d| d.message.contains("math"));
        if let Some(d) = math {
            assert_eq!(
                d.fix.as_ref().map(|x| x.applicability),
                Some(crate::Applicability::Unsafe),
                "a frame reader elsewhere must make the import unsafe"
            );
        }
        // Remove the reader: the same import is safe to remove.
        let f = library(
            "lib/b",
            "def run(cb):\n    import math\n    return cb('math')\n",
        );
        let d = f.lint(&["SCR102"]);
        let math = d.iter().find(|d| d.message.contains("math")).unwrap();
        assert_eq!(
            math.fix.as_ref().map(|x| x.applicability),
            Some(crate::Applicability::Safe)
        );
    }

    #[test]
    fn fix_removes_names_or_statements() {
        let f = library("m", "import os, json\nimport sys\nprint json.dumps(1)\n");
        let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
        // The safe fix keeps the line of a removed statement.
        assert_eq!(fixed, "import json\n\nprint json.dumps(1)\n");
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
                "def f():\n\n    pass\n",
            ),
            (
                "def f(x):\n    if x:\n        import os\n        import sys\n    else:\n        import re\n        import json\n    return x\n",
                "def f(x):\n    if x:\n\n        pass\n    else:\n\n        pass\n    return x\n",
            ),
            (
                "def f():\n    try:\n        import os\n        import sys\n    except ValueError:\n        import re\n        import json\n    finally:\n        import bisect\n        import math\n",
                "def f():\n    try:\n\n        pass\n    except ValueError:\n\n        pass\n    finally:\n\n        pass\n",
            ),
            (
                "def f(xs, l):\n    for x in xs:\n        import os\n        import sys\n    else:\n        import re\n        import json\n    while xs:\n        import bisect\n        import math\n    with l:\n        import time\n        import string\n",
                "def f(xs, l):\n    for x in xs:\n\n        pass\n    else:\n\n        pass\n    while xs:\n\n        pass\n    with l:\n\n        pass\n",
            ),
            (
                "def f():\n    class C:\n        x = 1\n        y = 2\n    def g():\n        import re\n        import json\n        import bisect\n    return C, g\n",
                "def f():\n    class C:\n        x = 1\n        y = 2\n    def g():\n\n\n        pass\n    return C, g\n",
            ),
        ];
        // Class bodies are left alone: their names are attributes. (An
        // import there is unknown code since review round 23, so the
        // module's other fixes are unsafe.)
        let f = library(
            "m",
            "def f():\n    class C:\n        import os\n        import sys\n    return C\n",
        );
        let result = f.run(&[("SCR102", "")], crate::engine::FixMode::None);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        for (code, expected) in cases {
            let f = library("m", code);
            let fixed = f.fixed(&[("SCR102", "")], false, &library_path(&f, "m"));
            assert_eq!(fixed, expected, "{code}");
            assert_eq!(fixed.lines().count(), code.lines().count(), "{code}");
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
        assert!(fixed.contains(r#""script": "\n\n\tpass""#), "{fixed}");
        let f = view(
            ViewScript::Method,
            "\tif a:\n\t\timport os\n\t\timport sys\n\treturn b",
        );
        let fixed = f.fixed(&[("SCR102", "")], false, &view_path(&f));
        assert!(
            fixed.contains(r#""script": "\tif a:\n\n\t\tpass\n\treturn b""#),
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
        assert_eq!(fixed, "import os\n\n");
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
    /// a top-level import is not reported (reviewer round 9, finding 2).
    #[test]
    fn library_imports_of_escaping_modules_are_used() {
        let f = with_library(
            library("sem/unusedimp", "import os\n"),
            "sem/user",
            "def f(name):\n    return getattr(sem.unusedimp, name)\n",
        );
        let d = f.lint(&["SCR102"]);
        assert!(d.is_empty(), "{d:?}");
        // Function-level imports are not attributes of the module, but the
        // computed `getattr` on the module object can reach reflection
        // (`getattr(mod, 'class')`) and from there any frame, so the
        // closed-world check makes the fix unsafe (reviewer round 7, H3).
        let f = with_library(
            library("sem/unusedimp", "def g():\n    import os\n"),
            "sem/user",
            "def f(name):\n    return getattr(sem.unusedimp, name)\n",
        );
        let d = f.lint(&["SCR102"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Unsafe)
        );
        // Without that dynamic reader anywhere, the function-local import
        // is safe to remove.
        let f = library("sem/unusedimp", "def g():\n    import os\n");
        let d = f.lint(&["SCR102"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Safe)
        );
    }

    /// Review round 28 (blocker): a name read through another library
    /// module's import (`m0.util.json` when `m0` does `from lib import
    /// util`; Ignition's ScriptManager resolves it on 8.3.3) uses the
    /// import of the module the import leads to, whatever the form of the
    /// imports and the length of the chain; `--fix --unsafe-fixes` keeps
    /// it.
    #[test]
    fn imports_read_through_other_library_modules_are_used() {
        let lib = "import json\nimport os\n";
        let mut chain: Vec<(String, String)> = (0..9)
            .map(|i| (format!("c{i}"), format!("import c{} as a\n", i + 1)))
            .collect();
        chain.push(("c9".into(), "from lib import util\n".into()));
        for (modules, user) in [
            (
                vec![("m0", "from lib import util\n")],
                "def u():\n    return m0.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "import lib.util as util\n")],
                "def u():\n    return m0.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "import lib.util\n")],
                "def u():\n    return m0.lib.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "import m1\n"), ("m1", "from lib import util\n")],
                "def u():\n    return m0.m1.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "import m1 as a\n"), ("m1", "from lib import util\n")],
                "def u():\n    return m0.a.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "from lib import util\n")],
                "from m0 import util\ndef u():\n    return util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "from lib import util\n")],
                "def u():\n    from m0.util import json\n    return json.dumps(1)\n",
            ),
            (
                vec![("m0", "from lib import util\n")],
                "def u():\n    import m0.util.json\n",
            ),
            (
                vec![
                    ("m0", "import m1\n"),
                    ("m1", "import m0\nfrom lib import util\n"),
                ],
                "def u():\n    return m0.m1.m0.m1.util.json.dumps(1)\n",
            ),
            (
                vec![
                    ("m0", "from m1 import *\n"),
                    ("m1", "from lib import util\n"),
                ],
                "def u():\n    return m0.util.json.dumps(1)\n",
            ),
            (
                vec![("m0", "from lib import util\n")],
                "from m0 import *\ndef u():\n    return util.json.dumps(1)\n",
            ),
            (
                chain
                    .iter()
                    .map(|(m, c)| (m.as_str(), c.as_str()))
                    .collect(),
                "def u():\n    return c0.a.a.a.a.a.a.a.a.a.util.json.dumps(1)\n",
            ),
        ] {
            let f = modules
                .iter()
                .fold(library("lib/util", lib), |f, (m, c)| with_library(f, m, c));
            let f = with_library(f, "user", user);
            let d: Vec<_> = f
                .lint(&["SCR102"])
                .into_iter()
                .filter(|d| d.file.path().ends_with("lib/util/code.py"))
                .collect();
            assert_eq!(flagged(&d), ["os"], "{modules:?} {user}");
            let fixed = f.fixed(&[("SCR102", "")], true, &library_path(&f, "lib/util"));
            assert!(fixed.contains("import json"), "{modules:?} {user}: {fixed}");
        }
        // From a `runScript` target.
        let f = with_library(library("lib/util", lib), "m0", "from lib import util\n").with_view(
            "Expr",
            r#"{"root": {"type": "ia.display.label", "meta": {"name": "root"}, "props": {"text": ""},
                "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "runScript(\"m0.util.json.dumps\", 0, 1)"}}}}}}"#,
        );
        assert_eq!(flagged(&f.lint(&["SCR102"])), ["os"]);
        // A name bound otherwise too may hold the module: any of its names
        // may be read, nothing is removed by a safe fix.
        let f = with_library(
            library("lib/util", lib),
            "m0",
            "try:\n    from lib import util\nexcept ImportError:\n    util = None\n",
        );
        let f = with_library(f, "user", "def u():\n    return m0.util.json.dumps(1)\n");
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.file.path().ends_with("lib/util/code.py"))
            .collect();
        assert!(d.is_empty(), "{:?}", flagged(&d));
        // Whoever holds a module as a value reaches what it imports.
        let f = with_library(library("lib/two", lib), "lib/util", "import lib.two as x\n");
        let f = with_library(f, "h", "def get():\n    return lib.util\n");
        let f = with_library(f, "user", "def u():\n    return h.get().x.json.dumps(1)\n");
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.file.path().ends_with("lib/two/code.py"))
            .collect();
        assert!(d.is_empty(), "{:?}", flagged(&d));
    }

    /// Review round 29 (B1): `del lib.util.json` needs the binding (on
    /// 8.3.3 a `KeyError` without the import) and `lib.util.json += 1` reads
    /// it first (`TypeError` with the import, `AttributeError` without;
    /// `chains.cases` of the library-routes oracle). Both are uses of the
    /// name, directly, through aliases, through other modules' imports and
    /// in tuple and list targets, from library modules and from view
    /// scripts; the fix keeps the import.
    #[test]
    fn deleting_or_augmenting_a_module_name_elsewhere_uses_it() {
        let lib = "import json\nimport os\ndef f():\n    return 1\n";
        let forms = [
            "del lib.util.json",
            "del m0.util.json",
            "del m0.m1.util.json",
            "import lib.util as u\n    del u.json",
            "from lib import util\n    del util.json",
            "del m0.util.json, n",
            "del [n, m0.util.json]",
            "del (m0.m1.util.json,)",
            "lib.util.json += 1",
            "m0.util.json += 1",
            "m0.m1.util.json -= 1",
            "import lib.util as u\n    u.json += 1",
            "from m0 import util\n    util.json <<= 1",
        ];
        for form in forms {
            let body = format!("n = 1\n    {form}\n");
            let modules = |f| {
                let f = with_library(f, "lib/util", lib);
                let f = with_library(f, "m0", "from lib import util\nimport m1\n");
                with_library(f, "m1", "from lib import util\n")
            };
            for f in [
                modules(library("user", &format!("def g():\n    {body}"))),
                modules(view(
                    ViewScript::Action,
                    &format!("\t{}", body.replace("    ", "\t")),
                )),
            ] {
                let d: Vec<_> = f
                    .lint(&["SCR102"])
                    .into_iter()
                    .filter(|d| d.file.path().ends_with("lib/util/code.py"))
                    .collect();
                assert_eq!(flagged(&d), ["os"], "{form}");
                let fixed = f.fixed(&[("SCR102", "")], true, &library_path(&f, "lib/util"));
                assert!(fixed.contains("import json\n"), "{form}: {fixed}");
            }
        }
    }

    /// Review round 30 (B1, new in round 29): a star import of a module
    /// named through another module's import (`from m0.exports import *`,
    /// `m0` doing `from lib import exports`), of a package (`from core
    /// import *`) or through a name bound twice, passed on by a further
    /// star import, reaches what that module binds (`chains.cases`: `ok`
    /// with the import, `AttributeError` without). The import is used;
    /// `--fix` keeps it.
    #[test]
    fn star_imports_through_imports_pass_names_on() {
        let leaf = "import json\nimport os\ndef f():\n    return 1\n";
        let firsts = [
            "from m0.exports import *\n",
            "from m0.e import *\n",
            "from m0.m00.exports import *\n",
            "from m0.v import *\n",
            "from core import *\n",
        ];
        for first in firsts {
            for user in [
                "def g():\n    return m2.leaf.json.dumps(1)\n",
                "from m2 import *\ndef g():\n    return leaf.json.dumps(1)\n",
                "from m1 import *\ndef g():\n    return leaf.json.dumps(1)\n",
            ] {
                let f = with_library(
                    library("core/leaf", leaf),
                    "lib/exports",
                    "from core import leaf\n",
                );
                let f = with_library(
                    f,
                    "m0",
                    "from lib import exports\nimport lib.exports as e\nimport m00\ntry:\n    from lib import exports as v\nexcept ImportError:\n    v = None\n",
                );
                let f = with_library(f, "m00", "from lib import exports\n");
                let f = with_library(f, "m1", first);
                let f = with_library(f, "m2", "from m1 import *\n");
                let f = with_library(f, "user", user);
                let d: Vec<_> = f
                    .lint(&["SCR102"])
                    .into_iter()
                    .filter(|d| d.file.path().ends_with("core/leaf/code.py"))
                    .collect();
                assert!(
                    !flagged(&d).contains(&"json".to_owned()),
                    "{first}{user}: {:?}",
                    flagged(&d)
                );
                // `os` is unused (unless the module may have any name
                // read): even unsafe fixes keep `json`.
                if !d.is_empty() {
                    let fixed = f.fixed(&[("SCR102", "")], true, &library_path(&f, "core/leaf"));
                    assert!(fixed.contains("import json\n"), "{first}{user}: {fixed}");
                }
            }
        }
    }

    /// Review round 29 (B1): an assignment from elsewhere replaces what the
    /// import binds (`lib.util.json = 5`, through other modules' imports, in
    /// `for`, `except` and tuple targets, or from a function of the module
    /// through `global`): the import may go, but not as a safe fix.
    /// (`setattr(m0.util, 'json', 5)` uses the module as a value, which
    /// keeps every name.)
    #[test]
    fn assigning_a_module_name_elsewhere_makes_the_removal_unsafe() {
        let lib = "import json\nimport os\ndef f():\n    return 1\n";
        for (form, own) in [
            ("lib.util.json = 5", ""),
            ("m0.util.json = 5", ""),
            ("m0.m1.util.json = 5", ""),
            ("import lib.util as u\n    u.json = 5", ""),
            ("for m0.util.json in [1]:\n        pass", ""),
            (
                "try:\n        pass\n    except KeyError, m0.util.json:\n        pass",
                "",
            ),
            ("m0.util.json, n = 1, 2", ""),
            ("pass", "def k():\n    global json\n    json = 5\n"),
        ] {
            let f = with_library(
                library("lib/util", &format!("{lib}{own}")),
                "m0",
                "from lib import util\nimport m1\n",
            );
            let f = with_library(f, "m1", "from lib import util\n");
            let f = with_library(f, "user", &format!("def g():\n    {form}\n"));
            let d: Vec<_> = f
                .lint(&["SCR102"])
                .into_iter()
                .filter(|d| d.file.path().ends_with("lib/util/code.py"))
                .collect();
            let json: Vec<_> = d.iter().filter(|d| d.message.contains("`json`")).collect();
            assert_eq!(json.len(), 1, "{form}: {:?}", flagged(&d));
            let fix = json[0].fix.as_ref().expect("fix");
            assert_eq!(
                fix.applicability,
                crate::diagnostic::Applicability::Unsafe,
                "{form}"
            );
            assert!(
                fix.unsafe_reason
                    .as_ref()
                    .is_some_and(|r| r.message.contains("assigns the name")),
                "{form}: {:?}",
                fix.unsafe_reason
            );
            // The reason names the store, in the resource that stores or
            // in the module's function (review round 30, Opus nit 3).
            let site = fix
                .unsafe_reason
                .as_ref()
                .and_then(|r| r.site.as_ref())
                .unwrap_or_else(|| panic!("{form}: no site"));
            let line_of = |text: &str, needle: &str| {
                text.lines().position(|l| l.contains(needle)).expect("line") as u32 + 1
            };
            let (file, line) = if own.is_empty() {
                let user = format!("def g():\n    {form}\n");
                ("user/code.py", line_of(&user, ".json"))
            } else {
                (
                    "lib/util/code.py",
                    line_of(&format!("{lib}{own}"), "json = 5"),
                )
            };
            assert!(
                site.path.ends_with(file) && site.line == line && !site.what.is_empty(),
                "{form}: {site:?}"
            );
            // Safe fixes only: `os` goes, `json` stays.
            let result = f.run(
                &[("SCR102", "")],
                crate::engine::FixMode::DiffOnly {
                    unsafe_fixes: false,
                },
            );
            let path = library_path(&f, "lib/util");
            let fixed = &result
                .fixed
                .iter()
                .find(|s| s.path == path)
                .expect("fixed")
                .fixed;
            assert_eq!(
                fixed,
                &format!(
                    "import json\n\n{}{own}",
                    lib.strip_prefix("import json\nimport os\n")
                        .expect("prefix")
                ),
                "{form}"
            );
        }
        // From a view script (the Opus repro of review round 30, nit 3):
        // the site is in the view.
        let f = with_library(
            view(ViewScript::Action, "\tlib.util.json = 1"),
            "lib/util",
            lib,
        );
        let d = f.lint(&["SCR102"]);
        let site = d
            .iter()
            .find(|d| d.message.contains("`json`"))
            .and_then(|d| d.fix.as_ref())
            .and_then(|f| f.unsafe_reason.as_ref())
            .and_then(|r| r.site.as_ref())
            .unwrap_or_else(|| panic!("{d:#?}"));
        assert!(
            site.path.ends_with("view.json") && site.what == "lib.util.json",
            "{site:?}"
        );
        // Without the store, the removal is safe.
        let f = with_library(library("lib/util", lib), "user", "def g():\n    pass\n");
        let d = f.lint(&["SCR102"]);
        assert_eq!(
            applicability(&d),
            [Some(crate::diagnostic::Applicability::Safe); 2]
        );
    }

    /// Review round 29 (N1): `__all__` changed in a way the model does not
    /// read (a slice or item store, `insert`, `+=`, an alias, a function)
    /// may export any name, also one starting with `_`.
    #[test]
    fn changed_all_may_export_any_name() {
        let user = "from lib.util import *\ndef g():\n    return json.dumps(f()), _j\n";
        for all in [
            "__all__ = ['f']\n__all__[:] = ['f', 'json', '_j']\n",
            "__all__ = ['f', 'x', 'y']\n__all__[1] = 'json'\n__all__[2] = '_j'\n",
            "__all__ = ['f']\n__all__.insert(0, 'json')\n__all__.insert(0, '_j')\n",
            "__all__ = ['f']\n__all__ += ['json', '_j']\n",
            "__all__ = ['f']\nnames = __all__\nnames.extend(['json', '_j'])\n",
            "__all__ = ['f']\ndef k():\n    __all__.extend(['json', '_j'])\n",
            "__all__ = ['f']\ndel __all__[0]\n__all__.extend(['f', 'json', '_j'])\n",
            "if f:\n    __all__ = ['f']\nelse:\n    __all__ = ['f', 'json', '_j']\n",
            // Review round 30 (Opus minor 2): one binding that is not an
            // assignment to the bare name (`chains.cases`, 8.3.3).
            "from lib.names import __all__\n",
            "for __all__ in [['f', 'json', '_j']]:\n    pass\n",
            "class C(object):\n    def __enter__(self):\n        return ['f', 'json', '_j']\n    def __exit__(self, *a):\n        return False\nwith C() as __all__:\n    pass\n",
            "__all__, x = ['f', 'json', '_j'], 1\n",
            "[__all__] = [['f', 'json', '_j']]\n",
            "import lib.names as __all__\n",
            "def __all__():\n    pass\n",
        ] {
            let code = format!("import json\nimport json as _j\n{all}def f():\n    return 1\n");
            let f = with_library(library("lib/util", &code), "user", user);
            let f = with_library(f, "lib/names", "__all__ = ['f', 'json', '_j']\n");
            let d: Vec<_> = f
                .lint(&["SCR102"])
                .into_iter()
                .filter(|d| d.file.path().ends_with("lib/util/code.py"))
                .collect();
            assert!(d.is_empty(), "{all}: {:?}", flagged(&d));
        }
        // A literal `__all__`, also extended with literals at module level,
        // is read exactly.
        let code = "import json\nimport os\n__all__ = ['f']\n__all__.append('json')\ndef f():\n    return 1\n";
        let f = with_library(library("lib/util", code), "user", user);
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.file.path().ends_with("lib/util/code.py"))
            .collect();
        assert_eq!(flagged(&d), ["os"]);
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
        // `lib/util` holds `ev = eval`, so the whole workspace has a frame
        // reader: the closed-world check makes even a clean-callee fix
        // unsafe (reviewer round 7, H3).
        assert_eq!(
            fixes("def f():\n    import os\n    lib.plain.log(1)\n"),
            [(
                "`os` imported but unused".to_owned(),
                crate::Applicability::Unsafe
            )]
        );
        // In a workspace with no unknown code, the clean-callee fix is safe.
        let clean = with_library(
            library("m", "def f():\n    import os\n    lib.plain.log(1)\n"),
            "lib/plain",
            "def log(m):\n    pass\n",
        )
        .lint(&["SCR102"]);
        assert_eq!(
            clean
                .iter()
                .map(|d| d.fix.as_ref().unwrap().applicability)
                .collect::<Vec<_>>(),
            [crate::Applicability::Safe]
        );
        assert!(
            fixes("import os\nimport m as me\ndef f(n):\n    return getattr(me, n)\n").is_empty()
        );
    }

    /// Reviewer round 9 (Opus) finding 2: another module reads a library
    /// module's import through the module object: a literal `getattr` or
    /// `hasattr` name is a static use, and the module used as a value (a
    /// default argument, a container, the globals dictionary, `sys.modules`
    /// reached through a vetted module) makes every top-level name count
    /// as used. No finding in either case.
    #[test]
    fn imports_read_through_the_module_object_elsewhere_are_used() {
        for user in [
            "def f():\n    return getattr(a.b, \"json\").dumps(1)\n",
            "def f():\n    return hasattr(a.b, \"json\")\n",
            "def g(m=a.b):\n    return m.json\n",
            "def f():\n    return [x.json for x in [a.b]]\n",
            "def f():\n    import re\n    return re.sys.modules[\"a.b\"].json\n",
            "m = a.b\ndef f():\n    return m.json\n",
        ] {
            let f = with_library(library("a/b", "import json\n"), "c/d", user);
            let d: Vec<_> = f
                .lint(&["SCR102"])
                .into_iter()
                .filter(|d| d.file.path().ends_with("a/b/code.py"))
                .collect();
            assert!(d.is_empty(), "{user}: {d:?}");
        }
        // Through the globals dictionary, read in another module.
        let f = with_library(
            with_library(
                library("a/b", "import json\n"),
                "c/d",
                "system.util.getGlobals()[\"m\"] = a.b\n",
            ),
            "e/f",
            "def f():\n    return system.util.getGlobals()[\"m\"].json\n",
        );
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.file.path().ends_with("a/b/code.py"))
            .collect();
        assert!(d.is_empty(), "{d:?}");
        // Unrelated literal names still leave the import unused.
        let f = with_library(
            library("a/b", "import json\n"),
            "c/d",
            "def f():\n    return getattr(a.b, \"other\")\n",
        );
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.file.path().ends_with("a/b/code.py"))
            .collect();
        assert_eq!(d.len(), 1, "{d:?}");
    }

    /// Review round 26 (Opus, blocker): a special attribute of a library
    /// module read from another module (`m.__getattribute__('re')`,
    /// `m.__findattr_ex__('re')`, `m.__dict__`) or a member of Ignition's
    /// module object (`m.dict`, `m.getDict()`, measured on 8.3.3) reaches
    /// the module's names: `import re` in `m` is used or its removal
    /// unsafe, also through aliases and package chains, and `--fix` keeps
    /// it. The table of every measured route is
    /// `rules::refs::library_routes`.
    #[test]
    fn special_attributes_of_library_modules_elsewhere_use_their_imports() {
        let module = "import re\n\ndef f():\n    return 1\n";
        for user in [
            "def g():\n    return m.__getattribute__('re').compile('a+').match('aaa').group(0)\n",
            "def g():\n    return m.__findattr_ex__('re')\n",
            "def g(n):\n    return m.__getattribute__(n)\n",
            "def g():\n    m.__delattr__('re')\n",
            "def g():\n    return m.dict['re']\n",
            "def g():\n    return m.getDict()['re']\n",
            "def g():\n    return m.__reduce_ex__(2)[2]['re']\n",
            "def g():\n    x = m\n    return x.__getattribute__('re')\n",
            "def g():\n    return getattr(m, 'dict')['re']\n",
        ] {
            let f = with_library(library("m", module), "b", user);
            let found: Vec<_> = f
                .lint(&["SCR102"])
                .into_iter()
                .filter(|d| d.file.path().ends_with("m/code.py"))
                .collect();
            assert!(
                found.iter().all(|d| d
                    .fix
                    .as_ref()
                    .is_none_or(|x| x.applicability != crate::diagnostic::Applicability::Safe)),
                "{user}: {found:?}"
            );
            f.run(&[("SCR102", "")], crate::engine::FixMode::Safe);
            assert_eq!(
                std::fs::read_to_string(library_path(&f, "m")).unwrap(),
                module,
                "{user}"
            );
        }
        let f = with_library(
            with_library(library("pkg/m", module), "pkg/n", "X = 1\n"),
            "b",
            "def g():\n    return pkg.m.__getattribute__('re')\n",
        );
        assert!(
            f.lint(&["SCR102"])
                .iter()
                .all(|d| !d.file.path().ends_with("pkg/m/code.py")),
        );
        // A name the module binds wins over the object's member.
        let f = with_library(
            library("m", "import re\nname = 'x'\n\ndef f():\n    return 1\n"),
            "b",
            "def g():\n    return m.name\n",
        );
        assert_eq!(
            applicability(&f.lint(&["SCR102"])),
            vec![Some(crate::diagnostic::Applicability::Safe)]
        );
    }

    /// Review round 26: the namespace of a library module (`m.dict`,
    /// `m.getDict()`, `m.__reduce_ex__(2)[2]`) holds its `__builtins__`,
    /// whose `locals()` reads the caller's frame (measured on 8.3.3: `['os']`
    /// with the import, `[]` without): the caller's own import is used.
    #[test]
    fn library_module_namespaces_read_the_callers_frame() {
        for route in [
            "m.dict['__builtins__']['locals']()",
            "m.getDict()['__builtins__']['locals']()",
            "m.__reduce_ex__(2)[2]['__builtins__']['locals']()",
            "vars(m)['__builtins__']['locals']()",
            "m.__dict__['__builtins__']['locals']()",
            "getattr(m, '__dict__')['__builtins__']['locals']()",
        ] {
            let user = format!("def g():\n    import os\n    return sorted({route}.keys())\n");
            let f = with_library(library("m", "def f():\n    return 1\n"), "b", &user);
            assert!(f.lint(&["SCR102"]).is_empty(), "{route}");
        }
        // Through a parameter the module is not known, the fix is unsafe.
        let user = "def g(x):\n    import os\n    return x.dict['__builtins__']['locals']()\n";
        let f = with_library(library("m", "def f():\n    return 1\n"), "b", user);
        assert_eq!(
            applicability(&f.lint(&["SCR102"])),
            vec![Some(crate::diagnostic::Applicability::Unsafe)]
        );
    }

    /// Reviewer round 9 (Codex) blocker: removing `from java.util import
    /// MissingReviewClass` turns a failed call into a successful one when
    /// the caller catches the `ImportError` (verified on Jython: `failed`
    /// before, `succeeded` after). Without positive evidence that the
    /// import succeeds the fix is unsafe.
    /// Reviewer round 10 (Codex) blocker: removing `import math` changes
    /// what a script that reads the module's own file sees
    /// (`open(...).read().startswith('import math\\n')` flips from `True` to
    /// `False` on Jython). The fix is unsafe and names the read.
    #[test]
    fn file_reads_make_the_removal_unsafe() {
        let code = "import math\nprint open('/w/projects/p/ignition/script-python/m/code.py').read().startswith('import math\\n')\n";
        let f = library("m", code);
        let d = f.lint(&["SCR102"]);
        assert_eq!(d.len(), 1);
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.applicability, crate::Applicability::Unsafe);
        let reason = fix.unsafe_reason.as_ref().unwrap();
        let site = reason.site.as_ref().unwrap();
        assert!(site.what.starts_with("open('/w/projects/"), "{reason:?}");
        assert_eq!((site.line, site.column), (2, 7));
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
        // With the opt-in, the closed world decides as before.
        let fixed = f.fixed_assuming(&[("SCR102", "")], false, &library_path(&f, "m"), true);
        assert!(fixed.starts_with("\nprint open("), "{fixed}");
    }

    /// A view `B` whose root holds `child` (a component object).
    fn client_view(child: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [{child}]}}}}"#
        )
    }

    /// Reviewer round 18 (Opus) blocker B1: `system.*` functions,
    /// expression functions and components iglint does not know are
    /// unknown code. A third-party `system.*` function in the module itself
    /// names its site; one elsewhere, an expression function the gateway
    /// does not register (`objectScript` runs Python) and a component that
    /// runs JavaScript in the browser make the fix unsafe under the closed
    /// world. `[fix] vetted-system-functions` and
    /// `vetted-expression-functions` vet the names they list.
    #[test]
    fn unknown_system_and_expression_functions_and_client_code_are_unknown() {
        let code = "import math\n\ndef f():\n    return system.thirdparty.evalHere('x')\n";
        let f = library("m", code);
        let d = f.lint(&["SCR102"]);
        assert_eq!(d.len(), 1, "{d:?}");
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.applicability, crate::Applicability::Unsafe);
        let site = fix.unsafe_reason.as_ref().unwrap().site.as_ref().unwrap();
        assert_eq!(
            (site.line, site.what.as_str()),
            (4, "system.thirdparty.evalHere")
        );
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
        let vetted = crate::settings::Vetted {
            system_functions: vec!["system.thirdparty.evalHere".to_owned()],
            ..Default::default()
        };
        assert_eq!(
            applicability(&f.lint_vetting(&["SCR102"], &vetted)),
            [Some(crate::Applicability::Safe)]
        );
        // Elsewhere in the workspace.
        let plain = "import math\n\ndef f():\n    return 1\n";
        let elsewhere = with_library(
            library("m", plain),
            "n",
            "def g():\n    return system.perspective.runJavaScriptAsync('1')\n",
        );
        let d = elsewhere.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.as_ref().unwrap();
        assert!(reason.message.starts_with("closed world"), "{reason:?}");
        let site = reason.site.as_ref().unwrap();
        assert!(
            site.what
                .starts_with("system.perspective.runJavaScriptAsync"),
            "{reason:?}"
        );
        let binding = r#"{"custom": {"x": 1}, "propConfig": {"custom.x": {"binding": {"type": "expr", "config": {"expression": "objectScript(\"eval('1')\")"}}}}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#;
        let expression = library("m", plain).with_view("B", binding);
        let d = expression.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
        let site = d[0]
            .fix
            .as_ref()
            .unwrap()
            .unsafe_reason
            .as_ref()
            .unwrap()
            .site
            .as_ref()
            .unwrap();
        assert!(site.what.starts_with("objectScript"), "{site:?}");
        let vetted = crate::settings::Vetted {
            expression_functions: vec!["OBJECTSCRIPT".to_owned()],
            ..Default::default()
        };
        assert_eq!(
            applicability(&expression.lint_vetting(&["SCR102"], &vetted)),
            [Some(crate::Applicability::Safe)]
        );
        // Perspective's `property` is an expression function of a view's
        // bindings, but reads properties by a computed path: still
        // unknown code; `isAuthorized` is not.
        for (call, safe) in [
            ("property('this.custom.' + 'x')", false),
            ("isAuthorized(false)", true),
        ] {
            let binding = format!(
                r#"{{"custom": {{"x": 1, "y": 2}}, "propConfig": {{"custom.y": {{"binding": {{"type": "expr", "config": {{"expression": "{call}"}}}}}}}}, "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#
            );
            let d = library("m", plain)
                .with_view("B", &binding)
                .lint(&["SCR102"]);
            let expected = if safe {
                crate::Applicability::Safe
            } else {
                crate::Applicability::Unsafe
            };
            assert_eq!(applicability(&d), [Some(expected)], "{call}");
        }
        for (child, safe) in [
            (
                r#"{"type": "ia.display.label", "meta": {"name": "L"}, "props": {"text": "<b>x</b>"}}"#,
                true,
            ),
            (
                r#"{"type": "ia.display.iframe", "meta": {"name": "F"}, "props": {"src": "https://example.com"}}"#,
                false,
            ),
            (
                r#"{"type": "embr.periscope.embedding.javascript", "meta": {"name": "J"}}"#,
                false,
            ),
            (
                r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"escapeHtml": false}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"skipHtml": false}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"escapeHtml": true, "allowDangerousHtml": true}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"escapeHtml": true, "skipHtml": false, "renderers": {}}}}"#,
                true,
            ),
            (
                r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"escapeHtml": true}}, "propConfig": {"props.markdown.escapeHtml": {"binding": {"type": "expr", "config": {"expression": "false"}}}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.navigation.link", "meta": {"name": "A"}, "props": {"url": "javascript:alert(1)"}}"#,
                false,
            ),
            (
                r#"{"type": "ia.navigation.link", "meta": {"name": "A"}, "props": {"url": "https://example.com"}}"#,
                true,
            ),
            (
                r#"{"type": "ia.navigation.link", "meta": {"name": "A"}, "props": {"url": "/page"}, "propConfig": {"props.url": {"binding": {"type": "expr", "config": {"expression": "'x'"}}}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.input.button", "meta": {"name": "B"}, "events": {"dom": {"onClick": {"type": "nav", "config": {"url": "javascript:alert(1)"}}}}}"#,
                false,
            ),
            (
                r##"{"type": "ia.shapes.svg", "meta": {"name": "S"}, "props": {"elements": [{"type": "group", "elements": [{"type": "path", "d": "M0 0", "fill": {"url": "#g"}}, {"type": "use", "href": "#p"}]}, {"type": "linearGradient", "id": "g", "elements": [{"type": "stop", "offset": 0}]}]}}"##,
                true,
            ),
            (
                r#"{"type": "ia.shapes.svg", "meta": {"name": "S"}, "props": {"elements": [{"type": "group", "elements": [{"type": "a", "href": "javascript:alert(1)"}]}]}}"#,
                false,
            ),
            (
                r#"{"type": "ia.shapes.svg", "meta": {"name": "S"}, "props": {"elements": [{"type": "path", "onClick": "x"}]}}"#,
                false,
            ),
            (
                r#"{"type": "ia.shapes.svg", "meta": {"name": "S"}, "props": {"elements": [{"type": "use", "href": "data:x"}]}}"#,
                false,
            ),
            (
                r#"{"type": "ia.shapes.svg", "meta": {"name": "S"}, "props": {"elements": [{"type": "path"}]}, "propConfig": {"props.elements": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}"#,
                false,
            ),
            (
                r#"{"type": "ia.input.button", "meta": {"name": "B"}, "events": {"dom": {"onClick": {"type": "nav", "config": {"url": "https://example.com"}}}}}"#,
                true,
            ),
        ] {
            let f = library("m", plain).with_view("B", &client_view(child));
            let expected = if safe {
                crate::Applicability::Safe
            } else {
                crate::Applicability::Unsafe
            };
            let d = f.lint(&["SCR102"]);
            assert_eq!(applicability(&d), [Some(expected)], "{child}");
            if !safe {
                let site = d[0]
                    .fix
                    .as_ref()
                    .unwrap()
                    .unsafe_reason
                    .as_ref()
                    .unwrap()
                    .site
                    .as_ref()
                    .unwrap();
                assert!(site.what.contains("JavaScript in the browser"), "{site:?}");
                assert!(site.path.ends_with("views/B/view.json"), "{site:?}");
            }
        }
        // A plain Markdown component or link that a script of its view
        // may change.
        let scripted = r#"{"type": "ia.display.markdown", "meta": {"name": "M"}, "props": {"markdown": {"escapeHtml": true}}, "events": {"dom": {"onClick": {"type": "script", "scope": "G", "config": {"script": "\tpass"}}}}}"#;
        let d = library("m", plain)
            .with_view("B", &client_view(scripted))
            .lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
    }

    /// Reviewer round 18 (Opus) blocker B2: removing an import from a
    /// function near the 64 KB method limit can make a function Jython
    /// rejects compile (the review's repro: 2,176 `a = abs(...)` lines).
    /// The lower bound cannot see it, so the safety net does not trust
    /// the change: the fix is reported unsafe with the reason and `--fix`
    /// does not apply it.
    #[test]
    fn removals_near_the_method_limit_are_unsafe() {
        let mut code = String::from("def g():\n    import math\n");
        for i in 0..2_176 {
            code.push_str(&format!("    a = abs({})\n", i % 7));
        }
        code.push_str("    c = 1\n    c = 1\n    return a + c\n");
        let f = library("m", &code);
        let d = f.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.as_ref().unwrap();
        assert!(
            reason
                .message
                .contains("compile status is not known: the function `g` needs between"),
            "{reason:?}"
        );
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
        // A small function keeps the safe fix.
        let d = library("m", "def g():\n    import math\n    return 1\n").lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Safe)]);
    }

    /// Reviewer round 19 (Opus) blocker: the upper bound charged a
    /// closure's free variables once per unit, not per creation, so it
    /// said this function fits: Jython rejects it, and compiles it
    /// without the import (65,517 bytes). The bound now follows Jython's
    /// code generator (a cell per free variable at every creation); the
    /// verdict is open and the fix is withheld.
    #[test]
    fn removals_from_functions_full_of_closures_are_unsafe() {
        let p: Vec<String> = (0..150).map(|i| format!("p{i}")).collect();
        let p = p.join(", ");
        let mut code = format!("def big({p}):\n    import math\n");
        for i in 0..25 {
            code.push_str(&format!("    l{i} = lambda: ({p})\n"));
        }
        for _ in 0..84 {
            code.push_str("    f(g(a, k=h(b, k=i(c, k=d))), k=j(a, b, c, d, e))\n");
        }
        for i in 0..8 {
            code.push_str(&format!("    x = {i}\n"));
        }
        code.push_str("    return 1\n");
        let f = library("m", &code);
        let d = f.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.as_ref().unwrap();
        assert!(
            reason
                .message
                .contains("compile status is not known: the function `big` needs between"),
            "{reason:?}"
        );
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
    }

    /// Reviewer round 20 (Codex) blocker: Jython loads names of more than
    /// 16,000 characters in chunks, which the upper bound did not charge,
    /// so it said this function fits: Jython rejects it, and compiles it
    /// with the import's line blanked (65,528 bytes). Such an identifier
    /// makes the compile status unknown, and the fix is withheld.
    #[test]
    fn removals_from_functions_with_long_names_are_unsafe() {
        let mut code = String::from("def work(obj):\n    import math\n");
        for _ in 0..1_683 {
            code.push_str(&format!("    obj.{}\n", "n".repeat(16_001)));
        }
        code.push_str("    return 1\nprint \"loaded\"\n");
        let f = library("m", &code);
        let d = f.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(crate::Applicability::Unsafe)]);
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.as_ref().unwrap();
        assert!(
            reason.message.contains(
                "compile status is not known: identifier of 16,001 characters: Jython's code size cannot be bounded precisely"
            ),
            "{reason:?}"
        );
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
    }

    #[test]
    fn removing_an_import_that_fails_is_unsafe() {
        let code = "def f():\n    from java.util import MissingReviewClass\ntry:\n    f()\nexcept Exception:\n    print \"failed\"\nelse:\n    print \"succeeded\"\n";
        let d = library("m", code).lint(&["SCR102"]);
        assert_eq!(
            applicability(&d),
            [Some(crate::diagnostic::Applicability::Unsafe)]
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
            // Not vetted: may resolve names from data.
            ("import javax.swing", Unsafe),
            ("import cPickle", Unsafe),
            ("import warnings", Unsafe),
            ("from java.lang import Thread", Unsafe),
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
            // An import that fails: removing it changes the program when
            // the caller catches the error (reviewer round 9, Codex).
            ("from java.util import MissingReviewClass", Unsafe),
            ("import java.util.MissingReviewClass", Unsafe),
            ("from json import MissingReviewName", Unsafe),
            ("from os import missing_review_name", Unsafe),
            ("from os import getcwd", Safe),
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

    /// Reviewer repros R6: line numbers read through `warnings`, a
    /// `logging` handler and Java stack traces; `cPickle` reading the
    /// module's globals; a removal that makes a misplaced `__future__`
    /// import valid; a removal that changes what a name resolves to; a
    /// reflective accessor under another name. None of them may change
    /// under `--fix`.
    #[test]
    fn review_round_six_repros() {
        use crate::diagnostic::Applicability::Unsafe;
        let header = "# iglint: ignore[SCR101]\nimport os\n";
        for (name, code, reported) in [
            ("warn", format!("{header}import warnings\ndef where():\n    with warnings.catch_warnings(record=True) as w:\n        warnings.simplefilter('always')\n        warnings.warn('x')\n        return w[0].lineno\n"), true),
            ("logh", format!("{header}import logging\nclass H(logging.Handler):\n    def emit(self, record):\n        self.line = record.lineno\n"), true),
            ("thread", format!("{header}from java.lang import Thread\ndef where():\n    for e in Thread.currentThread().getStackTrace():\n        if '<thread>' in str(e):\n            return e.getLineNumber()\n"), true),
            ("throw", format!("{header}from java.lang import Throwable\ndef where():\n    return Throwable().getStackTrace()[0].getLineNumber()\n"), true),
            ("pick", "import os\nimport cPickle\ndef where():\n\treturn cPickle.loads(\"c__builtin__\\nglobals\\n(tR.\").get(\"os\")\n".to_owned(), true),
            ("fut", "import os\nfrom __future__ import division\ndef where():\n\treturn 1/2\n".to_owned(), true),
            ("unbound", "def f():\n    try:\n        print len([1])\n    except UnboundLocalError:\n        print 'unbound'\n    import math as len\n".to_owned(), true),
            ("alias", "import os\nget = getattr\ng = (x for x in [])\nframe = get(g, 'gi_' + 'frame')\nnamespace = get(frame, 'f_' + 'globals')\nprint namespace['os'].name\n".to_owned(), false),
        ] {
            let f = library(&format!("lib/{name}"), &code);
            let d = f.lint(&["SCR102"]);
            assert_eq!(!d.is_empty(), reported, "{name}: {d:?}");
            for d in &d {
                assert_eq!(d.fix.as_ref().map(|f| f.applicability), Some(Unsafe), "{name}");
            }
            let path = library_path(&f, &format!("lib/{name}"));
            let fixed = safe_fixed(&f, &["SCR101", "SCR102", "SCR123"], &path);
            // Safe fixes may empty lines, never remove or move them, and
            // leave every import in place here.
            assert_eq!(fixed.lines().count(), code.lines().count(), "{name}");
            for (a, b) in code.lines().zip(fixed.lines()) {
                if a != b {
                    assert!(b.is_empty() && a.starts_with('#'), "{name}: {a:?} -> {b:?}");
                }
            }
        }
        // With unsafe fixes the import goes, with its line.
        let f = library("m", "import os\nimport cPickle\nx = cPickle.dumps(1)\n");
        let fixed = f.fixed(&[("SCR102", "")], true, &library_path(&f, "m"));
        assert_eq!(fixed, "import cPickle\nx = cPickle.dumps(1)\n");
    }

    /// A string literal names a member only in a sink position (reviewer
    /// round 8, finding 1): a docstring that mentions `module` (the bean
    /// name of `java.lang.Class.getModule`) is data; the same word as a
    /// `getattr` name is a first hop into reflection, which keeps the
    /// finding and makes the fix unsafe (review round 24), and `f_locals`
    /// there reads names, which silences module-level imports.
    #[test]
    fn literals_name_members_only_in_sinks() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        let d = library("m", "import os\n'module docstring'\n").lint(&["SCR102"]);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(applicability(&d), [Some(Safe)]);
        let d =
            library("m", "import os\nimport math\nm = getattr(math, 'module')\n").lint(&["SCR102"]);
        assert_eq!(flagged(&d), ["os"], "{d:?}");
        assert_eq!(applicability(&d), [Some(Unsafe)]);
        let d = library(
            "m",
            "import os\nimport math\nm = getattr(math, 'f_locals')\n",
        )
        .lint(&["SCR102"]);
        assert!(d.is_empty(), "{d:?}");
    }

    /// Reviewer round 8 (Codex) blocker: vetting resolved names in the
    /// order the walk met them, so a function defined before `import re`
    /// read `re.sys` unseen and `import math` was removed by a safe fix
    /// (Jython: `3.14159265359` before the fix, `NameError` after). The
    /// verdict is now order-free; the same code in another library module
    /// makes the fix of a clean module unsafe too (closed world).
    #[test]
    fn functions_before_imports_are_vetted() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        // Since review round 25 (M2) a module's `sys` attribute reads names
        // for the findings too (here `eval` reads `math` in `target`): no
        // finding. The order-free verdict is checked with an attribute that
        // is not vetted and reads nothing (`re.sre_parse`, a submodule).
        let observed = "def capture():\n    return re.sys.modules['__builtin__']\nimport re\ndef target():\n    import math\n    print capture().eval('math.pi')\ntarget()\n";
        assert!(library("m", observed).lint(&["SCR102"]).is_empty());
        let exploit = "def capture():\n    return re.sre_parse\nimport re\ndef target():\n    import math\n    print capture()\ntarget()\n";
        let f = library("m", exploit);
        let d = f.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(Unsafe)], "{d:?}");
        // The fix says why it is unsafe and where (reviewer round 8,
        // finding 2).
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.clone().unwrap();
        assert_eq!(reason.message, "the script uses code that is not vetted");
        assert!(!reason.workspace);
        let site = reason.site.unwrap();
        assert!(site.path.ends_with("m/code.py"), "{site:?}");
        assert_eq!(
            (site.line, site.column, site.what.as_str()),
            (2, 12, "re.sre_parse")
        );
        let fixed = safe_fixed(&f, &["SCR102"], &library_path(&f, "m"));
        assert_eq!(fixed, exploit);
        // Nested: the import comes after the inner function, in the outer.
        let nested = "def outer():\n    def capture():\n        return re.sre_parse\n    import re\n    def target():\n        import math\n        print capture()\n    target()\n";
        let d = library("m", nested).lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(Unsafe)], "{d:?}");
        // Workspace propagation: a clean module next to the exploit.
        let clean = "def g():\n    import os\n    return 1\n";
        assert_eq!(
            applicability(&library("c", clean).lint(&["SCR102"])),
            [Some(Safe)]
        );
        let f = with_library(library("c", clean), "m", exploit);
        let d: Vec<_> = f
            .lint(&["SCR102"])
            .into_iter()
            .filter(|d| d.message.contains("`os`"))
            .collect();
        assert_eq!(applicability(&d), [Some(Unsafe)], "{d:?}");
        // Closed world: the reason names the workspace's first unknown code.
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.clone().unwrap();
        assert!(reason.workspace, "{reason:?}");
        assert!(reason.message.starts_with("closed world"), "{reason:?}");
        let site = reason.site.unwrap();
        assert!(site.path.ends_with("m/code.py"), "{site:?}");
        assert_eq!((site.line, site.what.as_str()), (2, "re.sre_parse"));
        // Control: without the capture, the same import is removed safely.
        let benign =
            "import re\ndef target():\n    import math\n    print re.escape('a')\ntarget()\n";
        let f = library("m", benign);
        assert_eq!(applicability(&f.lint(&["SCR102"])), [Some(Safe)]);
    }

    /// Two deletions that together promote a string to a docstring in a
    /// view script: the safe-fix net keeps the second import (reviewer
    /// repro R6-E2).
    #[test]
    fn combined_deletions_keep_docstrings_in_view_scripts() {
        let f = view(
            ViewScript::Action,
            "\tdef f():\n\t\timport os\n\t\timport sys\n\t\t'promoted'\n\t\treturn f.__doc__\n\tprint f()\n",
        );
        let fixed = safe_fixed(&f, &["SCR102"], &view_path(&f));
        // Both imports go and a `pass` keeps the string from becoming the
        // docstring; the fix reaches this in one run (reviewer round 7,
        // finding 3).
        assert!(
            fixed.contains(r"\t\tpass\n\t\t\u0027promoted\u0027"),
            "{fixed}"
        );
        assert!(!fixed.contains("import os"), "{fixed}");
        assert!(!fixed.contains("import sys"), "{fixed}");
    }

    /// Every import that runs in a library module binds `__package__`
    /// (`None`) in the module's globals; removing the only one changes a
    /// read of it (Ignition 8.3.3: `None` before, `'ABSENT'` after, see
    /// `iglint_python_semantic/tests/oracle/implicit_names.cases`,
    /// `k1-repro-*`). The fix is unsafe when anything may observe it
    /// (reviewer round 12, K1).
    #[test]
    fn removing_the_import_that_binds_package_is_unsafe_when_observed() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        let repro = "import math\n\ndef get_package():\n    try:\n        return __package__\n    except NameError:\n        return 'ABSENT'\n";
        let f = library("m", repro);
        let d = f.lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(Unsafe)], "{d:?}");
        let reason = d[0].fix.as_ref().unwrap().unsafe_reason.clone().unwrap();
        assert!(reason.message.contains("`__package__`"), "{reason:?}");
        assert!(!safe_fix_applies(&f, "SCR102", "m"));
        // A function-local import binds the module's `__package__` too.
        let local = "def g():\n    import math\ng()\nprint __package__\n";
        assert_eq!(
            applicability(&library("m", local).lint(&["SCR102"])),
            [Some(Unsafe)]
        );
        // Code that binds `__name__` or reads `__warningregistry__`, or
        // reads the globals by name.
        for code in [
            "__name__ = 'a.b'\ndef g():\n    import math\n",
            "def g():\n    import math\n    return __warningregistry__\n",
            "def g():\n    import math\ndef h():\n    return globals()\n",
        ] {
            assert_eq!(
                applicability(&library("m", code).lint(&["SCR102"])),
                [Some(Unsafe)],
                "{code}"
            );
        }
        // Another resource reads it as a module attribute, through the
        // module's `__dict__` or with the module as a value.
        for user in [
            "def f():\n    return sem.m.__package__\n",
            "def f():\n    return getattr(sem.m, '__package__')\n",
            "def f():\n    return sem.m.__dict__\n",
            "def f():\n    return vars(sem.m)\n",
        ] {
            let f = with_library(
                library("sem/m", "def g():\n    import math\n"),
                "sem/user",
                user,
            );
            let d = f.lint(&["SCR102"]);
            assert_eq!(applicability(&d), [Some(Unsafe)], "{user}: {d:?}");
        }
        // Fails closed: another import still binds it, but the read keeps
        // the fix unsafe.
        let kept = "import math\nimport os\nprint os.sep\ndef get():\n    return __package__\n";
        let d = library("m", kept).lint(&["SCR102"]);
        assert_eq!(applicability(&d), [Some(Unsafe)]);
        // Controls: the statement stays, or nothing reads it.
        let partial = "import os, math\nprint os.sep\nprint __package__\n";
        assert_eq!(
            applicability(&library("m", partial).lint(&["SCR102"])),
            [Some(Safe)]
        );
        let unread = "def g():\n    import math\n";
        assert_eq!(
            applicability(&library("m", unread).lint(&["SCR102"])),
            [Some(Safe)]
        );
        // Event scripts run without `__name__`: their imports bind no
        // `__package__` (measured), so a read does not matter.
        let f = view(
            ViewScript::Action,
            "\timport math\n\ttry:\n\t\tprint __package__\n\texcept NameError:\n\t\tpass\n",
        );
        assert_eq!(applicability(&f.lint(&["SCR102"])), [Some(Safe)]);
    }

    /// Review round 13: class bodies read names by name (the class
    /// namespace, then the module's globals and the builtins, whatever the
    /// class binds later) and spell private names mangled; the interpreter
    /// reads `__metaclass__` for a class without bases. Each of these
    /// imports is used: Jython raises `NameError` (or creates the class)
    /// differently without it.
    #[test]
    fn imports_read_by_class_bodies_are_used() {
        for code in [
            "import math as _D__m\n\nclass D(object):\n    x = __m.floor(3.7)\n",
            "import math as m\n\nclass C(object):\n    x = m.pi\n    m = 1\n\nm = None\n",
            "import math as m\n\nclass C(object):\n    x = m.pi\n    m = 1\n",
            "import math as m\n\ndef f():\n    class C(object):\n        x = m.pi\n        m = 1\n    return C\n",
            "from math import pi as _D__pi\n\nclass D(object):\n    x = __pi\n",
            "import math as __metaclass__\ntry:\n    class C:\n        pass\nexcept TypeError:\n    print 'failed'\n",
            "import math as AssertionError\n\ndef f(x):\n    assert x\n",
        ] {
            assert!(library("m", code).lint(&["SCR102"]).is_empty(), "{code}");
        }
        // A class with bases does not read `__metaclass__` from the module.
        let code = "import math as __metaclass__\n\nclass C(object):\n    pass\n";
        assert_eq!(
            flagged(&library("m", code).lint(&["SCR102"])),
            ["__metaclass__"]
        );
        // Names in methods are not mangled: `__m` there is the global `__m`.
        let code =
            "import math as _D__m\n\nclass D(object):\n    def f(self):\n        return __m\n";
        assert_eq!(flagged(&library("m", code).lint(&["SCR102"])), ["_D__m"]);
    }

    /// Review round 25 (M1, both reviewers): in a module that fails to
    /// compile on its size before and after the fix, the net keeps the fix
    /// safe (the status stays "fails"), but `--fix`'s health check took
    /// `SCR002`'s message with another byte count for a new error and
    /// refused the fix as making the file worse. Both now compare size
    /// errors without their numbers (`size_error_key`): `check` reports the
    /// fix safe and `--fix` applies it (the Fixture checks that for every
    /// safe fix it reports).
    #[test]
    fn safe_fixes_in_modules_too_large_either_way_apply() {
        let code: String = (0..9000)
            .map(|i| format!("x{i} = 1   \n"))
            .collect::<String>()
            + "import math\n";
        let f = library("m", &code);
        for rules in [&["SCR102"][..], &["SCR002", "SCR102"]] {
            let d = f.lint(rules);
            let fixes: Vec<_> = d
                .iter()
                .filter_map(|d| d.fix.as_ref().map(|f| f.applicability))
                .collect();
            assert_eq!(fixes, [crate::Applicability::Safe], "{rules:?}");
        }
        assert!(safe_fix_applies(&f, "SCR102", "m"));
    }

    /// Review round 25 (M2, Opus): `importlib.import_module('__builtin__')`
    /// hands out the builtins like `__import__('__builtin__')`, and its
    /// `locals()` reads the caller's frame (`'os'` on 8.3.3's Jython), but
    /// it was only a first hop, so `os` was reported unused. Module loaders
    /// are judged like `__import__`, and the other ways to the builtins and
    /// the module table measured on 8.3.3 (`sys.builtins`,
    /// `sys.getDefaultBuiltins()`, a module's `sys` attribute) observe. A
    /// loader of a plain module keeps the finding; its fix is unsafe.
    #[test]
    fn module_loaders_that_hand_out_the_builtins_observe() {
        for reach in [
            "importlib.import_module('__builtin__').locals()",
            "importlib.import_module(name).locals()",
            "importlib.sys.modules['__builtin__'].locals()",
            "sys.builtins['locals']()",
            "sys.getDefaultBuiltins()['locals']()",
            "imp.load_source('x', path).f()",
        ] {
            let code = format!(
                "import sys, imp, importlib\nname = '__builtin__'\npath = ''\ndef f():\n    import os\n    return {reach}\n"
            );
            assert!(
                flagged(&library("m", &code).lint(&["SCR102"])).is_empty(),
                "{code}"
            );
        }
        let code = "import importlib\ndef f():\n    import os\n    return importlib.import_module('json')\n";
        let d = library("m", code).lint(&["SCR102"]);
        assert_eq!(flagged(&d), ["os"]);
        assert_eq!(
            d[0].fix.as_ref().unwrap().applicability,
            crate::Applicability::Unsafe
        );
    }
}
