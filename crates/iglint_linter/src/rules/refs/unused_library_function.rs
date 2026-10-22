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
(passed to a function, `getattr(core.util, name)`), star-imported, or a
project with non-literal `runScript`, `__import__`, `eval` or `exec`
targets. It also stays silent in inheritable projects, in projects whose
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
        let ws = cx.workspace();
        if ws.project(project).inheritable()
            || !ws.project(project).inheritance.complete
            || super::has_unmodelled_scripts(ws, project)
            || index.module_escapes(project, resource.id)
            || index
                .dynamic_references(project)
                .library
                .iter()
                .any(String::is_empty)
        {
            return;
        }
        let Some(symbols) = index.module_symbols(resource.id) else {
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
        let f = with_library(
            library(
                "app/main",
                "def go(name):\n    return getattr(core.util, name)()\n",
            ),
            "core/util",
            UTIL,
        );
        // Only the caller itself is unused.
        assert_eq!(flagged(&f.lint(&["REF012"])), vec!["go"]);
        let f = with_library(
            library("app/main", "def go(name):\n    return eval(name)\n"),
            "core/util",
            UTIL,
        );
        assert!(f.lint(&["REF012"]).is_empty());
    }
}
