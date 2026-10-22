//! SCR117 redefined-builtin.

use iglint_python_semantic::{BindingKind, ScopeKind};
use iglint_text::TextRange;
use rustc_hash::FxHashSet;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR117",
    name: "redefined-builtin",
    category: Category::Script,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Name shadows a builtin or Ignition's `system`",
    explanation: r#"## What it does

Reports assignments, parameters, loop variables, functions, classes and
imports whose name is a Jython builtin (`id`, `type`, `input`, `file`,
`max`, ...) or Ignition's `system`, once per name and scope. Class
attributes and methods are not reported: methods do not see the class
body, so they do not hide the builtin anywhere else.

## Why is this bad?

Inside that scope the builtin is no longer reachable: a later `type(x)`
or `system.tag.readBlocking(...)` calls the local value instead. The
failure appears far from the assignment.

## Example

```python
def onMessageReceived(self, payload):
    id = payload["id"]
    type = payload["type"]
    self.custom.label = "%s (%s)" % (id, type)
```

Use instead:

```python
def onMessageReceived(self, payload):
    itemId = payload["id"]
    itemType = payload["type"]
    self.custom.label = "%s (%s)" % (itemId, itemType)
```

## Options

* `allowed` (default `["__doc__", "__name__", "copyright", "credits",
  "exit", "help", "license", "quit"]`): names that may be rebound.

## pylint

Corresponds to `W0622 redefined-builtin`.
"#,
    options: &[OptionMeta {
        name: "allowed",
        ty: OptionType::StringList,
        default: r#"["__doc__", "__name__", "copyright", "credits", "exit", "help", "license", "quit"]"#,
        description: "Builtin names that may be rebound.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&RedefinedBuiltin)],
    options: Some(parse_options),
};

struct Options {
    allowed: Vec<String>,
}

fn parse_options(reader: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        allowed: reader.string_list("allowed")?,
    }))
}

struct RedefinedBuiltin;

impl ScriptRule for RedefinedBuiltin {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let allowed = &cx.options::<Options>().allowed;
        let mut seen = FxHashSet::default();
        let mut found: Vec<(TextRange, String)> = Vec::new();
        let mut bindings: Vec<_> = model.bindings().collect();
        bindings.sort_by_key(|(_, b)| b.range.start());
        for (_, b) in bindings {
            if b.is_implicit()
                || matches!(b.kind, BindingKind::Global | BindingKind::FutureImport)
                || model.scope(b.scope).kind == ScopeKind::Class
                || allowed.iter().any(|a| a == b.name)
            {
                continue;
            }
            let builtin = iglint_stubs::is_jython_builtin(b.name) || b.name == "system";
            if !builtin || !seen.insert((b.scope, b.name)) {
                continue;
            }
            let what = if b.name == "system" {
                "Ignition's scripting API"
            } else {
                "the builtin of the same name"
            };
            found.push((b.range, format!("`{}` shadows {what}", b.name)));
        }
        for (range, message) in found {
            report(cx, unit, range, message);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_shadowing() {
        let d = lint_module(
            "SCR117",
            "def f(id, type=None):\n    for input in []:\n        pass\n    id = 2\nsystem = None\n__doc__ = 'x'\nclass C:\n    def id(self): pass\n",
        );
        assert_eq!(flagged(&d), ["id", "type", "input", "system"]);
    }

    #[test]
    fn options() {
        let f = library("m", "def f(id):\n    pass\n");
        assert!(f.lint_with(&[("SCR117", "allowed = ['id']")]).is_empty());
    }
}
