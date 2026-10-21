//! VIEW019 output-param-never-written.

use iglint_project::ModelData;
use iglint_project::view::{ParamDirection, PropCategory, View};

use super::props::def_range;
use super::unused_custom_property::usage;
use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW019",
    name: "output-param-never-written",
    category: Category::View,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "Output parameter is never written",
    explanation: r#"## What it does

Reports view parameters with `paramDirection: "output"` that nothing in the
view writes: no binding on the parameter, no bidirectional property binding
targeting it, and no script assignment (`self.view.params.result = ...`,
found on the parsed scripts; see `VIEW015` for how scripts are followed).
Views whose scripts use the parameters in a way that cannot be followed are
not reported.

## Why is this bad?

An output parameter exists so the embedding view can read a value back. If
the view never sets it, the caller always sees the default stored in
`view.json`, which is usually a mistake (a parameter that should be an
input, or a forgotten assignment).

## Example

```json
{"params": {"selected": null},
 "propConfig": {"params.selected": {"paramDirection": "output"}}}
```

Use instead: bind the parameter (`"binding": {"type": "property", ...}`) or
assign it in a script, or make it an input.

## ign-lint

No equivalent.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&OutputParamNeverWritten)],
    options: None,
};

struct OutputParamNeverWritten;

impl ViewRule for OutputParamNeverWritten {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let ModelData::View { file, .. } = cx.data() else {
            return;
        };
        let file = *file;
        if !view
            .params()
            .any(|p| p.direction == Some(ParamDirection::Output))
        {
            return;
        }
        let (accesses, complete) = cx.script_property_accesses();
        if !complete && !view.scripts.is_empty() {
            return;
        }
        let usage = usage(view, accesses);
        for (i, def) in view.properties.iter().enumerate() {
            if def.category != PropCategory::Params
                || def.direction != Some(ParamDirection::Output)
                || usage.is_written(i, def)
            {
                continue;
            }
            cx.report_in(
                file,
                def_range(view, def),
                format!("Output parameter `{}` is never written", def.name),
            )
            .node_path(def.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn found(params: &str, prop_config: &str, script: &str) -> Vec<String> {
        let view = format!(
            r#"{{"params": {params}, "propConfig": {prop_config},
  "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [
    {{"meta": {{"name": "B"}}, "type": "ia.input.button", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}}}]}}}}"#
        );
        Fixture::view(&view)
            .lint(&["VIEW019"])
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    const CONFIG: &str =
        r#"{"params.out": {"paramDirection": "output"}, "params.in": {"paramDirection": "input"}}"#;

    #[test]
    fn reports_unwritten_outputs_only() {
        assert_eq!(
            found(r#"{"out": 1, "in": 2}"#, CONFIG, "\\tpass"),
            vec!["Output parameter `out` is never written"]
        );
    }

    #[test]
    fn script_and_binding_writes() {
        assert!(found(r#"{"out": 1}"#, CONFIG, "\\tself.view.params.out = 2").is_empty());
        let bound = r#"{"params.out": {"paramDirection": "output", "binding": {"type": "expr", "config": {"expression": "1"}}}}"#;
        assert!(found(r#"{"out": 1}"#, bound, "\\tpass").is_empty());
        // Reading is not writing.
        assert_eq!(
            found(r#"{"out": 1}"#, CONFIG, "\\tprint self.view.params.out").len(),
            1
        );
        // Dynamic access: silent.
        assert!(
            found(
                r#"{"out": 1}"#,
                CONFIG,
                "\\tsetattr(self.view.params, k, 1)"
            )
            .is_empty()
        );
    }
}
