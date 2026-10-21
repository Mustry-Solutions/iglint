//! BIND010 map-transform-duplicate-input.

use iglint_json::{Value, ValueKind};
use iglint_project::view::{Binding, TransformKind};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, Hook, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "BIND010",
    name: "map-transform-duplicate-input",
    category: Category::Bind,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Map transform maps the same input twice",
    explanation: r#"## What it does

Reports map transform rows whose input equals the input of an earlier row
of the same transform (same value and type: `1` and `"1"` are different,
`1` and `1.0` are the same; expression inputs are compared as text).

## Why is this bad?

The map transform uses the first matching row, so the later row is dead.
It is usually a copy-paste error where the input was meant to change.

## Example

```json
"mappings": [{"input": 1, "output": "Running"}, {"input": 1, "output": "Stopped"}]
```

Use instead:

```json
"mappings": [{"input": 1, "output": "Running"}, {"input": 2, "output": "Stopped"}]
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Binding(&MapTransformDuplicateInput)],
    options: None,
};

struct MapTransformDuplicateInput;

/// A comparable form of a literal input; `None` for objects and arrays,
/// which are not compared.
fn key(v: &Value<'_>) -> Option<String> {
    Some(match &v.kind {
        ValueKind::Null => "null".to_owned(),
        ValueKind::Bool(b) => format!("b:{b}"),
        ValueKind::Number(n) => format!("n:{}", n.as_f64()?),
        ValueKind::String(s) => format!("s:{}", s.value()),
        _ => return None,
    })
}

impl BindingRule for MapTransformDuplicateInput {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        for t in &binding.transforms {
            let TransformKind::Map(m) = &t.kind else {
                continue;
            };
            let mut seen: Vec<(String, usize)> = Vec::new();
            for (i, mapping) in m.mappings.iter().enumerate() {
                let Some(input) = mapping.input else {
                    continue;
                };
                let Some(k) = key(input) else {
                    continue;
                };
                if let Some((_, first)) = seen.iter().find(|(s, _)| *s == k) {
                    let file = cx.primary_file();
                    cx.report_in(
                        file,
                        input.range,
                        format!("This input is already mapped by row {} of the map transform; this row never applies", first + 1),
                    )
                    .node_path(format!("{}.mappings[{i}]", t.node_path));
                } else {
                    seen.push((k, i));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(mappings: &str) -> Fixture {
        Fixture::view(&format!(
            r#"{{"propConfig": {{"custom.x": {{"binding": {{"type": "property", "config": {{"path": "view.custom.y"}}, "transforms": [{{"type": "map", "inputType": "scalar", "outputType": "scalar", "mappings": [{mappings}], "fallback": null}}]}}}}}}}}"#
        ))
    }

    #[test]
    fn duplicates_by_value_and_type() {
        let d = view(r#"{"input": 1, "output": "a"}, {"input": "1", "output": "b"}, {"input": 1.0, "output": "c"}"#).lint(&["BIND010"]);
        assert_eq!(flagged(&d), vec!["1.0"]);
        assert!(d[0].message.contains("row 1"));
    }

    #[test]
    fn distinct_inputs() {
        assert!(view(r#"{"input": 1, "output": "a"}, {"input": 2, "output": "a"}, {"input": null, "output": "c"}"#).lint(&["BIND010"]).is_empty());
    }
}
