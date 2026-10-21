//! PERF004 large-view.

use std::any::Any;

use iglint_project::view::View;

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "PERF004",
    name: "large-view",
    category: Category::Perf,
    default_severity: Severity::Info,
    default_enabled: false,
    summary: "View has more than `max-components` components",
    explanation: r#"## What it does

Reports a view with more than `max-components` components (the root
container included).

## Why is this bad?

Every component is a property tree on the gateway and a set of DOM
elements in each session that shows the view. Very large views load
slowly, use memory per session and are hard to change. Repeated parts are
better as an embedded view or a flex repeater.

## Example

A view with 600 labels and LEDs laid out by hand.

Use instead: one small view per repeated element, shown by a flex repeater
over a list of parameters.

## Options

* `max-components` (default `300`): the largest allowed number of
  components.
"#,
    options: &[OptionMeta {
        name: "max-components",
        ty: OptionType::Integer,
        default: "300",
        description: "Largest allowed number of components in a view.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&LargeView)],
    options: Some(parse_options),
};

struct Options {
    max: usize,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let max = r.int("max-components")?;
    if max < 1 {
        return Err("option `max-components` of PERF004 must be at least 1".to_owned());
    }
    Ok(Box::new(Options {
        max: usize::try_from(max).unwrap_or(usize::MAX),
    }) as Box<dyn Any + Send + Sync>)
}

struct LargeView;

impl ViewRule for LargeView {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let max = cx.options::<Options>().max;
        let count = view.components.len();
        if count <= max {
            return;
        }
        let Some(root) = view.root_component() else {
            return;
        };
        cx.report(
            root.name_range(),
            format!("View has {count} components (maximum {max})"),
        )
        .node_path(root.path.clone())
        .fingerprint_key("large-view");
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(n: usize) -> String {
        let children: Vec<String> = (0..n)
            .map(|i| format!(r#"{{"type": "ia.display.label", "meta": {{"name": "L{i}"}}}}"#))
            .collect();
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [{}]}}}}"#,
            children.join(",")
        )
    }

    #[test]
    fn counts_components() {
        assert!(
            Fixture::view(&view(4))
                .lint_with(&[("PERF004", "max-components = 5")])
                .is_empty()
        );
        let d = Fixture::view(&view(5)).lint_with(&[("PERF004", "max-components = 5")]);
        assert_eq!(d[0].message, "View has 6 components (maximum 5)");
        assert!(Fixture::view(&view(5)).lint(&["PERF004"]).is_empty());
    }
}
