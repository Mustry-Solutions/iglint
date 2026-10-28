//! SCR204 deprecated-system-function.

use iglint_python::ast::{Expr, ExprContext};
use iglint_stubs::{Scope, Scopes};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::system_api::{scope_names, script_scope, system_path};
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

A replacement is only suggested where it exists: a gateway script that
calls `system.util.beep` is told that `system.vision.beep` exists only in
the Vision Client and the Designer. A library module runs in the scope of
whatever imports it, so its message says where a replacement exists when
that is not everywhere (`system.security.getUsername` and its replacement
`system.vision.getUsername` both exist only in the Vision Client and the
Designer).

Some replacements Ignition names are not `system.*` functions:
`system.net.httpGet` and the other `system.net.http*` functions are
replaced by the client `system.net.httpClient()` returns, and the message
names that method (`.get(...)`).

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

/// A replacement as the message names it, and where it exists (`None`
/// when the stubs cannot tell).
struct Replacement {
    /// Markdown for the message: `` `system.tag.readBlocking` ``.
    text: String,
    scopes: Option<Scopes>,
}

impl Replacement {
    /// The replacement `name` as Ignition or the documentation spells it.
    /// Ignition's deprecation notes name some replacements that are not
    /// `system.*` members:
    ///
    /// * a method of an object a `system.*` function returns
    ///   (`system.net.httpClient.get` for `system.net.httpGet`): the
    ///   message names the function that makes the object and the method;
    /// * a function with other capitalisation
    ///   (`system.vision.isTouchscreenMode`, which 8.3 spells
    ///   `isTouchScreenMode`): the message uses the member's own name.
    fn resolve(name: &str) -> Replacement {
        if let Some(member) = iglint_stubs::system_member(name) {
            return Replacement {
                text: format!("`{}`", member.path()),
                scopes: Some(member.scopes()),
            };
        }
        if let Some((parent, method)) = name.rsplit_once('.') {
            if let Some(object) = iglint_stubs::system_object(parent) {
                return Replacement {
                    text: format!("`{parent}()` and its `.{method}(...)` method"),
                    scopes: Some(object.scopes()),
                };
            }
            if let Some(member) =
                iglint_stubs::system_members(parent).find(|m| m.path().eq_ignore_ascii_case(name))
            {
                return Replacement {
                    text: format!("`{}`", member.path()),
                    scopes: Some(member.scopes()),
                };
            }
        }
        Replacement {
            text: format!("`{name}`"),
            scopes: None,
        }
    }
}

/// The end of the message for a deprecated function that exists in
/// `deprecated` and is used in `scope` (`None` in a library module), with
/// the replacements `names` and the `note` on how the call changes.
///
/// A replacement is only suggested where it exists: in a script with a
/// known scope, the replacements missing there are left out, and when
/// none is left the message says where they exist instead. A library
/// module runs wherever it is imported from, so each replacement that
/// does not exist everywhere says where it does (`system.vision.*` only
/// in the Vision Client and the Designer).
fn replacement_advice(
    names: &[&str],
    note: &str,
    scope: Option<Scope>,
    deprecated: Scopes,
) -> String {
    if names.is_empty() {
        return String::new();
    }
    let all: Vec<Replacement> = names.iter().map(|n| Replacement::resolve(n)).collect();
    let joined = |replacements: &[&Replacement]| -> String {
        replacements
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join(" or ")
    };
    let mut notes: Vec<String> = Vec::new();
    if !note.is_empty() {
        notes.push(note.to_owned());
    }
    let suggested: Vec<&Replacement> = match scope {
        Some(scope) => {
            let here: Vec<&Replacement> = all
                .iter()
                .filter(|r| r.scopes.is_none_or(|s| s.contains(scope)))
                .collect();
            if here.is_empty() {
                let (subject, verb) = if all.len() == 1 {
                    ("its replacement", "exists")
                } else {
                    ("its replacements", "exist")
                };
                let elsewhere = all
                    .iter()
                    .filter_map(|r| r.scopes)
                    .fold(Scopes::NONE, Scopes::union);
                return format!(
                    "; {subject} {} {verb} only in {}, not in {}",
                    joined(&all.iter().collect::<Vec<_>>()),
                    scope_names(elsewhere),
                    scope_names(scope.into())
                );
            }
            here
        }
        None => {
            for r in &all {
                let Some(scopes) = r.scopes else { continue };
                if scopes == Scopes::ALL {
                    continue;
                }
                let subject = if all.len() > 1 {
                    r.text.clone()
                } else if scopes == deprecated {
                    "both".to_owned()
                } else {
                    "it".to_owned()
                };
                notes.push(format!(
                    "{subject} {} only in {}",
                    if subject == "both" { "exist" } else { "exists" },
                    scope_names(scopes)
                ));
            }
            all.iter().collect()
        }
    };
    let mut text = format!("; use {} instead", joined(&suggested));
    if !notes.is_empty() {
        text.push_str(&format!(" ({})", notes.join("; ")));
    }
    text
}

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
            // The replacement Ignition names, else the documented one with
            // how the call changes.
            let (names, note): (Vec<&str>, &str) = match (replacement, known) {
                (Some(replacement), _) => (
                    replacement
                        .split(',')
                        .map(str::trim)
                        .filter(|r| !r.is_empty())
                        .collect(),
                    "",
                ),
                (None, Some((_, to, note))) => (vec![*to], *note),
                (None, None) => (Vec::new(), ""),
            };
            message.push_str(&replacement_advice(&names, note, scope, function.scopes()));
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

    /// A replacement is only suggested where it exists (reviewer report:
    /// `system.vision.getUsername` suggested for a library module that
    /// runs on the gateway).
    #[test]
    fn replacements_that_exist_in_fewer_scopes() {
        // A library module runs wherever it is imported from: say where
        // the replacement exists.
        let d = lint_module(
            "SCR204",
            "a = system.security.getUsername()\nsystem.util.beep()\nb = system.tag.readAll(['x'])\n",
        );
        assert_eq!(
            messages(&d),
            [
                "`system.security.getUsername` is deprecated; use `system.vision.getUsername` instead (both exist only in the Vision Client and the Designer)",
                "`system.util.beep` is deprecated; use `system.vision.beep` instead (it exists only in the Vision Client and the Designer)",
                "`system.tag.readAll` is deprecated; use `system.tag.readBlocking` instead",
            ]
        );
        // On the gateway the Vision replacement does not exist: it is not
        // suggested.
        let d = gateway_timer("def handleTimerEvent():\n\tsystem.util.beep()\n").lint(&["SCR204"]);
        assert_eq!(
            messages(&d),
            [
                "`system.util.beep` is deprecated; its replacement `system.vision.beep` exists only in the Vision Client and the Designer, not in the gateway"
            ]
        );
        let d = webdev("def doGet(request, session):\n\tsystem.util.playSoundClip('a.wav')\n")
            .lint(&["SCR204"]);
        assert_eq!(
            messages(&d),
            [
                "`system.util.playSoundClip` is deprecated; its replacement `system.vision.playSoundClip` exists only in the Vision Client and the Designer, not in the gateway"
            ]
        );
    }

    /// Replacements that are a method of the object a function returns,
    /// or that Ignition spells with other capitalisation.
    #[test]
    fn replacements_that_are_not_system_functions() {
        let d = lint_module(
            "SCR204",
            "system.net.httpGet('https://x')\nsystem.net.httpPut('https://x', 'a')\nsystem.gui.isTouchscreenModeEnabled()\n",
        );
        assert_eq!(
            messages(&d),
            [
                "`system.net.httpGet` is deprecated; use `system.net.httpClient()` and its `.get(...)` method instead",
                "`system.net.httpPut` is deprecated; use `system.net.httpClient()` and its `.put(...)` method instead",
                "`system.gui.isTouchscreenModeEnabled` is deprecated; use `system.vision.isTouchScreenMode` instead (both exist only in the Vision Client and the Designer)",
            ]
        );
    }

    /// Every replacement Ignition names is a `system.*` member or a
    /// method of an object a `system.*` function returns.
    #[test]
    fn named_replacements_resolve() {
        let mut checked = 0;
        for package in iglint_stubs::system_packages() {
            for member in iglint_stubs::system_members(package.path) {
                let iglint_stubs::Member::Function(function) = member else {
                    continue;
                };
                for v in function.variants {
                    for name in v.replacement.iter().flat_map(|r| r.split(',')) {
                        let r = super::Replacement::resolve(name.trim());
                        assert!(r.scopes.is_some(), "{}: {}", function.path, r.text);
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 50, "{checked}");
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
