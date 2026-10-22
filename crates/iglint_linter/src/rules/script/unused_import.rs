//! SCR102 unused-import.

use iglint_project::ScriptKind;
use iglint_python::ast::{Stmt, StmtImport, StmtImportFrom};
use iglint_python_semantic::{BindingId, BindingKind, ScopeId, SemanticModel};
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
  `exec`);
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
last one becomes `pass`). Safe, except for top-level imports of library
modules when iglint cannot see every use of the module's names (the module
is used as a value, references are computed at run time, the project is
inheritable or has scripts iglint does not model, or the file is linted
outside its project): then unsafe.

Top-level names of a library module are attributes of the module: an
import that another resource uses as `pkg.mod.name` is not reported.

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
        if b.scope == ScopeId::MODULE {
            if model.dunder_all_is_dynamic() || model.dunder_all().iter().any(|n| n == b.name) {
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

impl ScriptRule for UnusedImport {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let library = unit.script.kind == ScriptKind::LibraryModule;
        let text = source(unit);
        for (stmt, mut unused) in unused_imports(model, library) {
            // Top-level names of a library module are its attributes: other
            // resources may use them as `pkg.mod.name`.
            let mut applicability = Applicability::Safe;
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
                let shown = b.alias.map_or(b.name, |a| a.name.as_str());
                let r = report(cx, unit, b.range, format!("`{shown}` imported but unused"));
                let _ = r.fix_opt(fix.clone());
            }
        }
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
                "def f():\n    class C:\n        pass\n    def g():\n        pass\n    return C, g\n",
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
}
