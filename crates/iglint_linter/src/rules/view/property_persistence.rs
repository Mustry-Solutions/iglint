//! VIEW013 property-persistence.

use iglint_json::InsertPosition;
use iglint_project::view::{BindingConfig, BindingKind, PropConfig, TagMode, View};
use iglint_project::{FileId, ModelData};
use iglint_text::TextEdit;

use super::props::{ign_full_path, lookup, owner_object};
use crate::checker::Checker;
use crate::diagnostic::{Fix, Severity};
use crate::options::OptionReader;
use crate::registry::{Category, FixAvailability, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW013",
    name: "property-persistence",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: false,
    fix: FixAvailability::Sometimes,
    summary: "Bound custom property persistence differs from the project convention",
    explanation: r#"## What it does

Checks `persistent` on the `propConfig` entries of bound custom properties
(`custom.*` entries with a `binding`, view level and components) against
`expected-persistent`. A missing `persistent` means persistent. Nothing is
checked until `expected-persistent` is set.

* `expected-persistent = false`: reports bound properties that are
  persistent. The Designer then writes every binding result into
  `view.json`, which bloats the file and produces noisy diffs.
* `expected-persistent = true`: reports bound properties that are not
  persistent, and persistent ones without a stored default value.

## Why is this bad?

A bound, persistent property stores the Designer's last binding result in
the resource. Query results and tag values end up in version control and
change on every save.

## Example

With `expected-persistent = false`:

```json
{"custom": {"rows": [1, 2, 3]},
 "propConfig": {"custom.rows": {"binding": {"type": "query", "config": {"queryPath": "Rows"}}}}}
```

Use instead:

```json
{"custom": {},
 "propConfig": {"custom.rows": {"binding": {"type": "query", "config": {"queryPath": "Rows"}}, "persistent": false}}}
```

## Options

* `expected-persistent`: `true` or `false`; unset disables the rule.
* `exempt-binding-types`: binding types that are not checked: `expr`,
  `expr-struct`, `property`, `query`, `tag` (every tag mode), `tag.direct`,
  `tag.indirect`, `tag.expression`, and the other registered types
  (`tag-history`, `http`). Default `["tag"]`; `[]` checks everything.

## Fix

Safe, for `expected-persistent = false`: sets `persistent: false` and
removes the stored value (when its path has no array index).

## ign-lint

Equivalent of `PropertyPersistenceRule` (`expected_persistent`,
`exempt_binding_types`), with the same messages in `iglint ign-lint`.
"#,
    options: &[
        OptionMeta {
            name: "expected-persistent",
            ty: OptionType::Bool,
            default: "none",
            description: "Whether bound custom properties must be persistent.",
        },
        OptionMeta {
            name: "exempt-binding-types",
            ty: OptionType::StringList,
            default: "[\"tag\"]",
            description: "Binding types that are not checked.",
        },
    ],
    ign_lint_names: &["PropertyPersistenceRule"],
    ign_lint_option_map: &[
        ("expected_persistent", "expected-persistent"),
        ("exempt_binding_types", "exempt-binding-types"),
    ],
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&PropertyPersistence)],
    options: Some(parse_options),
};

const TOKENS: &[&str] = &[
    "expr",
    "expr-struct",
    "property",
    "query",
    "tag",
    "tag.direct",
    "tag.indirect",
    "tag.expression",
    "tag-history",
    "http",
];

struct Options {
    expected: Option<bool>,
    exempt: Vec<String>,
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let expected = match o.raw("expected-persistent") {
        None => None,
        Some(_) => Some(o.bool("expected-persistent")?),
    };
    let exempt = o.string_list("exempt-binding-types")?;
    for t in &exempt {
        if !TOKENS.contains(&t.as_str()) {
            return Err(format!(
                "unknown binding type `{t}` in `exempt-binding-types` (expected one of {})",
                TOKENS.join(", ")
            ));
        }
    }
    Ok(Box::new(Options { expected, exempt }))
}

struct PropertyPersistence;

impl ViewRule for PropertyPersistence {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let options = cx.options::<Options>();
        let Some(expected) = options.expected else {
            return;
        };
        let exempt = options.exempt.clone();
        let ModelData::View { file, .. } = cx.data() else {
            return;
        };
        let file = *file;
        for pc in view
            .prop_configs
            .iter()
            .filter(|c| c.path.category == "custom" && !c.path.name.is_empty())
        {
            let Some(binding_id) = pc.binding else {
                continue;
            };
            let binding = view.binding(binding_id);
            if is_exempt(
                &exempt,
                binding.kind,
                &binding.typed,
                binding.type_value.map(|t| t.value()),
            ) {
                continue;
            }
            let persistent = pc.persistent != Some(false);
            let full = ign_full_path(view, pc.owner, pc.key.value());
            let stored = owner_object(view, pc.owner).and_then(|o| lookup(o, pc.key.value()));
            let (message, ign) = match (expected, persistent) {
                (false, true) => (
                    format!("Bound property `{}` is persistent", pc.key.value()),
                    "bound property is persistent; set persistent=false to stop storing designer results in view.json",
                ),
                (true, false) => (
                    format!("Bound property `{}` is not persistent", pc.key.value()),
                    "bound property is not persistent; this project expects persistent=true",
                ),
                (true, true) if stored.is_none() => (
                    format!(
                        "Persistent bound property `{}` has no stored default value",
                        pc.key.value()
                    ),
                    "persistent bound property has no stored default value",
                ),
                _ => continue,
            };
            let fix = if !expected {
                fix(cx, view, file, pc, stored, &full)
            } else {
                None
            };
            cx.report_in(file, pc.key.range(), message)
                .node_path(pc.node_path.clone())
                .ign_lint_path(Some(full))
                .ign_lint_message(ign)
                .fix_opt(fix);
        }
    }
}

fn is_exempt(
    exempt: &[String],
    kind: BindingKind,
    typed: &BindingConfig<'_>,
    raw: Option<&str>,
) -> bool {
    let name = match kind {
        BindingKind::Unknown => raw.unwrap_or(""),
        k => k.name(),
    };
    if exempt.iter().any(|e| e == name) {
        return true;
    }
    if let BindingConfig::Tag(t) = typed {
        let mode = match t.mode {
            TagMode::Direct => "tag.direct",
            TagMode::Indirect => "tag.indirect",
            TagMode::Expression => "tag.expression",
            TagMode::Other => return false,
        };
        return exempt.iter().any(|e| e == mode);
    }
    false
}

fn fix(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    file: FileId,
    pc: &PropConfig<'_>,
    stored: Option<&iglint_json::Value<'_>>,
    full: &str,
) -> Option<Fix> {
    let editor = cx.json_editor(file, view.root)?;
    let mut edits: Vec<TextEdit> = Vec::new();
    edits.push(match pc.value.get("persistent") {
        Some(v) => editor.replace_value(v, "false"),
        None => editor
            .insert_member(pc.value, InsertPosition::Sorted, "persistent", "false")
            .ok()?,
    });
    let key = pc.key.value();
    if stored.is_some() && !key.contains('[') {
        let (parent_path, last) = key.rsplit_once('.')?;
        let owner = owner_object(view, pc.owner)?;
        let parent = lookup(owner, parent_path)?;
        edits.push(editor.remove_member_by_key(parent, last).ok()?);
    }
    Some(Fix::safe(
        format!("Make bound property '{full}' non-persistent"),
        cx.file_edits(file, edits),
    ))
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    const VIEW: &str = r#"{
  "custom": {
    "kept": 1,
    "rows": [
      1,
      2
    ]
  },
  "propConfig": {
    "custom.rows": {
      "binding": {
        "config": {
          "queryPath": "Rows"
        },
        "type": "query"
      }
    },
    "custom.tagged": {
      "binding": {
        "config": {
          "tagPath": "[default]A"
        },
        "type": "tag"
      },
      "persistent": false
    },
    "custom.volatile": {
      "binding": {
        "config": {
          "expression": "now(0)"
        },
        "type": "expr"
      },
      "persistent": false
    }
  },
  "root": {
    "meta": {
      "name": "root"
    },
    "type": "ia.container.flex"
  }
}"#;

    fn lint(options: &str) -> Vec<String> {
        Fixture::view(VIEW)
            .lint_with(&[("VIEW013", options)])
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn inert_without_expectation() {
        assert!(lint("").is_empty());
    }

    #[test]
    fn expected_false() {
        assert_eq!(
            lint("expected-persistent = false"),
            vec!["Bound property `custom.rows` is persistent"]
        );
    }

    #[test]
    fn expected_true() {
        assert_eq!(
            lint("expected-persistent = true"),
            vec!["Bound property `custom.volatile` is not persistent"]
        );
        assert_eq!(
            lint("expected-persistent = true\nexempt-binding-types = []"),
            vec![
                "Bound property `custom.tagged` is not persistent",
                "Bound property `custom.volatile` is not persistent"
            ]
        );
        assert_eq!(
            lint("expected-persistent = true\nexempt-binding-types = [\"expr\", \"tag.direct\"]"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn missing_default_value() {
        let view = r#"{"propConfig": {"custom.x": {"binding": {"type": "expr", "config": {"expression": "1"}}}}}"#;
        let found = Fixture::view(view).lint_with(&[("VIEW013", "expected-persistent = true")]);
        assert_eq!(
            found[0].message,
            "Persistent bound property `custom.x` has no stored default value"
        );
    }

    #[test]
    fn fix_sets_persistent_false_and_drops_value() {
        let f = Fixture::view(VIEW);
        let fixed = f.fixed(
            &[("VIEW013", "expected-persistent = false")],
            false,
            &f.view_path("Main"),
        );
        let expected = VIEW
            .replace(
                "    \"kept\": 1,\n    \"rows\": [\n      1,\n      2\n    ]\n",
                "    \"kept\": 1\n",
            )
            .replace(
                "        \"type\": \"query\"\n      }\n    },",
                "        \"type\": \"query\"\n      },\n      \"persistent\": false\n    },",
            );
        assert_eq!(fixed, expected);
    }
}
