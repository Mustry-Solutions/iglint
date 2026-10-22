//! SCR304 shadowed-library-package.

use iglint_project::model::ModelData;
use iglint_project::{LibraryLookup, ProjectId};
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "SCR304",
    name: "shadowed-library-package",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Library module with its own `code.py` hides its child modules",
    explanation: r#"## What it does

Reports a project library module that has its own `code.py` and also
child modules below its folder, for example `legacy/code.py` next to
`legacy/helpers/code.py`. The finding is on the parent's `code.py` and
lists the hidden children. Inheritance is taken into account: children
added by a child project of an inheritable project are found as well, as
long as the parent's module is not overridden there.

## Why is this bad?

A folder with its own `code.py` becomes a module, and Ignition does not
make its subfolders reachable through it. Verified on an 8.3 gateway:
`legacy.helpers` raises `AttributeError` and `import legacy.helpers`
raises `ImportError: No module named helpers`. The child modules exist in
the Designer but no script can call them.

## Example

```text
ignition/script-python/legacy/code.py
ignition/script-python/legacy/helpers/code.py
```

Use instead: move the code of `legacy/code.py` into a child module (for
example `legacy/core/code.py`) and delete `legacy/code.py`, so `legacy`
is a package folder again.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Resource(&ShadowedLibraryPackage)],
    options: None,
};

struct ShadowedLibraryPackage;

impl ResourceRule for ShadowedLibraryPackage {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::Library(module) = data else {
            return;
        };
        let (Some((index, project)), Some(resource)) = (cx.project_index(), cx.resource()) else {
            return;
        };
        let ws = cx.workspace();
        let mut projects: Vec<ProjectId> = vec![project];
        if ws.project(project).inheritable() {
            projects.extend(ws.project_descendants(project));
        }
        let mut hidden: Vec<String> = Vec::new();
        for p in projects {
            let tree = index.library(p);
            // Only where this resource is the module the project sees.
            let sees_this = matches!(
                tree.resolve(&module.module_path, |_| None),
                LibraryLookup::Module { resource: r, .. } if r == resource.id
            );
            if !sees_this {
                continue;
            }
            for (dotted, children) in tree.shadowed_packages() {
                if dotted == module.module_path {
                    hidden.extend(children.into_iter().map(str::to_owned));
                }
            }
        }
        hidden.sort();
        hidden.dedup();
        if hidden.is_empty() {
            return;
        }
        let list: Vec<String> = hidden.iter().map(|h| format!("`{h}`")).collect();
        let message = format!(
            "`{}` has its own `code.py`, which hides its child {} {}: no script can reach {}",
            module.module_path,
            if hidden.len() == 1 {
                "module"
            } else {
                "modules"
            },
            list.join(", "),
            if hidden.len() == 1 { "it" } else { "them" },
        );
        let text: &str = &module.script.source;
        let first_line = text.lines().next().unwrap_or("").trim_end_matches('\r');
        let range = TextRange::at(TextSize::ZERO, TextSize::of(first_line));
        let _ = cx
            .report_in(module.file, range, message)
            .node_path(module.script.node_path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;
    use crate::rules::test_support::{Fixture, messages};

    #[test]
    fn module_with_children() {
        let f = with_library(
            with_library(
                library("legacy", "def a(): pass\n"),
                "legacy/helpers",
                "def b(): pass\n",
            ),
            "legacy/more/deep",
            "x = 1\n",
        );
        let d = f.lint(&["SCR304"]);
        assert_eq!(d.len(), 1);
        assert!(d[0].file.path().ends_with("legacy/code.py"));
        assert_eq!(flagged(&d), ["def a(): pass"]);
        assert_eq!(
            messages(&d)[0].1,
            "`legacy` has its own `code.py`, which hides its child modules `legacy.helpers`, `legacy.more`: no script can reach them"
        );
    }

    #[test]
    fn package_folders_are_fine() {
        let f = with_library(library("core/a", "x = 1\n"), "core/b", "y = 1\n");
        assert!(f.lint(&["SCR304"]).is_empty());
    }

    #[test]
    fn empty_module_file() {
        let f = with_library(library("legacy", "\n"), "legacy/helpers", "def b(): pass\n");
        let d = f.lint(&["SCR304"]);
        assert_eq!(d.len(), 1);
        assert!(d[0].range.is_empty());
    }

    fn inheritance(child_overrides: bool) -> Fixture {
        let f = Fixture::new()
            .file(
                "projects/base/project.json",
                r#"{"title": "base", "enabled": true, "inheritable": true}"#,
            )
            .file(
                "projects/c/project.json",
                r#"{"title": "c", "enabled": true, "inheritable": false, "parent": "base"}"#,
            )
            .resource(
                "projects/base/ignition/script-python/legacy",
                &[("code.py", "def a(): pass\n")],
            )
            .resource(
                "projects/c/ignition/script-python/legacy/helpers",
                &[("code.py", "def b(): pass\n")],
            );
        if child_overrides {
            f.resource(
                "projects/c/ignition/script-python/legacy",
                &[("code.py", "def a(): return 1\n")],
            )
        } else {
            f
        }
    }

    #[test]
    fn children_from_a_child_project() {
        let d = inheritance(false).lint(&["SCR304"]);
        assert_eq!(d.len(), 1);
        assert!(
            d[0].file
                .path()
                .ends_with("base/ignition/script-python/legacy/code.py")
        );
        // The child overrides the module: reported on the child's copy.
        let d = inheritance(true).lint(&["SCR304"]);
        assert_eq!(d.len(), 1);
        assert!(
            d[0].file
                .path()
                .ends_with("c/ignition/script-python/legacy/code.py")
        );
    }
}
