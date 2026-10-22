//! SCR206 legacy-script-prefix.

use iglint_python::ast::{Expr, ExprContext};
use iglint_python_semantic::Resolution;
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{fix, report, semantic, source, walk_exprs};

pub static META: RuleMeta = RuleMeta {
    code: "SCR206",
    name: "legacy-script-prefix",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "`project.`, `shared.` or `app.` prefix that does not exist in 8.3",
    explanation: r#"## What it does

Reports `project.x`, `shared.x` and `app.x` where `project`, `shared` or
`app` is not defined anywhere the script can see. These were the script
library prefixes of Ignition 7 (`app.` the even older application
scripts). In 8.3 every top-level package of the project library is a
global of its own, and the prefixes are not defined: verified on an 8.3
gateway, `project.x` and `shared.x` raise `NameError`.

Nothing is reported when the name is bound (a variable, parameter, import
or a library package actually called `project`), when the script's
library is not fully known (a parent project outside the linted tree, tag
scripts of gateway resources), or inside a `try` that catches
`NameError`.

## Why is this bad?

The script raises `NameError` when it reaches the reference. Code
migrated from Ignition 7 often still carries these prefixes in places
that rarely run, such as error handlers.

## Example

```python
def runAction(self, event):
    project.util.refresh(self)
```

Use instead:

```python
def runAction(self, event):
    util.refresh(self)
```

## Fix

Removes the prefix when the name after it is a top-level package or
module of the project's library (with inheritance) and the script does
not bind that name itself. Unsafe: in Ignition 7, `shared.x` and
`project.x` were different libraries, so `x` in the 8.3 project may not be
the code the prefix meant.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&LegacyScriptPrefix)],
    options: None,
};

struct LegacyScriptPrefix;

impl ScriptRule for LegacyScriptPrefix {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let text = source(unit);
        // (attribute range, prefix range including the dot, prefix, name)
        let mut found: Vec<(TextRange, Option<TextRange>, String, String)> = Vec::new();
        walk_exprs(&unit.parsed.module.body, &mut |e| {
            let Expr::Attribute(a) = e else { return };
            let Expr::Name(n) = &*a.value else { return };
            if n.ctx != ExprContext::Load || !matches!(&*n.id, "project" | "shared" | "app") {
                return;
            }
            let Some(reference) = model.reference_at(n.range) else {
                return;
            };
            if reference.resolution != Resolution::Unresolved || reference.name_error_guarded {
                return;
            }
            let dot = TextRange::new(n.range.end(), a.attr.range.start());
            let prefix = (&text[dot.to_std()] == ".")
                .then(|| TextRange::new(n.range.start(), n.range.end() + TextSize::new(1)));
            found.push((a.range, prefix, n.id.to_string(), a.attr.id.to_string()));
        });
        if found.is_empty() {
            return;
        }
        let library = cx
            .project_index()
            .map(|(index, project)| index.library(project));
        for (range, prefix, root, name) in found {
            let message = if root == "app" {
                format!(
                    "`app.{name}`: the `app` prefix of legacy application scripts does not exist in Ignition 8.3 (`NameError`)"
                )
            } else {
                format!(
                    "`{root}.{name}`: the `{root}` prefix does not exist in Ignition 8.3 (`NameError`); top-level library packages are globals"
                )
            };
            let fixable = prefix.filter(|_| {
                library.is_some_and(|l| l.is_global(&name))
                    && !model
                        .bindings()
                        .any(|(_, b)| b.name == name && !b.is_implicit())
            });
            let fix = fixable.and_then(|p| {
                fix(
                    cx,
                    unit,
                    format!("Remove the `{root}.` prefix"),
                    Applicability::Unsafe,
                    vec![(p, String::new())],
                )
            });
            let _ = report(cx, unit, range, message).fix_opt(fix);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;
    use crate::rules::test_support::Fixture;

    #[test]
    fn reports_legacy_prefixes() {
        let d = lint_module(
            "SCR206",
            "def f():\n    project.util.go()\n    shared.db.q()\n    return app.x\n",
        );
        assert_eq!(flagged(&d), ["project.util", "shared.db", "app.x"]);
        assert_eq!(
            messages(&d)[0],
            "`project.util`: the `project` prefix does not exist in Ignition 8.3 (`NameError`); top-level library packages are globals"
        );
    }

    #[test]
    fn bound_names_are_not_reported() {
        let code = "def f(project):\n    return project.name\nshared = {}\nx = shared.keys()\nimport app\napp.run()\ntry:\n    project.y\nexcept NameError:\n    pass\n";
        assert!(lint_module("SCR206", code).is_empty());
        // A library package called `shared`.
        let f = with_library(
            library("m", "shared.util.f()\n"),
            "shared/util",
            "def f(): pass\n",
        );
        assert!(f.lint(&["SCR206"]).is_empty());
    }

    #[test]
    fn fix_removes_the_prefix_for_library_packages() {
        let f = with_library(
            library(
                "m",
                "def f():\n    project.util.go()\n    shared.other.x()\n    project . util\n",
            ),
            "util",
            "def go(): pass\n",
        );
        let d = f.lint(&["SCR206"]);
        assert_eq!(d.len(), 3);
        assert!(d[0].fix.is_some());
        // `other` is not a library package; spaced text is left alone.
        assert!(d[1].fix.is_none() && d[2].fix.is_none());
        let fixed = f.fixed(&[("SCR206", "")], true, &library_path(&f, "m"));
        assert_eq!(
            fixed,
            "def f():\n    util.go()\n    shared.other.x()\n    project . util\n"
        );
    }

    #[test]
    fn no_fix_when_the_name_is_bound_locally() {
        let f = with_library(
            library("m", "def f(util):\n    return project.util.go()\n"),
            "util",
            "def go(): pass\n",
        );
        let d = f.lint(&["SCR206"]);
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn fix_in_json_escaped_scripts() {
        let f = with_library(
            view(
                ViewScript::Action,
                "\tx \u{3d} '<&>'\n\tproject.util.go(self)",
            ),
            "util",
            "def go(c): pass\n",
        );
        let d = f.lint(&["SCR206"]);
        assert_eq!(decoded_flagged(&d), ["project.util"]);
        let fixed = f.fixed(&[("SCR206", "")], true, &view_path(&f));
        let expected = json("\tx = '<&>'\n\tutil.go(self)");
        assert!(expected.contains("\\u003c"));
        assert!(fixed.contains(&expected), "{fixed}");
    }

    #[test]
    fn every_location() {
        assert_eq!(
            flagged_everywhere("SCR206", "\tshared.tools.run()"),
            ["shared.tools"]
        );
        let d = gateway_message("def handleMessage(payload):\n\tproject.handlers.go(payload)\n")
            .lint(&["SCR206"]);
        assert_eq!(flagged(&d), ["project.handlers"]);
        let d =
            session_event("def onStartup(session):\n\tshared.s.go(session)\n").lint(&["SCR206"]);
        assert_eq!(flagged(&d), ["shared.s"]);
        // Tag scripts of gateway resources see an unknown library.
        assert!(tag_event("\tshared.t.go()").lint(&["SCR206"]).is_empty());
    }

    #[test]
    fn unknown_parent_library_is_silent() {
        let f = Fixture::new()
            .file(
                "projects/c/project.json",
                r#"{"title": "c", "enabled": true, "inheritable": false, "parent": "missing"}"#,
            )
            .resource(
                "projects/c/ignition/script-python/m",
                &[("code.py", "project.x.y()\n")],
            );
        assert!(f.lint(&["SCR206"]).is_empty());
    }
}
