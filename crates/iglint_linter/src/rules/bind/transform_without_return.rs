//! BIND009 transform-without-return.

use iglint_project::ScriptKind;
use iglint_python::ast::Stmt;
use iglint_text::{Ranged, TextRange};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{report, semantic};

pub static META: RuleMeta = RuleMeta {
    code: "BIND009",
    name: "transform-without-return",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Script transform can finish without `return`",
    explanation: r#"## What it does

Reports script transforms whose body can reach its end without a `return`
statement. The analysis follows the control flow of the body: `if`/`elif`
without a final `else`, loops that may run zero times, `except` handlers
that do not return, `try`/`finally`, `with`. Paths that end in `raise` or
in a `while True:` loop without `break` do not count. Calls that never
return (`sys.exit()`) are not recognised, and conditions are not
evaluated: `if x: return 1` followed by `elif not x: return 0` is reported
because the code does not show that one branch always runs.

Transforms that do not parse are left to `SCR001`.

## Why is this bad?

Ignition calls the transform as a function: falling off the end returns
`None`, so the bound property becomes null. The binding shows no error and
no quality problem; the component just loses its value on some inputs.

## Example

```python
def transform(self, value, quality, timestamp):
    if value > 100:
        return "High"
    elif value > 50:
        return "Medium"
```

Use instead:

```python
def transform(self, value, quality, timestamp):
    if value > 100:
        return "High"
    elif value > 50:
        return "Medium"
    return "Low"
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&TransformWithoutReturn)],
    options: None,
};

struct TransformWithoutReturn;

impl ScriptRule for TransformWithoutReturn {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if unit.script.kind != ScriptKind::Transform || unit.script.is_blank() {
            return;
        }
        let Some(model) = semantic(cx, unit) else {
            return;
        };
        let Some(Stmt::FunctionDef(f)) = unit.parsed.module.body.first() else {
            return;
        };
        if model
            .function_scope(f)
            .is_some_and(|s| model.scope(s).is_generator)
            || !model.function_can_fall_through(f)
        {
            return;
        }
        // Point at the last statement of the body (where execution ends).
        let range: TextRange = f.body.last().map_or(f.range, |s| s.range());
        let _ = report(
            cx,
            unit,
            range,
            "Script transform can finish without `return`; the property then becomes null",
        )
        .fingerprint_key("transform-without-return");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    fn lint(code: &str) -> Vec<String> {
        decoded_flagged(&view(ViewScript::Transform, code).lint(&["BIND009"]))
    }

    #[test]
    fn reports_paths_without_return() {
        assert_eq!(
            lint("\tif value \u{3e} 1:\n\t\treturn 'a'\n\telif value \u{3e} 0:\n\t\treturn 'b'"),
            ["if value > 1:\n\t\treturn 'a'\n\telif value > 0:\n\t\treturn 'b'"]
        );
        assert_eq!(
            lint("\tself.custom.x \u{3d} value"),
            ["self.custom.x = value"]
        );
        assert_eq!(
            lint("\ttry:\n\t\treturn int(value)\n\texcept:\n\t\tpass").len(),
            1
        );
        assert_eq!(lint("\tfor v in value:\n\t\treturn v").len(), 1);
    }

    #[test]
    fn complete_transforms_are_fine() {
        let cases = [
            "\treturn value",
            "\tif value:\n\t\treturn 1\n\telse:\n\t\treturn 0",
            "\tif value:\n\t\treturn 1\n\traise ValueError(value)",
            "\ttry:\n\t\treturn int(value)\n\texcept ValueError:\n\t\treturn 0",
            "\twhile True:\n\t\treturn value",
            "\tfor v in value:\n\t\treturn v\n\telse:\n\t\treturn None",
            "\twith lock:\n\t\treturn value",
            "\ttry:\n\t\tx \u{3d} 1\n\tfinally:\n\t\treturn value",
        ];
        for code in cases {
            assert!(lint(code).is_empty(), "{code}");
        }
    }

    #[test]
    fn only_transforms_and_parsable_scripts() {
        assert!(
            view(ViewScript::Action, "\tself.x \u{3d} 1")
                .lint(&["BIND009"])
                .is_empty()
        );
        assert!(lint("\tif value\n\t\treturn 1").is_empty());
        assert!(lint("  ").is_empty());
    }
}
