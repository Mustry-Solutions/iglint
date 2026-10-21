//! PERF001 deep-component-nesting.

use std::any::Any;

use iglint_project::view::{Component, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{ComponentRule, Hook, ParsedOptions, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "PERF001",
    name: "deep-component-nesting",
    category: Category::Perf,
    default_severity: Severity::Info,
    default_enabled: true,
    summary: "Components nested deeper than `max-depth`",
    explanation: r#"## What it does

Reports components nested more than `max-depth` containers below the
view's root container. Only the first component beyond the limit on each
branch is reported.

## Why is this bad?

Every container level adds DOM elements, layout work in the browser and
property tree nodes on the gateway. Deep trees are slow to render and hard
to maintain; an embedded view or a flatter layout usually does the same.

## Example

A label inside fourteen nested flex containers.

Use instead: move the inner part into its own view and embed it, or
remove containers that only wrap a single child.

## Options

* `max-depth` (default `12`): the deepest allowed level; the root
  container is level 0.
"#,
    options: &[OptionMeta {
        name: "max-depth",
        ty: OptionType::Integer,
        default: "12",
        description: "Deepest allowed nesting level (root container is 0).",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Component(&DeepNesting)],
    options: Some(parse_options),
};

struct Options {
    max_depth: u32,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let max = r.int("max-depth")?;
    if max < 1 {
        return Err("option `max-depth` of PERF001 must be at least 1".to_owned());
    }
    Ok(Box::new(Options {
        max_depth: u32::try_from(max).unwrap_or(u32::MAX),
    }) as Box<dyn Any + Send + Sync>)
}

struct DeepNesting;

impl ComponentRule for DeepNesting {
    fn check_component(
        &self,
        cx: &mut Checker<'_, '_>,
        _view: &View<'_>,
        component: &Component<'_>,
    ) {
        let max = cx.options::<Options>().max_depth;
        if component.depth != max + 1 {
            return;
        }
        cx.report(
            component.name_range(),
            format!(
                "Component `{}` is nested {} levels deep (maximum {max})",
                component.name, component.depth
            ),
        )
        .node_path(component.path.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn nested(levels: usize) -> String {
        let mut inner = r#"{"type": "ia.display.label", "meta": {"name": "Leaf"}}"#.to_owned();
        for i in (1..levels).rev() {
            inner = format!(
                r#"{{"type": "ia.container.flex", "meta": {{"name": "C{i}"}}, "children": [{inner}]}}"#
            );
        }
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [{inner}]}}}}"#
        )
    }

    #[test]
    fn default_limit() {
        assert!(Fixture::view(&nested(12)).lint(&["PERF001"]).is_empty());
        let d = Fixture::view(&nested(14)).lint(&["PERF001"]);
        assert_eq!(flagged(&d), vec!["\"C13\""]);
        assert_eq!(
            d[0].message,
            "Component `C13` is nested 13 levels deep (maximum 12)"
        );
    }

    #[test]
    fn option() {
        let d = Fixture::view(&nested(3)).lint_with(&[("PERF001", "max-depth = 2")]);
        assert_eq!(flagged(&d), vec!["\"Leaf\""]);
    }
}
