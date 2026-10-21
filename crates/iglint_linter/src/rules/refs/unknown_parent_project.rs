//! REF010 unknown-parent-project.

use iglint_project::ProjectId;
use iglint_project::workspace::ParentLink;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ProjectRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "REF010",
    name: "unknown-parent-project",
    category: Category::Ref,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`parent` names a project that does not exist or is not inheritable",
    explanation: r#"## What it does

Checks the `parent` of every `project.json`: the parent must be a project
next to it (in the same projects directory), it must be `inheritable`,
and following parents must not lead back to the project.

## Why is this bad?

The gateway refuses to load a project whose parent is missing or not
inheritable, and everything the project inherits (views, scripts, named
queries) is unavailable.

## Example

```json
{"title": "HMI", "parent": "global", "inheritable": false}
```

when `global/project.json` has `"inheritable": false`. Use instead:

```json
{"title": "Global", "inheritable": true}
```

in `global/project.json`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Project(&UnknownParent)],
    options: None,
};

struct UnknownParent;

impl ProjectRule for UnknownParent {
    fn check_project(&self, cx: &mut Checker<'_, '_>, project: ProjectId) {
        let ws = cx.workspace();
        let p = ws.project(project);
        let Some(parent) = p.manifest.doc.root().and_then(|r| r.get("parent")) else {
            return;
        };
        let range = parent.range;
        let message = match &p.parent {
            ParentLink::None => return,
            ParentLink::Missing(name) => format!("Parent project `{name}` does not exist"),
            ParentLink::Cycle(name) => {
                format!("Parent project `{name}` leads back to this project (inheritance cycle)")
            }
            ParentLink::Found(id) => {
                let parent = ws.project(*id);
                if leads_back(ws, project) {
                    format!(
                        "Parent project `{}` leads back to this project (inheritance cycle)",
                        parent.name
                    )
                } else if parent.inheritable() {
                    return;
                } else {
                    format!("Parent project `{}` is not inheritable", parent.name)
                }
            }
        };
        let file = p.manifest.file;
        cx.report_in(file, range, message).node_path("parent");
    }
}

/// Whether following found parents from `project` comes back to it.
fn leads_back(ws: &iglint_project::Workspace, project: ProjectId) -> bool {
    let mut at = project;
    for _ in 0..=ws.projects.len() {
        match &ws.project(at).parent {
            ParentLink::Found(p) if *p == project => return true,
            ParentLink::Found(p) => at = *p,
            ParentLink::Cycle(_) => return true,
            _ => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn child(parent: &str) -> Fixture {
        Fixture::new().file(
            "projects/p/project.json",
            &format!(
                r#"{{"title": "p", "parent": "{parent}", "enabled": true, "inheritable": false}}"#
            ),
        )
    }

    #[test]
    fn missing_not_inheritable_and_valid_parents() {
        let d = child("nope").lint(&["REF010"]);
        assert_eq!(messages(&d)[0].1, "Parent project `nope` does not exist");
        assert_eq!(flagged(&d), vec!["\"nope\""]);

        let f = child("base").file(
            "projects/base/project.json",
            r#"{"title": "base", "enabled": true, "inheritable": false}"#,
        );
        assert_eq!(
            messages(&f.lint(&["REF010"]))[0].1,
            "Parent project `base` is not inheritable"
        );

        let f = child("base").file(
            "projects/base/project.json",
            r#"{"title": "base", "enabled": true, "inheritable": true}"#,
        );
        assert!(f.lint(&["REF010"]).is_empty());
        assert!(child("").lint(&["REF010"]).is_empty());
    }

    #[test]
    fn cycles() {
        let f = child("q").file(
            "projects/q/project.json",
            r#"{"title": "q", "parent": "p", "enabled": true, "inheritable": true}"#,
        );
        let d = f.lint(&["REF010"]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().all(|d| d.message.contains("cycle")), "{d:?}");
    }
}
