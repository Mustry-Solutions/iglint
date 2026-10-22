//! REF008 unknown-page-url.

use iglint_project::Lookup;
use iglint_project::view::{ActionKind, NavTarget, View};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit, ViewRule};
use crate::script_facts::api::ApiTarget;
use crate::script_facts::calls::string_values;

pub static META: RuleMeta = RuleMeta {
    code: "REF008",
    name: "unknown-page-url",
    category: Category::Ref,
    default_severity: Severity::Warning,
    default_enabled: true,
    summary: "Navigation to a page URL that the page configuration does not define",
    explanation: r#"## What it does

Checks the `page` of every navigate action against the page
configuration of the project (with inheritance), matching `:param`
segments the way Perspective does (`/motors/:line` matches
`/motors/L1`). Literal page URLs passed to `system.perspective.navigate`
in scripts (`navigate('/motors/L1')`, `navigate(page=...)`) are checked
too; a query string or fragment is ignored.

## Why is this bad?

Navigating to a URL no page serves shows Perspective's "page not found"
screen.

External URLs (`url` navigation), URLs with `{...}` placeholders and
projects that inherit from a project outside the workspace are not
reported.

## Example

```json
{"type": "nav", "config": {"page": "/motor/L1"}}
```

Use instead (the page is configured as `/motors/:line`):

```json
{"type": "nav", "config": {"page": "/motors/L1"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::View(&UnknownPage), Hook::Script(&UnknownPage)],
    options: None,
};

struct UnknownPage;

impl ViewRule for UnknownPage {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        for e in &view.events {
            for a in &e.actions {
                let ActionKind::Navigate(NavTarget::Page(url)) = &a.kind else {
                    continue;
                };
                let value = url.value();
                if value.is_empty() || value.contains('{') || value.contains("://") {
                    continue;
                }
                if matches!(index.page_for_url(project, value), Lookup::Missing) {
                    let file = cx.primary_file();
                    cx.report_in(
                        file,
                        url.range(),
                        format!("No page in the page configuration serves `{value}`"),
                    )
                    .node_path(&a.node_path);
                }
            }
        }
    }
}

impl ScriptRule for UnknownPage {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        let Some((model, targets)) = super::script_calls::api_targets(cx, unit) else {
            return;
        };
        let Some((index, project)) = cx.project_index() else {
            return;
        };
        for (_, target) in targets {
            let ApiTarget::Page { url } = target else {
                continue;
            };
            for literal in string_values(model, url).unwrap_or_default() {
                let value = literal.value.split(['?', '#']).next().unwrap_or("");
                if value.is_empty() || value.contains('{') || literal.value.contains("://") {
                    continue;
                }
                if matches!(index.page_for_url(project, value), Lookup::Missing) {
                    let message = format!("No page in the page configuration serves `{value}`");
                    crate::rules::script::report(cx, unit, literal.range, message);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn nav(page: &str) -> String {
        format!(
            r#"{{"root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
                {{"type": "ia.input.button", "meta": {{"name": "B"}}, "events": {{"component": {{"onActionPerformed":
                    {{"type": "nav", "scope": "C", "config": {{"page": "{page}"}}}}}}}}}}
            ]}}}}"#
        )
    }

    fn with_pages(f: Fixture) -> Fixture {
        f.resource(
            "projects/p/com.inductiveautomation.perspective/page-config",
            &[(
                "config.json",
                r#"{"pages": {"/": {"viewPath": "Main"}, "/motors/:line": {"viewPath": "Main"}}}"#,
            )],
        )
    }

    #[test]
    fn unknown_and_known_urls() {
        let d = with_pages(Fixture::view(&nav("/motor/L1"))).lint(&["REF008"]);
        assert_eq!(flagged(&d), vec!["\"/motor/L1\""]);
        assert!(
            with_pages(Fixture::view(&nav("/motors/L1")))
                .lint(&["REF008"])
                .is_empty()
        );
        assert!(
            with_pages(Fixture::view(&nav("/")))
                .lint(&["REF008"])
                .is_empty()
        );
        assert!(
            with_pages(Fixture::view(&nav("https://example.com/x")))
                .lint(&["REF008"])
                .is_empty()
        );
    }

    #[test]
    fn script_call_sites() {
        use crate::rules::script::testing::{ViewScript, decoded_flagged, view};
        let body = "\tsystem.perspective.navigate('/motor/L1')\n\tsystem.perspective.navigate(page='/motors/L1?x=1')\n\tsystem.perspective.navigate(url='http://x/y')\n\tsystem.perspective.navigate('/')";
        let d = with_pages(view(ViewScript::Action, body)).lint(&["REF008"]);
        assert_eq!(decoded_flagged(&d), vec!["'/motor/L1'"]);
    }
}
