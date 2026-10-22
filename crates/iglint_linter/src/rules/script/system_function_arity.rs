//! SCR202 system-function-arity.

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::system_api::arity_problems;
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR202",
    name: "system-function-arity",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`system.*` call with arguments that match no signature",
    explanation: r#"## What it does

Checks calls to `system.*` functions (also through aliases such as
`tag = system.tag`) against every signature the function has in the
scope the script runs in: too many or too few positional arguments, a
required argument missing, or (for functions written in Python) the same
argument given twice. Unknown keyword arguments are `SCR203`. A Java
`@KeywordArgs` function given the same argument positionally and by
keyword is not reported: some of them keep the keyword's value.

The signatures were measured on a real 8.3 gateway. Calls are not checked
when they unpack arguments (`f(*args)`, `f(**kwargs)`), when the stubs do
not know the function's parameters, or when a Java signature ends in an
array parameter (Jython may accept it as variable arguments). A keyword
parameter only counts as required when calling without it was observed
to fail.

Library modules run in the scope of their caller, so a call in a library
module is only reported when it fails in every scope where the function
exists.

## Why is this bad?

Jython raises `TypeError` when the call runs.

## Example

```python
def runAction(self, event):
    values = system.tag.readBlocking("[default]A", "[default]B", 5000)
```

Use instead:

```python
def runAction(self, event):
    values = system.tag.readBlocking(["[default]A", "[default]B"], 5000)
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&SystemFunctionArity)],
    options: None,
};

struct SystemFunctionArity;

impl ScriptRule for SystemFunctionArity {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        for p in arity_problems(model, unit) {
            if !p.keyword {
                report(cx, unit, p.range, p.message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn wrong_positional_counts() {
        let d = lint_module(
            "SCR202",
            "def f():\n    system.tag.readBlocking('a', 'b', 1, 2)\n    system.date.now(1)\n    system.tag.readBlocking(['a'])\n    system.date.now()\n",
        );
        assert_eq!(
            flagged(&d),
            [
                "system.tag.readBlocking('a', 'b', 1, 2)",
                "system.date.now(1)"
            ]
        );
        assert_eq!(
            messages(&d)[1],
            "Call to `system.date.now`: expected 0 arguments, got 1"
        );
    }

    #[test]
    fn missing_required_and_duplicates() {
        let d = view(
            ViewScript::Action,
            "\tsystem.perspective.openPopup(id\u{3d}'p')\n\tsystem.perspective.openPopup('p', 'V', id\u{3d}'x')\n\tsystem.perspective.openPopup('p', view\u{3d}'V')",
        )
        .lint(&["SCR202"]);
        assert_eq!(
            decoded_flagged(&d),
            ["system.perspective.openPopup(id='p')"]
        );
        assert!(messages(&d)[0].ends_with("missing required argument `view`"));
    }

    #[test]
    fn unknown_required_keywords_are_optional() {
        // Every parameter of `navigate` is optional as far as measured.
        assert!(
            view(ViewScript::Action, "\tsystem.perspective.navigate()")
                .lint(&["SCR202"])
                .is_empty()
        );
    }

    #[test]
    fn unpacking_and_unchecked_overloads_are_skipped() {
        let code = "def f(a, k):\n    system.date.now(*a)\n    system.date.now(**k)\n    system.util.parseTranslateArguments(1, 2, 3)\n    system.dataset.addRows(ds, [1], [2], [3])\n";
        assert!(lint_module("SCR202", code).is_empty());
    }

    #[test]
    fn keywords_are_left_to_scr203() {
        assert!(
            lint_module(
                "SCR202",
                "system.date.now(x=1)\nsystem.db.runPrepQuery('q', nope=1)\n"
            )
            .is_empty()
        );
    }

    #[test]
    fn libraries_need_a_failure_in_every_scope() {
        // `system.util.invokeLater` exists in the Vision Client and the
        // Designer only; a library call is checked against those scopes.
        assert!(
            lint_module("SCR202", "def f(g):\n    system.util.invokeLater(g, 100)\n").is_empty()
        );
        assert_eq!(
            flagged(&lint_module(
                "SCR202",
                "def f(g):\n    system.util.invokeLater(g, 100, 3)\n"
            )),
            ["system.util.invokeLater(g, 100, 3)"]
        );
    }

    #[test]
    fn per_scope_signatures() {
        // The gateway's `runNamedQuery` takes the project first (five
        // parameters), the Vision Client's does not (four).
        let five = "system.db.runNamedQuery('p', 'q', {}, 'tx', False)";
        assert!(lint_module("SCR202", &format!("{five}\n")).is_empty());
        assert!(
            view(ViewScript::Action, &format!("\t{five}"))
                .lint(&["SCR202"])
                .is_empty()
        );
        let six = "system.db.runNamedQuery('p', 'q', {}, 'tx', False, 1)";
        assert_eq!(flagged(&lint_module("SCR202", &format!("{six}\n"))), [six]);
    }

    #[test]
    fn every_location() {
        assert_eq!(
            flagged_everywhere("SCR202", "\tx \u{3d} system.date.addDays(1)"),
            ["system.date.addDays(1)"]
        );
        let d = gateway_message("def handleMessage(payload):\n\tsystem.util.getLogger()\n")
            .lint(&["SCR202"]);
        assert_eq!(flagged(&d), ["system.util.getLogger()"]);
        let d = tag_event("\tsystem.date.now(1)").lint(&["SCR202"]);
        assert_eq!(decoded_flagged(&d), ["system.date.now(1)"]);
        let d = session_event("def onStartup(session):\n\tsystem.date.now(1)\n").lint(&["SCR202"]);
        assert_eq!(flagged(&d), ["system.date.now(1)"]);
    }

    #[test]
    fn aliases() {
        let d = lint_module(
            "SCR202",
            "d = system.date\nfrom system.date import now\nd.now(1)\nnow(2)\n",
        );
        assert_eq!(flagged(&d), ["d.now(1)", "now(2)"]);
    }
}
