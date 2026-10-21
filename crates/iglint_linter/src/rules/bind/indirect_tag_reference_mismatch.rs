//! BIND006 indirect-tag-reference-mismatch.

use iglint_project::view::{Binding, BindingConfig, TagMode};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND006",
    name: "indirect-tag-reference-mismatch",
    category: Category::Bind,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "Indirect tag binding placeholders and references differ",
    explanation: r#"## What it does

In a tag binding in `indirect` mode, compares the `{name}` placeholders of
the tag path with the keys of `references`. Reports placeholders without a
reference and references that no placeholder uses.

## Why is this bad?

A placeholder without a reference is never replaced, so the binding
subscribes to a tag path that literally contains `{name}` and never gets a
good value. A reference without a placeholder is dead configuration,
usually left behind after the path was edited.

## Example

```json
{"type": "tag", "config": {"mode": "indirect",
  "tagPath": "[default]Line{line}/Motor{motor}/Speed",
  "references": {"line": "{view.params.line}"}}}
```

Use instead:

```json
{"type": "tag", "config": {"mode": "indirect",
  "tagPath": "[default]Line{line}/Motor{motor}/Speed",
  "references": {"line": "{view.params.line}", "motor": "{view.params.motor}"}}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Binding(&IndirectTagReferenceMismatch)],
    options: None,
};

struct IndirectTagReferenceMismatch;

impl BindingRule for IndirectTagReferenceMismatch {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Tag(t) = &binding.typed else {
            return;
        };
        if t.mode != TagMode::Indirect || t.tag_path.is_none() {
            return;
        }
        let file = cx.primary_file();
        for (name, range) in &t.placeholders {
            if !t.references.iter().any(|r| r.key.value() == name) {
                cx.report_in(
                    file,
                    *range,
                    format!("Placeholder `{{{name}}}` has no entry in `references`; it is never replaced"),
                )
                .node_path(binding.node_path.clone());
            }
        }
        for r in &t.references {
            if !t.placeholders.iter().any(|(n, _)| n == r.key.value()) {
                cx.report_in(
                    file,
                    r.key.range(),
                    format!(
                        "Reference `{}` is not used by any `{{{}}}` placeholder of the tag path",
                        r.key.value(),
                        r.key.value()
                    ),
                )
                .node_path(binding.node_path.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(path: &str, refs: &str, mode: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"propConfig": {{"custom.x": {{"binding": {{"type": "tag", "config": {{"mode": "{mode}", "tagPath": "{path}", "references": {{{refs}}}}}}}}}}}}}"#
        ))
    }

    #[test]
    fn missing_and_unused_references() {
        let f = view(
            "[default]L{line}/M{motor}",
            r#""line": "{view.params.l}", "extra": "1""#,
            "indirect",
        );
        let d = f.lint(&["BIND006"]);
        assert_eq!(flagged(&d), vec!["motor", "\"extra\""]);
    }

    #[test]
    fn matching_and_numeric_placeholders() {
        assert!(
            view(
                "[default]L{0}/M{motor}",
                r#""0": "a", "motor": "b""#,
                "indirect"
            )
            .lint(&["BIND006"])
            .is_empty()
        );
    }

    #[test]
    fn other_modes_are_ignored() {
        assert!(
            view("[default]L{line}", "", "direct")
                .lint(&["BIND006"])
                .is_empty()
        );
    }
}
