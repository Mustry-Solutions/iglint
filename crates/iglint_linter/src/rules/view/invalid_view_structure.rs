//! VIEW021 invalid-view-structure.

use iglint_project::model::ModelData;
use rustc_hash::FxHashSet;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW021",
    name: "invalid-view-structure",
    category: Category::View,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "A part of a view or of the session properties has the wrong JSON type",
    explanation: r#"## What it does

Reports the parts of a `view.json` (and of the session properties'
`props.json`) that iglint reads and that have the wrong JSON type: a
`root` or child component that is not an object, `children` that is not an
array, `events`, `scripts` or `propConfig` that is not an object, an event
action, binding, change script, message handler, custom method or
extension function that is not an object, a script or a name that is not a
string, a component without `meta.name`. Each problem is reported once, at
the offending value.

## Why is this bad?

Perspective cannot load such a part as written: the component, event or
script is dropped or the view fails to open, usually without a message in
the Designer. iglint skips the broken part too, so no other rule checks
what is inside it. When the broken part may hold scripts (it is a
component, an event, a script place or a script and contains text), the
code there is unknown to iglint: safe fixes that rely on knowing every
script of the workspace (`SCR102`, `VIEW015`, `NAME001`) become unsafe and
name it.

## Example

```json
{
  "root": {
    "type": "ia.container.flex",
    "meta": {"name": "root"},
    "children": "Label",
    "scripts": {"customMethods": [{"name": 5, "script": "\treturn 1"}]}
  }
}
```

Use instead:

```json
{
  "root": {
    "type": "ia.container.flex",
    "meta": {"name": "root"},
    "children": [{"type": "ia.display.label", "meta": {"name": "Label"}}],
    "scripts": {"customMethods": [{"name": "answer", "params": [], "script": "\treturn 1"}]}
  }
}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&InvalidViewStructure)],
    options: None,
};

struct InvalidViewStructure;

impl ResourceRule for InvalidViewStructure {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let (file, problems) = match data {
            ModelData::View { file, view } => (*file, &view.problems),
            ModelData::SessionProps { file, props } => (*file, &props.problems),
            _ => return,
        };
        let mut seen = FxHashSet::default();
        for p in problems {
            if !seen.insert((p.range, p.message.as_str())) {
                continue;
            }
            let message = format!("{}: Perspective cannot load this part", p.message);
            cx.report_in(file, p.range, message)
                .node_path(p.node_path.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Diagnostic;
    use crate::rules::test_support::*;

    fn messages(d: &[Diagnostic]) -> Vec<String> {
        d.iter()
            .map(|d| d.message.split(':').next().unwrap_or("").to_owned())
            .collect()
    }

    /// Reviewer round 9 (Opus) finding 4: each malformed part is reported
    /// once, at the offending value.
    #[test]
    fn reports_wrong_types_at_the_offending_node() {
        for (view, expected, at) in [
            (r#"{"root": []}"#, "`root` is not a component object", "[]"),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": "x"}}"#,
                "`children` is not an array",
                "\"x\"",
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "events": {"component": {"onActionPerformed": {"type": "script", "config": {"script": 7}}}}}}"#,
                "`script` is not a string",
                "7",
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "scripts": {"customMethods": [{"name": 5, "script": "\treturn 1"}]}}}"#,
                "custom method `name` is not a string",
                "{\"name\": 5",
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "scripts": {"customMethods": {"a": 1}}}}"#,
                "`customMethods` is not an array",
                "{\"a\": 1}",
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "scripts": {"customMethods": [{"name": "m", "script": ["\treturn 1"]}]}}}"#,
                "custom method `script` is not a string",
                "[\"",
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "events": {"component": {"onClick": {"type": "script", "config": "x"}}}}}"#,
                "action `config` is not an object",
                "\"x\"",
            ),
        ] {
            let d = Fixture::view(view).lint(&["VIEW021"]);
            assert_eq!(messages(&d), [expected], "{view}");
            let text = d[0].file.text();
            let found = text.get(d[0].range.start().to_usize()..).unwrap();
            assert!(found.starts_with(at), "{view}: {found:?}");
        }
        // Well-formed views: nothing.
        let ok = r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [], "scripts": {"customMethods": [{"name": "m", "params": [], "script": "\treturn 1"}]}}}"#;
        assert!(Fixture::view(ok).lint(&["VIEW021"]).is_empty());
    }

    /// A malformed part that may hold scripts hides code iglint does not
    /// read: the closed world has unknown code, and a VIEW015 fix in
    /// another view is unsafe and names the site. A malformed part without
    /// text (`"script": 7`) hides nothing.
    #[test]
    fn hidden_scripts_are_unknown_code() {
        let other = r#"{"custom": {"unused": 1}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#;
        for (broken, hides) in [
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "scripts": {"customMethods": [{"name": 5, "script": "\tprint 1"}]}}}"#,
                true,
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [["\tprint 1"]]}}"#,
                true,
            ),
            (
                r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "events": {"component": {"onClick": {"type": "script", "config": {"script": 7}}}}}}"#,
                false,
            ),
        ] {
            let f = Fixture::new().with_view("A", other).with_view("B", broken);
            let d = f.lint(&["VIEW015", "VIEW021"]);
            assert!(d.iter().any(|d| d.rule.code() == "VIEW021"), "{d:?}");
            let fix = d
                .iter()
                .find(|d| d.rule.code() == "VIEW015")
                .and_then(|d| d.fix.as_ref())
                .unwrap_or_else(|| panic!("{d:?}"));
            assert_eq!(
                fix.applicability == crate::Applicability::Unsafe,
                hides,
                "{broken}: {fix:?}"
            );
        }
    }
}
