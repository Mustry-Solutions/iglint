//! SCR305 unresolved-component-reference.

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, RuleMeta};
use crate::registry::{OptionMeta, OptionType};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::report;
use crate::script_facts::access::unresolved_lookups;

pub static META: RuleMeta = RuleMeta {
    code: "SCR305",
    name: "unresolved-component-reference",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    summary: "`getSibling`/`getChild` with a literal name that matches no component",
    explanation: r#"## What it does

Follows component navigation in view scripts from `self` (the component,
or the view for view-level scripts) through `.parent`, `.getParent()`,
`.view`, `.rootContainer`, `getSibling('X')`, `getChild('A/B')` and local
aliases (`card = self.getSibling('Card')`), and reports a literal name
that no component of the view has at that place:

* `getSibling('X')`: the component's parent has no child `X`;
* `getChild('A/B')`: a path segment names no child of the container
  reached so far;
* `self.view.getChild('root/A')`: the path starts at the root container
  (paths that do not start with the root's name are not checked).

Navigation that cannot be followed (non-literal names, `children[i]`,
components reached through parameters) is not checked, and neither are
lookups below components that do not hold children in this view
(embedded views, and other components without a `children` array that
are not containers).

## Why is this bad?

The lookup returns `None` at run time and the next attribute access fails
with `AttributeError`. A wrong name is usually a component that was
renamed or moved.

## Example

```python
self.getSibling('TotalLabel').props.text = str(total)
```

Use instead (the sibling is called `Total`):

```python
self.getSibling('Total').props.text = str(total)
```

## Options

* `validate-scripts` (default `true`): when false the rule reports
  nothing (ign-lint's `validate_scripts`).

## ign-lint

Equivalent of the script part of `ComponentReferenceValidationRule`, with
its wording in `iglint ign-lint`. ign-lint reports a missing sibling twice
and also reports navigation that goes above the root container
(`self.parent.parent...` from a child of the root); iglint reports each
lookup once and does not guess what navigation above the root returns.
"#,
    options: &[OptionMeta {
        name: "validate-scripts",
        ty: OptionType::Bool,
        default: "true",
        description: "Check component lookups in scripts.",
    }],
    ign_lint_names: &["ComponentReferenceValidationRule"],
    ign_lint_option_map: &[("validate_scripts", "validate-scripts")],
    ..RuleMeta::DEFAULT
};

struct Options {
    validate: bool,
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        validate: o.bool("validate-scripts")?,
    }))
}

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&UnresolvedComponentReference)],
    options: Some(parse_options),
};

struct UnresolvedComponentReference;

impl ScriptRule for UnresolvedComponentReference {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !cx.options::<Options>().validate {
            return;
        }
        let Some(view) = cx.view() else {
            return;
        };
        for lookup in unresolved_lookups(unit, view) {
            let call = lookup.call.slice(&unit.script.source).to_owned();
            let relation = if lookup.method == "getSibling" {
                "sibling"
            } else {
                "child"
            };
            report(cx, unit, lookup.range, lookup.message).ign_lint_message(format!(
                "Component '{}' not found as {relation} in: {call}",
                lookup.name
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::{decoded_flagged, messages};
    use crate::rules::test_support::Fixture;

    fn view_with(script: &str) -> Fixture {
        let s = super::super::testing::json(script);
        Fixture::view(&format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
                {{"type": "ia.display.label", "meta": {{"name": "Total"}}}},
                {{"type": "ia.container.flex", "meta": {{"name": "Card"}}, "children": [
                    {{"type": "ia.display.label", "meta": {{"name": "Title"}}}}]}},
                {{"type": "ia.display.view", "meta": {{"name": "Embed"}}}},
                {{"type": "ia.input.button", "meta": {{"name": "B"}}, "events": {{"component": {{"onActionPerformed":
                    {{"type": "script", "scope": "G", "config": {{"script": {s}}}}}}}}}}}
            ]}}}}"#
        ))
    }

    #[test]
    fn missing_names() {
        let body = "\tself.getSibling('Total').props.text = 'a'\n\tself.getSibling('TotalLabel').props.text = 'b'\n\tself.getSibling('B')\n\tcard = self.getSibling('Card')\n\tcard.getChild('Title')\n\tcard.getChild('Subtitle')\n\tself.parent.getChild('Card/Nope')\n\tself.view.getChild('root/Card/Title')\n\tself.view.getChild('Root/Card')\n\tself.view.getChild('root/Gone')\n\tself.getSibling('card')\n\tself.getSibling('Embed').getChild('Inner')\n\tself.getSibling(name)\n\tself.parent.parent.getChild('X')";
        let d = view_with(body).lint(&["SCR305"]);
        assert_eq!(
            decoded_flagged(&d),
            vec![
                "'TotalLabel'",
                "'Subtitle'",
                "'Card/Nope'",
                "'root/Gone'",
                "'card'"
            ]
        );
        assert_eq!(
            messages(&d),
            vec![
                "No sibling component named `TotalLabel` (in `root`)",
                "No child component named `Subtitle` in `root/Card`",
                "No child component named `Nope` in `root/Card`",
                "No child component named `Gone` in `root`",
                "No sibling component named `card` (in `root`) (names are case-sensitive; did you mean `Card`?)",
            ]
        );
    }

    #[test]
    fn scripts_outside_views_are_not_checked() {
        let f =
            crate::rules::script::testing::library("m", "def f(self):\n    self.getSibling('X')\n");
        assert!(f.lint(&["SCR305"]).is_empty());
    }
}
