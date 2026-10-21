//! TAG003 unknown-tag-path.

use iglint_expression::{ExprKind, Reference, parse_tag_path};
use iglint_project::event_resource::EventSettings;
use iglint_project::model::ModelData;
use iglint_project::script::Owner;
use iglint_project::view::{Binding, BindingConfig, TagMode};
use iglint_project::{EmbeddedExpression, Existence, FileId};
use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::Severity;
use crate::registry::{Category, RuleMeta};
use crate::rule::{BindingRule, ExpressionRule, Hook, Phase, ResourceRule, RuleImpl};

pub static META: RuleMeta = RuleMeta {
    code: "TAG003",
    name: "unknown-tag-path",
    category: Category::Tag,
    default_severity: Severity::Error,
    default_enabled: false,
    summary: "Tag path to a tag that does not exist",
    explanation: r#"## What it does

Checks static, absolute tag paths (`[provider]path`) against the tags of
the workspace's gateway configuration: direct tag bindings, `{[provider]...}`
references and literal `tag("...")` calls in expressions (bindings and
tag expressions), and the paths of tag change scripts. UDT instance
members are resolved through their definitions, tag properties
(`.Quality`) are ignored, and paths are compared case-insensitively.

Only providers whose tags are in the workspace are checked: paths to
other providers, remote and managed providers (`MQTT Engine`, remote tag
providers), relative paths (`[.]`, `[~]`) and paths with placeholders stay
silent.

## Why is this bad?

A binding or script to a missing tag gets bad quality (`Bad_NotFound`):
the component shows nothing, the expression evaluates to null, the tag
change script never fires.

## Why is this rule not in the default set?

Tags are not only configured in the repository. Scripts create tags at
run time (`system.tag.configure`), and a workspace with several gateways
can have tag providers of the same name whose configurations differ; the
workspace does not say which gateway a project runs on. A missing tag is
therefore only certain when the repository holds the complete
configuration of the gateway the project runs on. Enable it for such
repositories.

## Example

```json
{"type": "tag", "config": {"mode": "direct", "tagPath": "[default]Line1/Sped"}}
```

Use instead:

```json
{"type": "tag", "config": {"mode": "direct", "tagPath": "[default]Line1/Speed"}}
```
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[
        Hook::Binding(&UnknownTagPath),
        Hook::Expression(&UnknownTagPath),
        Hook::Resource(&UnknownTagPath),
    ],
    options: None,
};

struct UnknownTagPath;

fn check(cx: &mut Checker<'_, '_>, file: FileId, path: &str, range: TextRange, node: &str) {
    let Some(index) = cx.index() else {
        return;
    };
    if path.contains('{') {
        return;
    }
    if index.tag(path) == Existence::Missing {
        cx.report_in(file, range, format!("Tag `{path}` does not exist"))
            .node_path(node);
    }
}

impl BindingRule for UnknownTagPath {
    fn check_binding(&self, cx: &mut Checker<'_, '_>, binding: &Binding<'_>) {
        let BindingConfig::Tag(t) = &binding.typed else {
            return;
        };
        if t.mode != TagMode::Direct {
            return;
        }
        if let Some(path) = t.tag_path {
            let file = cx.primary_file();
            check(cx, file, path.value(), path.range(), &binding.node_path);
        }
    }
}

impl ExpressionRule for UnknownTagPath {
    fn check_expression(&self, cx: &mut Checker<'_, '_>, e: &EmbeddedExpression<'_>) {
        let file = match (cx.data(), e.owner) {
            (ModelData::Tags(t), Owner::Tag(id)) => t.tag(id).file,
            _ => cx.primary_file(),
        };
        for (bound, reference) in e.references() {
            if let Reference::Tag(t) = reference
                && t.is_valid()
                && !t.is_relative()
            {
                let range = e.file_range(bound.path_range);
                check(cx, file, &bound.path, range, &e.node_path);
            }
        }
        for call in e.parsed.expr.calls() {
            if !call.is("tag") {
                continue;
            }
            let Some(arg) = call.args.first().map(|a| a.unparenthesized()) else {
                continue;
            };
            let ExprKind::String(s) = &arg.kind else {
                continue;
            };
            let parsed = parse_tag_path(s, TextSize::ZERO);
            if parsed.is_valid() && !parsed.is_relative() {
                let range = e.file_range(arg.range);
                check(cx, file, s, range, &e.node_path);
            }
        }
    }
}

impl ResourceRule for UnknownTagPath {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        let ModelData::EventScript(e) = data else {
            return;
        };
        let EventSettings::TagChange { paths, .. } = &e.settings else {
            return;
        };
        let Some(resource) = cx.resource() else {
            return;
        };
        let (manifest, node) = (resource.manifest.file, resource.path.to_string());
        for (s, parsed) in paths {
            if parsed.is_valid() && !parsed.is_relative() {
                check(cx, manifest, s.value(), s.range(), &node);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::tag::fixtures::*;
    use crate::rules::test_support::*;

    fn view() -> String {
        r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [
            {"type": "ia.display.label", "meta": {"name": "L"}, "props": {"text": ""},
             "propConfig": {"props.text": {"binding": {"type": "tag", "config": {"mode": "direct", "tagPath": "[default]Line/Sped"}}}}},
            {"type": "ia.display.label", "meta": {"name": "M"}, "props": {"text": ""},
             "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression":
                "{[default]Line/Speed} + {[default]Line/Gone.Quality} + tag('[default]M1/Speed') + tag('[default]M1/Nope') + {[Other]X} + {[.]Y}"}}}}}
        ]}}"#
        .to_owned()
    }

    fn workspace() -> Fixture {
        let f = udts(
            gateway(),
            r#"[{"name": "Motor", "tagType": "UdtType", "tags": [{"name": "Speed", "tagType": "AtomicTag"}]}]"#,
        );
        let f = tags(
            f,
            r#"[{"name": "Line", "tagType": "Folder", "tags": [{"name": "Speed", "tagType": "AtomicTag"}]}]"#,
            r#"[{"name": "M1", "tagType": "UdtInstance", "typeId": "Motor"}]"#,
        );
        f.with_view("Main", &view())
    }

    #[test]
    fn missing_tags_in_bindings_and_expressions() {
        let d = workspace().lint(&["TAG003"]);
        assert_eq!(
            flagged(&d),
            vec![
                "\"[default]Line/Sped\"",
                "[default]Line/Gone.Quality",
                "'[default]M1/Nope'"
            ]
        );
    }

    #[test]
    fn providers_outside_the_workspace_are_unknown() {
        let f = Fixture::view(&view());
        assert!(f.lint(&["TAG003"]).is_empty());
    }
}
