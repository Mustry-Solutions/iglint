//! VIEW012 property-access.

use iglint_json::InsertPosition;
use iglint_project::Owner;
use iglint_project::view::{PropCategory, View};

use super::props::{def_range, ign_full_path, owner_object, owner_prop_config, wildcard_match};
use crate::checker::Checker;
use crate::diagnostic::{Fix, Severity};
use crate::options::OptionReader;
use crate::registry::{Category, OptionMeta, OptionType, RuleMeta};
use crate::rule::{Hook, ParsedOptions, Phase, RuleImpl, ViewRule};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW012",
    name: "property-access",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: false,
    fix: crate::registry::FixAvailability::Sometimes,
    summary: "Custom property access mode differs from the project convention",
    explanation: r#"## What it does

Checks the `access` mode of every top-level custom property of the view and
of its components against `expected-access`. The access mode is the
`access` of the property's `propConfig` entry (`custom.<name>`); a property
without one is `PUBLIC`. Nothing is checked until `expected-access` is set.

## Why is this bad?

Projects that keep view internals private (or public, for embedding) want
every custom property to follow the same rule. A property with the wrong
access is either reachable from places that should not depend on it, or
hidden from embedding views and scripts that need it.

## Example

With `expected-access = "PRIVATE"`:

```json
{"custom": {"rows": []}}
```

Use instead:

```json
{"custom": {"rows": []}, "propConfig": {"custom.rows": {"access": "PRIVATE"}}}
```

## Options

* `expected-access`: `PUBLIC`, `PROTECTED` or `PRIVATE` (case-insensitive).
  Unset: the rule reports nothing.
* `exempt-props`: patterns (`*` and `?` wildcards, everything else literal)
  matched against the property name, `custom.<name>` and the full ign-lint
  path (`root.root.children[0].Card.custom.x`).

## Fix

Safe: sets `access` in the existing `propConfig` entry, adds the entry (and
the `propConfig` object when the owner has none), or for `PUBLIC` removes
the `access` key (or the whole entry when `access` was its only key).

## ign-lint

Equivalent of `PropertyAccessRule` (`expected_access`, `exempt_props`), with
the same messages in `iglint ign-lint`.
"#,
    options: &[
        OptionMeta {
            name: "expected-access",
            ty: OptionType::String,
            default: "none",
            description: "PUBLIC, PROTECTED or PRIVATE; unset disables the rule.",
        },
        OptionMeta {
            name: "exempt-props",
            ty: OptionType::StringList,
            default: "[]",
            description: "Wildcard patterns of properties that are not checked.",
        },
    ],
    ign_lint_names: &["PropertyAccessRule"],
    ign_lint_option_map: &[
        ("expected_access", "expected-access"),
        ("exempt_props", "exempt-props"),
    ],
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&PropertyAccess)],
    options: Some(parse_options),
};

struct Options {
    expected: Option<String>,
    exempt: Vec<String>,
}

fn parse_options(o: &OptionReader<'_>) -> Result<ParsedOptions, String> {
    let expected = match o.opt_string("expected-access")? {
        None => None,
        Some(e) => {
            let upper = e.to_ascii_uppercase();
            if !matches!(upper.as_str(), "PUBLIC" | "PROTECTED" | "PRIVATE") {
                return Err(format!(
                    "`expected-access` must be PUBLIC, PROTECTED or PRIVATE, found `{e}`"
                ));
            }
            Some(upper)
        }
    };
    Ok(Box::new(Options {
        expected,
        exempt: o.string_list("exempt-props")?,
    }))
}

struct PropertyAccess;

impl ViewRule for PropertyAccess {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let options = cx.options::<Options>();
        let Some(expected) = options.expected.clone() else {
            return;
        };
        let exempt = options.exempt.clone();
        let Some(file) = view_file(cx) else {
            return;
        };
        for def in view
            .properties
            .iter()
            .filter(|p| p.category == PropCategory::Custom)
        {
            let key = format!("custom.{}", def.name);
            let full = ign_full_path(view, def.owner, &key);
            if exempt.iter().any(|p| {
                wildcard_match(p, def.name) || wildcard_match(p, &key) || wildcard_match(p, &full)
            }) {
                continue;
            }
            let entry = view
                .prop_configs
                .iter()
                .rev()
                .find(|c| c.owner == def.owner && c.key.value() == key);
            let actual = entry
                .and_then(|c| c.access)
                .map_or_else(|| "PUBLIC".to_owned(), str::to_ascii_uppercase);
            if actual == expected {
                continue;
            }
            let fix = fix(cx, view, file, def.owner, &key, entry, &expected, &full);
            cx.report_in(
                file,
                def_range(view, def),
                format!(
                    "Custom property `{}` has access {actual}, expected {expected}",
                    def.name
                ),
            )
            .node_path(def.node_path.clone())
            .ign_lint_path(Some(full.clone()))
            .ign_lint_message(format!(
                "custom property access mode is {actual}, expected {expected}"
            ))
            .fix_opt(fix);
        }
    }
}

fn view_file(cx: &Checker<'_, '_>) -> Option<iglint_project::FileId> {
    match cx.data() {
        iglint_project::ModelData::View { file, .. } => Some(*file),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn fix(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    file: iglint_project::FileId,
    owner: Owner,
    key: &str,
    entry: Option<&iglint_project::view::PropConfig<'_>>,
    expected: &str,
    full: &str,
) -> Option<Fix> {
    let editor = cx.json_editor(file, view.root)?;
    let quoted = editor.quote(expected);
    let edit = match entry {
        Some(entry) => {
            let object = entry.value;
            match object.get("access") {
                Some(access) if expected == "PUBLIC" => {
                    if object.as_object().is_some_and(|o| o.len() == 1) {
                        let pc = owner_prop_config(view, owner)?;
                        editor.remove_member_by_key(pc, key).ok()?
                    } else {
                        let _ = access;
                        editor.remove_member_by_key(object, "access").ok()?
                    }
                }
                Some(access) => editor.replace_value(access, &quoted),
                None => editor
                    .insert_member(object, InsertPosition::Sorted, "access", &quoted)
                    .ok()?,
            }
        }
        None if expected == "PUBLIC" => return None,
        None => {
            let entry_json = format!("{{\n  \"access\": {quoted}\n}}");
            match owner_prop_config(view, owner) {
                Some(pc) => editor
                    .insert_member(pc, InsertPosition::Sorted, key, &entry_json)
                    .ok()?,
                None => {
                    let object = owner_object(view, owner)?;
                    let json = format!(
                        "{{\n  {}: {{\n    \"access\": {quoted}\n  }}\n}}",
                        editor.quote(key)
                    );
                    editor
                        .insert_member(object, InsertPosition::Sorted, "propConfig", &json)
                        .ok()?
                }
            }
        }
    };
    Some(Fix::safe(
        format!("Set access mode of '{full}' to {expected}"),
        cx.file_edits(file, vec![edit]),
    ))
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn lint(view: &str, options: &str) -> Vec<(String, String)> {
        Fixture::view(view)
            .lint_with(&[("VIEW012", options)])
            .iter()
            .map(|d| {
                (
                    d.message.clone(),
                    d.ign_lint_path.clone().unwrap_or_default(),
                )
            })
            .collect()
    }

    const VIEW: &str = r#"{
  "custom": {
    "a": 1,
    "b": 2
  },
  "propConfig": {
    "custom.b": {
      "access": "PRIVATE"
    },
    "custom.c": {
      "binding": {"type": "expr", "config": {"expression": "1"}}
    }
  },
  "root": {
    "custom": {
      "x": 1
    },
    "meta": {
      "name": "root"
    },
    "type": "ia.container.flex"
  }
}"#;

    #[test]
    fn inert_without_expected_access() {
        assert!(lint(VIEW, "").is_empty());
    }

    #[test]
    fn reports_mismatches() {
        let found = lint(VIEW, "expected-access = \"private\"");
        assert_eq!(
            found,
            vec![
                (
                    "Custom property `a` has access PUBLIC, expected PRIVATE".into(),
                    "custom.a".into()
                ),
                (
                    "Custom property `c` has access PUBLIC, expected PRIVATE".into(),
                    "custom.c".into()
                ),
                (
                    "Custom property `x` has access PUBLIC, expected PRIVATE".into(),
                    "root.root.custom.x".into()
                ),
            ]
        );
        let public = lint(VIEW, "expected-access = \"PUBLIC\"");
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].1, "custom.b");
    }

    #[test]
    fn exemptions() {
        let found = lint(
            VIEW,
            "expected-access = \"PRIVATE\"\nexempt-props = [\"a\", \"root.root.*\", \"custom.c\"]",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn invalid_option_falls_back() {
        let f = Fixture::view(VIEW);
        let settings = settings(f.root(), &[("VIEW012", "expected-access = \"secret\"")]);
        let result = crate::engine::check(
            &settings,
            &[f.root().to_path_buf()],
            &crate::engine::CheckOptions::default(),
        );
        assert!(result.diagnostics.is_empty());
        assert!(result.errors[0].message.contains("expected-access"));
    }

    #[test]
    fn fixes() {
        let f = Fixture::view(VIEW);
        let fixed = f.fixed(
            &[("VIEW012", "expected-access = \"PRIVATE\"")],
            false,
            &f.view_path("Main"),
        );
        assert_eq!(
            fixed,
            r#"{
  "custom": {
    "a": 1,
    "b": 2
  },
  "propConfig": {
    "custom.a": {
      "access": "PRIVATE"
    },
    "custom.b": {
      "access": "PRIVATE"
    },
    "custom.c": {
      "access": "PRIVATE",
      "binding": {"type": "expr", "config": {"expression": "1"}}
    }
  },
  "root": {
    "custom": {
      "x": 1
    },
    "meta": {
      "name": "root"
    },
    "propConfig": {
      "custom.x": {
        "access": "PRIVATE"
      }
    },
    "type": "ia.container.flex"
  }
}"#
        );
        let public = f.fixed(
            &[("VIEW012", "expected-access = \"PUBLIC\"")],
            false,
            &f.view_path("Main"),
        );
        assert!(!public.contains("PRIVATE"));
        assert!(
            public.contains("\"propConfig\": {\n    \"custom.c\""),
            "{public}"
        );
    }
}
