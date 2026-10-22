//! SCR203 unknown-system-keyword.

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::system_api::arity_problems;
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR203",
    name: "unknown-system-keyword",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Keyword argument a `system.*` function does not accept",
    explanation: r#"## What it does

Reports keyword arguments to `system.*` functions that the function does
not accept in the scope the script runs in: a keyword that is not a
parameter name, or any keyword passed to a function implemented as a
plain Java method (those take positional arguments only).

The same exclusions as `SCR202` apply: calls that unpack arguments,
functions whose parameters the stubs do not know, and, in library
modules, calls that are valid in at least one scope where the function
exists.

## Why is this bad?

Jython raises `TypeError` when the call runs ("unexpected keyword
argument" or "takes no keyword arguments").

## Example

```python
def runAction(self, event):
    data = system.db.runPrepQuery("SELECT * FROM t WHERE id = ?", [1], db="Plant")
```

Use instead:

```python
def runAction(self, event):
    data = system.db.runPrepQuery("SELECT * FROM t WHERE id = ?", [1], database="Plant")
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&UnknownSystemKeyword)],
    options: None,
};

struct UnknownSystemKeyword;

impl ScriptRule for UnknownSystemKeyword {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        for p in arity_problems(model, unit) {
            if p.keyword {
                report(cx, unit, p.range, p.message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn unknown_keywords() {
        let d = lint_module(
            "SCR203",
            "system.db.runPrepQuery('q', [1], db='x')\nsystem.db.runPrepQuery('q', [1], database='x')\n",
        );
        assert_eq!(flagged(&d), ["db"]);
        assert_eq!(
            messages(&d),
            ["Call to `system.db.runPrepQuery`: unexpected keyword argument `db`"]
        );
    }

    #[test]
    fn keywords_to_positional_functions() {
        let d = lint_module("SCR203", "system.date.addDays(date=d, value=1)\n");
        assert_eq!(flagged(&d), ["date"]);
        assert!(messages(&d)[0].contains("takes no keyword arguments"));
    }

    #[test]
    fn counts_are_left_to_scr202() {
        assert!(lint_module("SCR203", "system.date.now(1)\n").is_empty());
    }

    #[test]
    fn every_location() {
        assert_eq!(
            flagged_everywhere("SCR203", "\tsystem.perspective.navigate(pag\u{3d}'/x')"),
            ["pag"]
        );
        let d =
            webdev("def doGet(request, session):\n\tsystem.tag.readBlocking(['a'], timeOut=5)\n")
                .lint(&["SCR203"]);
        assert_eq!(flagged(&d), ["timeOut"]);
    }
}
