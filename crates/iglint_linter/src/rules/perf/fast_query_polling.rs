//! PERF002 fast-query-polling.

use std::any::Any;

use iglint_project::view::{Binding, BindingConfig, Polling};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{BindingRule, Hook, ParsedOptions, Phase, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "PERF002",
    name: "fast-query-polling",
    category: Category::Perf,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Query, tag history or HTTP binding polls faster than `minimum-interval`",
    explanation: r#"## What it does

Reports query, tag history and HTTP bindings with polling enabled whose
literal polling rate is shorter than `minimum-interval`. The binding's
`polling.rate` is in seconds; `minimum-interval` is in milliseconds, like
`BIND004`'s. A rate of `0` (no polling) and rates given as expressions are
not reported.

## Why is this bad?

Each poll runs the query (or history request, or HTTP call) again, for
every session that shows the view. A one-second poll on a view open on
twenty screens is twenty queries a second against the database, mostly
returning the same rows.

## Example

```json
"binding": {"type": "query", "config": {"queryPath": "Machines/Status",
  "polling": {"enabled": true, "rate": "1"}}}
```

Use instead:

```json
"binding": {"type": "query", "config": {"queryPath": "Machines/Status",
  "polling": {"enabled": true, "rate": "30"}}}
```

## Options

* `minimum-interval` (default `10000`): the shortest allowed polling
  interval in milliseconds.
"#,
    options: &[OptionMeta {
        name: "minimum-interval",
        ty: OptionType::Integer,
        default: "10000",
        description: "Shortest allowed polling interval in milliseconds.",
    }],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Binding(&FastQueryPolling)],
    options: Some(parse_options),
};

struct Options {
    minimum_ms: f64,
}

fn parse_options(r: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    Ok(Box::new(Options {
        minimum_ms: r.int("minimum-interval")? as f64,
    }) as Box<dyn Any + Send + Sync>)
}

struct FastQueryPolling;

impl BindingRule for FastQueryPolling {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        if !binding.is_enabled() {
            return;
        }
        let (what, polling): (&str, &Polling<'_>) = match &binding.typed {
            BindingConfig::Query(q) => ("Query binding", &q.polling),
            BindingConfig::TagHistory(h) => ("Tag history binding", &h.polling),
            BindingConfig::Http(h) => ("HTTP binding", &h.polling),
            _ => return,
        };
        if polling.enabled != Some(true) {
            return;
        }
        let (Some(seconds), Some(rate)) = (polling.literal_rate(), polling.rate) else {
            return;
        };
        let minimum = cx.options::<Options>().minimum_ms;
        let ms = seconds * 1000.0;
        if seconds <= 0.0 || ms >= minimum {
            return;
        }
        cx.report(
            rate.range,
            format!(
                "{what} polls every {} s (minimum {} s)",
                trim(seconds),
                trim(minimum / 1000.0)
            ),
        )
        .node_path(binding.node_path.clone());
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn view(binding: &str) -> String {
        format!(
            r#"{{"custom": {{"x": 1}}, "propConfig": {{"custom.x": {{"binding": {binding}}}}},
               "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}}}}}"#
        )
    }

    #[test]
    fn reports_fast_polling() {
        let f = Fixture::view(&view(
            r#"{"type": "query", "config": {"queryPath": "Q", "polling": {"enabled": true, "rate": "2"}}}"#,
        ));
        let d = f.lint(&["PERF002"]);
        assert_eq!(flagged(&d), vec!["\"2\""]);
        assert_eq!(d[0].message, "Query binding polls every 2 s (minimum 10 s)");
    }

    #[test]
    fn slow_disabled_zero_and_expression_rates() {
        for polling in [
            r#"{"enabled": true, "rate": "10"}"#,
            r#"{"enabled": false, "rate": "1"}"#,
            r#"{"enabled": true, "rate": 0}"#,
            r#"{"enabled": true, "rate": "{view.custom.rate}"}"#,
        ] {
            let f = Fixture::view(&view(&format!(
                r#"{{"type": "query", "config": {{"queryPath": "Q", "polling": {polling}}}}}"#
            )));
            assert!(f.lint(&["PERF002"]).is_empty(), "{polling}");
        }
    }

    #[test]
    fn option_and_tag_history() {
        let f = Fixture::view(&view(
            r#"{"type": "tag-history", "config": {"tags": [], "polling": {"enabled": true, "rate": 5}}}"#,
        ));
        assert_eq!(f.lint(&["PERF002"]).len(), 1);
        assert!(
            f.lint_with(&[("PERF002", "minimum-interval = 5000")])
                .is_empty()
        );
    }
}
