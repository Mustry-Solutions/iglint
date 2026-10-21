//! BIND007 placeholder-in-direct-tag-path.

use iglint_project::view::{Binding, BindingConfig, TagMode};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND007",
    name: "placeholder-in-direct-tag-path",
    category: Category::Bind,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Direct tag binding path contains `{...}`",
    explanation: r#"## What it does

Reports tag bindings in `direct` mode (the default) whose tag path contains
a `{name}` placeholder.

## Why is this bad?

Placeholders are only replaced in `indirect` mode. In `direct` mode the
braces are part of the tag path, so the binding subscribes to a tag that
does not exist. This usually happens when the mode was switched back to
direct, or the path was pasted from an indirect binding.

## Example

```json
{"type": "tag", "config": {"mode": "direct", "tagPath": "[default]Line{line}/Speed"}}
```

Use instead:

```json
{"type": "tag", "config": {"mode": "indirect", "tagPath": "[default]Line{line}/Speed",
  "references": {"line": "{view.params.line}"}}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Binding(&PlaceholderInDirectTagPath)],
    options: None,
};

struct PlaceholderInDirectTagPath;

impl BindingRule for PlaceholderInDirectTagPath {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Tag(t) = &binding.typed else {
            return;
        };
        if t.mode != TagMode::Direct {
            return;
        }
        let Some((name, range)) = t.placeholders.first() else {
            return;
        };
        let file = cx.primary_file();
        cx.report_in(
            file,
            *range,
            format!("`{{{name}}}` is not replaced in a direct tag binding; use indirect mode"),
        )
        .node_path(binding.node_path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(config: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"propConfig": {{"custom.x": {{"binding": {{"type": "tag", "config": {config}}}}}}}}}"#
        ))
    }

    #[test]
    fn placeholder_in_direct_and_default_mode() {
        assert_eq!(
            flagged(
                &view(r#"{"mode": "direct", "tagPath": "[default]L{line}/S"}"#).lint(&["BIND007"])
            ),
            vec!["line"]
        );
        assert_eq!(
            flagged(&view(r#"{"tagPath": "[default]L{a}/{b}"}"#).lint(&["BIND007"])),
            vec!["a"]
        );
    }

    #[test]
    fn plain_and_indirect_paths() {
        assert!(
            view(r#"{"tagPath": "[default]L1/S"}"#)
                .lint(&["BIND007"])
                .is_empty()
        );
        assert!(
            view(r#"{"mode": "indirect", "tagPath": "[default]L{a}", "references": {"a": "1"}}"#)
                .lint(&["BIND007"])
                .is_empty()
        );
    }
}
