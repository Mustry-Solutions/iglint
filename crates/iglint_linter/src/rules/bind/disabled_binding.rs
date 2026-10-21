//! BIND008 disabled-binding.

use iglint_project::view::Binding;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};
use crate::rules::helpers::key_range;

pub static META: RuleMeta = RuleMeta {
    code: "BIND008",
    name: "disabled-binding",
    category: Category::Bind,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "Binding disabled with `enabled: false`",
    explanation: r#"## What it does

Reports bindings that are switched off with `"enabled": false`.

## Why is this bad?

A disabled binding does nothing at run time, but the property still shows
the value the Designer cached when it last ran. Readers of the view assume
the value is live. Remove the binding, or enable it.

## Example

```json
{"binding": {"type": "expr", "enabled": false, "config": {"expression": "now(1000)"}}}
```

Use instead: delete the binding, or remove `"enabled": false`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Binding(&DisabledBinding)],
    options: None,
};

struct DisabledBinding;

impl BindingRule for DisabledBinding {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        if binding.is_enabled() {
            return;
        }
        let range = key_range(binding.value, "enabled").unwrap_or(binding.value.range);
        let file = cx.primary_file();
        cx.report_in(
            file,
            range,
            format!("The binding of `{}` is disabled", binding.property),
        )
        .node_path(binding.node_path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    #[test]
    fn disabled_and_enabled_bindings() {
        let f = Fixture::view(
            r#"{"propConfig": {
                "custom.a": {"binding": {"type": "expr", "enabled": false, "config": {"expression": "1"}}},
                "custom.b": {"binding": {"type": "expr", "enabled": true, "config": {"expression": "1"}}},
                "custom.c": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}"#,
        );
        let d = f.lint(&["BIND008"]);
        assert_eq!(flagged(&d), vec!["\"enabled\""]);
        assert_eq!(d[0].message, "The binding of `custom.a` is disabled");
    }
}
