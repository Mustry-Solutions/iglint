//! REF012 unused-library-function.

use iglint_project::library::SymbolKind;
use iglint_project::model::ModelData;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF012",
    name: "unused-library-function",
    category: Category::Ref,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Library function is never referenced",
    explanation: r#"## What it does

Reports top-level functions of project library modules that nothing in
the project references: no script (views, event scripts, tag event
scripts of the gateway, other library modules, the module itself) uses
`core.util.fn`, `from core.util import fn` or an alias of it, and no
expression calls `runScript("core.util.fn")`.

The rule stays silent for a module whose names may be reached in ways
that cannot be followed: the module (or its package) used as a value
(passed to a function, `getattr(core.util, name)`, `vars(core.util)`,
`core.util.__dict__`), star-imported, a
module that may rebind its own names at run time (`exec`, writes through
`globals()`, code that reads or writes a frame's or a module's names by
itself such as `sys._getframe()` or `e = eval`; reflection that is only a
first hop, such as a `.type` Java member or `getattr` with a computed
name, does not count, see `docs/internal/fix-safety.md`, "Frame routes:
findings and fixes"), a function that a later `from x import *` may
overwrite, or a project with non-literal `runScript`, `__import__`,
`eval` or `exec` targets. It also stays silent in inheritable projects, in projects whose
parent is not in the workspace and in projects with resources whose
scripts are not analysed (Vision windows and templates, reports, alarm
pipelines, SFC charts, event streams).

## Why is this bad?

Unused functions are dead code that still has to be read and maintained.
This rule is not in the default set: functions are also called from other
projects through the gateway and by name from configuration.

## Example

A function `core.util.oldFormat` that no script calls any more. Delete
it.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&UnusedLibraryFunction)],
    options: None,
};

struct UnusedLibraryFunction;

impl ResourceRule for UnusedLibraryFunction {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Library(module) = data else {
            return;
        };
        let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
            return;
        };
        if !super::all_library_references_known(cx.workspace(), index, project, resource.id) {
            return;
        }
        // A module whose only escape routes do not observe names (a
        // reflective Java member such as `.type`) keeps its functions here.
        let Some(symbols) = index.module_symbols(resource.id).map(|s| s.for_findings()) else {
            return;
        };
        for symbol in &symbols.symbols {
            if !matches!(symbol.kind, SymbolKind::Function(_))
                || index.symbol_usage_count(resource.id, &symbol.name) > 0
            {
                continue;
            }
            cx.report_in(
                module.file,
                symbol.range,
                format!(
                    "Library function `{}.{}` is never referenced",
                    module.module_path, symbol.name
                ),
            )
            .node_path(format!("{}.{}", module.module_path, symbol.name));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{ViewScript, flagged, library, view, with_library};

    const UTIL: &str = "def used(a):\n    return helper(a)\ndef helper(a):\n    return a\ndef unused():\n    pass\ndef imported():\n    pass\ndef by_expression():\n    pass\ndef callback():\n    pass\n";

    #[test]
    fn unreferenced_functions() {
        let f = with_library(
            view(
                ViewScript::Action,
                "\tcore.util.used(1)\n\tsystem.util.invokeAsynchronous(core.util.callback)",
            ),
            "core/util",
            UTIL,
        );
        let f = with_library(f, "app/main", "from core.util import imported\n");
        let f = f.with_view(
            "Expr",
            r#"{"root": {"type": "ia.display.label", "meta": {"name": "root"}, "props": {"text": ""},
                "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "runScript('core.util.by_expression')"}}}}}}"#,
        );
        assert_eq!(flagged(&f.lint(&["REF012"])), vec!["unused"]);
    }

    #[test]
    fn escaped_modules_are_skipped() {
        // Review round 25 (Opus, N2): the namespace of a module read
        // through `__dict__` from another module is the module used as a
        // value, like `vars(m)` and `getattr(m, name)`: `m.helper` and
        // `m`'s import are not reported (a function's own `__dict__` is
        // not its module's namespace).
        for user in [
            "def g(k):\n    return m.__dict__[k]\n",
            "def g(k):\n    return vars(m)[k]\n",
            "def g(k):\n    return getattr(m, k)\n",
            "def g(k):\n    m.__dict__[k] = 1\n",
        ] {
            let f = with_library(
                library("m", "import os\ndef helper():\n    return 1\n"),
                "n",
                user,
            );
            assert_eq!(flagged(&f.lint(&["REF012", "SCR102"])), vec!["g"], "{user}");
        }
        let f = with_library(
            library("m", "import os\ndef helper():\n    return 1\n"),
            "n",
            "def g():\n    return m.helper.__dict__\n",
        );
        assert_eq!(flagged(&f.lint(&["REF012", "SCR102"])), vec!["os", "g"]);
        // The target module is passed as a value: its functions may be
        // reached any way, but the caller itself is not introspective, so
        // its own unused function is still reported.
        let f = with_library(
            library("app/main", "def go():\n    return send(core.util)\n"),
            "core/util",
            UTIL,
        );
        assert_eq!(flagged(&f.lint(&["REF012"])), vec!["go"]);
        // A reflective accessor with a computed name is a first hop that
        // does not observe names by itself (`dynamic::Route::FixOnly`):
        // the caller's own unused function is still reported (review
        // round 24), while `core.util`, used as a value, is skipped.
        let f = with_library(
            library(
                "app/main",
                "def go(name):\n    return getattr(core.util, name)()\n",
            ),
            "core/util",
            UTIL,
        );
        assert_eq!(flagged(&f.lint(&["REF012"])), vec!["go"]);
        // Under another name the accessor hides its calls: nothing in the
        // module is reported.
        let f = with_library(
            library(
                "app/main",
                "get = getattr\ndef go(name):\n    return get(core.util, name)()\n",
            ),
            "core/util",
            UTIL,
        );
        assert!(f.lint(&["REF012"]).is_empty());
        let f = with_library(
            library("app/main", "def go(name):\n    return eval(name)\n"),
            "core/util",
            UTIL,
        );
        assert!(f.lint(&["REF012"]).is_empty());
    }
}
