//! SCR104 redefined-unused.

use iglint_python_semantic::{Binding, BindingKind, TryPart};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{file_line, report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "SCR104",
    name: "redefined-unused",
    category: Category::Script,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Function, class or import redefined before it is used",
    explanation: r#"## What it does

Reports a `def`, `class` or import that rebinds a name whose previous
`def`, `class` or import is still live there and is never read on any
path. The analysis follows control flow: definitions in different branches
of an `if` do not redefine each other, and a fallback import in an
`except ImportError:` handler does not redefine the import it replaces.

## Why is this bad?

The first definition is dead code. Usually it is a copy-paste mistake: two
functions with the same name in a library module, where callers only ever
get the second one.

## Example

```python
def formatValue(v):
    return "%.2f" % v

def formatValue(v, unit):
    return "%.2f %s" % (v, unit)
```

Use instead: give the functions different names, or delete the first one.

## pylint

Corresponds to `E0102 function-redefined`, `W0404 reimported` and pyflakes'
`redefinition of unused name`. pylint also reports redefinitions of names
that were used in between; iglint only reports definitions that are never
used.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&RedefinedUnused)],
    options: None,
};

struct RedefinedUnused;

fn is_definition(b: &Binding<'_>) -> bool {
    matches!(
        b.kind,
        BindingKind::FunctionDef
            | BindingKind::ClassDef
            | BindingKind::Import
            | BindingKind::FromImport
    )
}

/// `first` in the body of a `try` and `second` in one of its handlers.
fn different_forks(first: &Binding<'_>, second: &Binding<'_>) -> bool {
    first.try_parts.iter().any(|(range, part)| {
        *part == TryPart::Body
            && second
                .try_parts
                .iter()
                .any(|(r, p)| r == range && matches!(p, TryPart::Handler | TryPart::Else))
    })
}

impl ScriptRule for RedefinedUnused {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let mut found = Vec::new();
        for (_, second) in model.bindings() {
            if !is_definition(second) || second.name == "_" {
                continue;
            }
            let scope = model.scope(second.scope);
            if scope.reads_locals || scope.is_dynamic() {
                continue;
            }
            for id in &second.shadowed {
                let first = model.binding(*id);
                if !is_definition(first)
                    || first.is_used()
                    || first.import_guarded
                    || first.range.start() >= second.range.start()
                    || different_forks(first, second)
                {
                    continue;
                }
                found.push((first.range, second.range, second.name));
                break;
            }
        }
        for (first, second, name) in found {
            let line = file_line(cx, unit, first.start());
            let message = match line {
                Some(line) => format!("Redefinition of unused `{name}` from line {line}"),
                None => format!("Redefinition of unused `{name}`"),
            };
            let label_range = unit.script.file_range(first);
            let file = unit.file;
            let _ = report(cx, unit, second, message)
                .label(file, label_range, "first definition, never used")
                .fingerprint_key(format!("Redefinition of unused `{name}`"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_redefinitions() {
        let d = lint_module("SCR104", "def f():\n    pass\ndef f():\n    pass\nf()\n");
        assert_eq!(lines(&d), [(3, "f".to_owned())]);
        assert_eq!(messages(&d), ["Redefinition of unused `f` from line 1"]);
        let d = lint_module("SCR104", "import os\nimport os\nprint os\n");
        assert_eq!(lines(&d), [(2, "os".to_owned())]);
    }

    #[test]
    fn branches_and_fallbacks_are_not_redefinitions() {
        assert!(
            lint_module(
                "SCR104",
                "if c:\n    def f(): pass\nelse:\n    def f(): pass\nf()\n"
            )
            .is_empty()
        );
        assert!(lint_module("SCR104", "try:\n    import json\nexcept ImportError:\n    import simplejson as json\nprint json\n").is_empty());
        // Used before redefinition (on some path).
        assert!(lint_module("SCR104", "def f(): pass\nf()\ndef f(): pass\nf()\n").is_empty());
        assert!(lint_module("SCR104", "def f(): pass\nif c:\n    def f(): pass\nf()\n").is_empty());
        // Property setters read the first definition.
        assert!(lint_module("SCR104", "class C(object):\n    @property\n    def x(self): return 1\n    @x.setter\n    def x(self, v): pass\n").is_empty());
        // Assignments are not definitions.
        assert!(lint_module("SCR104", "import os\nos = None\n").is_empty());
    }

    #[test]
    fn view_scripts() {
        let d = view(ViewScript::Action, "\tdef helper():\n\t\treturn 1\n\tdef helper():\n\t\treturn 2\n\tself.x \u{3d} helper()").lint(&["SCR104"]);
        assert_eq!(decoded_flagged(&d), ["helper"]);
        assert_eq!(
            messages(&d),
            ["Redefinition of unused `helper` from line 1"]
        );
    }
}
