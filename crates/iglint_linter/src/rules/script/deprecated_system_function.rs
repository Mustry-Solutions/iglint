//! SCR204 deprecated-system-function.

use iglint_python::ast::{Expr, ExprContext};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::system_api::{script_scope, system_path};
use crate::rules::script::{report, semantic, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR204",
    name: "deprecated-system-function",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Deprecated `system.*` function",
    explanation: r#"## What it does

Reports uses of `system.*` functions that Ignition 8.3 marks as deprecated
(also through aliases), with the replacement Ignition names when it names
one, or else the one Inductive Automation's documentation names (for
example `system.tag.readBlocking` for `system.tag.read`, with how the call
changes). `system.dataset.toPyDataSet` and a few others have no
replacement in 8.3. Deprecation is taken from the scope the script runs in; a library
module is only reported when the function is deprecated in every scope
where it exists.

## Why is this bad?

Deprecated functions still work, but they are kept only for
compatibility: they may be removed in a later version, some no longer
receive fixes, and many have a replacement that is faster or works in
more scopes (for example `system.tag.readBlocking` instead of
`system.tag.read`).

## Example

```python
def runAction(self, event):
    value = system.tag.read("[default]Line/Speed").value
```

Use instead:

```python
def runAction(self, event):
    value = system.tag.readBlocking(["[default]Line/Speed"])[0].value
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&DeprecatedSystemFunction)],
    options: None,
};

struct DeprecatedSystemFunction;

/// Replacements for deprecated functions that Ignition does not name
/// itself, from Inductive Automation's 8.3 documentation; each exists in
/// the 8.3 stubs and is not deprecated (checked by a test). The note
/// says how the call must change.
const KNOWN_REPLACEMENTS: &[(&str, &str, &str)] = &[
    (
        "system.tag.read",
        "system.tag.readBlocking",
        "it takes a list of paths and returns a list of qualified values",
    ),
    ("system.tag.readAll", "system.tag.readBlocking", ""),
    (
        "system.tag.getTagValue",
        "system.tag.readBlocking",
        "it takes a list of paths and returns a list of qualified values",
    ),
    ("system.tag.getTagValues", "system.tag.readBlocking", ""),
    (
        "system.tag.write",
        "system.tag.writeAsync",
        "it takes lists of paths and values; `system.tag.writeBlocking` waits for the result",
    ),
    (
        "system.tag.writeAll",
        "system.tag.writeAsync",
        "`system.tag.writeBlocking` waits for the result",
    ),
    (
        "system.tag.writeToTag",
        "system.tag.writeAsync",
        "it takes lists of paths and values",
    ),
    ("system.tag.writeToTags", "system.tag.writeAsync", ""),
    (
        "system.tag.writeAllSynchronous",
        "system.tag.writeBlocking",
        "",
    ),
    (
        "system.tag.writeToTagSynchronous",
        "system.tag.writeBlocking",
        "it takes lists of paths and values",
    ),
    (
        "system.tag.browseTags",
        "system.tag.browse",
        "it returns browse results, not tag objects",
    ),
    (
        "system.tag.browseTagsSimple",
        "system.tag.browse",
        "it returns browse results, not tag objects",
    ),
    (
        "system.tag.addTag",
        "system.tag.configure",
        "it takes a base path and a list of tag dictionaries",
    ),
    (
        "system.tag.editTag",
        "system.tag.configure",
        "it takes a base path and a list of tag dictionaries",
    ),
    (
        "system.tag.editTags",
        "system.tag.configure",
        "it takes a base path and a list of tag dictionaries",
    ),
    (
        "system.tag.removeTag",
        "system.tag.deleteTags",
        "it takes a list of paths",
    ),
    ("system.tag.removeTags", "system.tag.deleteTags", ""),
    ("system.tag.loadFromFile", "system.tag.importTags", ""),
    ("system.dataset.toDataSet", "system.dataset.toDataset", ""),
    ("system.db.toDataSet", "system.dataset.toDataset", ""),
    ("system.dataset.dataSetToCSV", "system.dataset.toCSV", ""),
    ("system.db.dataSetToCSV", "system.dataset.toCSV", ""),
    ("system.gui.openWindow", "system.vision.openWindow", ""),
    ("system.gui.closeWindow", "system.vision.closeWindow", ""),
    (
        "system.gui.closeParentWindow",
        "system.vision.closeParentWindow",
        "",
    ),
    ("system.gui.swapWindow", "system.vision.swapWindow", ""),
];

impl ScriptRule for DeprecatedSystemFunction {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let scope = script_scope(unit);
        let mut found = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Attribute(a) = e else { return };
            if a.ctx != ExprContext::Load {
                return;
            }
            let Some(path) = system_path(model, e) else {
                return;
            };
            let Some(function) = iglint_stubs::system_function(&path) else {
                return;
            };
            let (deprecated, replacement) = match scope {
                Some(s) => match function.variant(s) {
                    Some(v) => (v.deprecated, v.replacement),
                    // Not available here at all: `SCR205`.
                    None => return,
                },
                None => (function.is_deprecated(), function.replacement()),
            };
            if !deprecated {
                return;
            }
            let mut message = format!("`{path}` is deprecated");
            let known = KNOWN_REPLACEMENTS.iter().find(|(from, _, _)| *from == path);
            if let (None, Some((_, to, note))) = (replacement, known) {
                message.push_str(&format!("; use `{to}` instead"));
                if !note.is_empty() {
                    message.push_str(&format!(" ({note})"));
                }
            } else if let Some(replacement) = replacement {
                let names: Vec<String> = replacement
                    .split(',')
                    .map(|r| format!("`{}`", r.trim()))
                    .collect();
                message.push_str(&format!("; use {} instead", names.join(" or ")));
            }
            found.push((a.range, message));
        });
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn deprecated_with_and_without_replacement() {
        let d = lint_module(
            "SCR204",
            "a = system.tag.read('x')\nb = system.tag.queryTagHistory(paths=['x'])\nc = system.tag.readBlocking(['x'])\n",
        );
        assert_eq!(
            flagged(&d),
            ["system.tag.read", "system.tag.queryTagHistory"]
        );
        assert_eq!(
            messages(&d),
            [
                "`system.tag.read` is deprecated; use `system.tag.readBlocking` instead (it takes a list of paths and returns a list of qualified values)",
                "`system.tag.queryTagHistory` is deprecated; use `system.historian.queryRawPoints` instead"
            ]
        );
    }

    #[test]
    fn several_replacements() {
        let d = lint_module("SCR204", "system.db.runQuery('q')\n");
        assert_eq!(
            messages(&d),
            [
                "`system.db.runQuery` is deprecated; use `system.db.execQuery` or `system.db.runPrepQuery` instead"
            ]
        );
    }

    #[test]
    fn aliases_and_references_without_call() {
        let d = lint_module(
            "SCR204",
            "t = system.tag\nfrom system.tag import writeAll\nf = t.read\nwriteAll\n",
        );
        assert_eq!(flagged(&d), ["t.read"]);
    }

    #[test]
    fn functions_missing_in_the_scope_are_left_to_scr205() {
        // `system.util.invokeLater` is deprecated where it exists (Vision),
        // but does not exist in Perspective.
        assert!(
            view(ViewScript::Action, "\tsystem.util.invokeLater(f)")
                .lint(&["SCR204"])
                .is_empty()
        );
        assert_eq!(
            flagged(&lint_module("SCR204", "system.util.invokeLater(f)\n")),
            ["system.util.invokeLater"]
        );
    }

    #[test]
    fn every_location() {
        assert_eq!(
            flagged_everywhere("SCR204", "\tx \u{3d} system.tag.read('a')"),
            ["system.tag.read"]
        );
        let d = tag_event("\tsystem.tag.write('a', 1)").lint(&["SCR204"]);
        assert_eq!(decoded_flagged(&d), ["system.tag.write"]);
        let d = gateway_timer("def handleTimerEvent():\n\tsystem.tag.writeAll(['a'], [1])\n")
            .lint(&["SCR204"]);
        assert_eq!(flagged(&d), ["system.tag.writeAll"]);
    }

    #[test]
    fn known_replacements_exist_and_are_current() {
        for (from, to, _) in super::KNOWN_REPLACEMENTS {
            let old = iglint_stubs::system_function(from).unwrap_or_else(|| panic!("{from}"));
            assert!(old.is_deprecated(), "{from} is not deprecated");
            assert_eq!(old.replacement(), None, "{from}: Ignition names one");
            let new = iglint_stubs::system_function(to).unwrap_or_else(|| panic!("{to}"));
            assert!(!new.is_deprecated(), "{to} is deprecated");
        }
    }
}
