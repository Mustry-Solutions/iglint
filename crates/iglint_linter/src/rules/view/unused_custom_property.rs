//! VIEW015 unused-custom-property.

use iglint_expression::{Reference, parse_property_ref};
use iglint_project::view::{
    BindingConfig, ParamDirection, PropCategory, PropertyDef, RefResolution, RefTarget, View,
};
use iglint_project::{ModelData, Owner};
use iglint_text::TextSize;

use super::props::{def_range, owner_object, owner_prop_config};
use crate::checker::Checker;
use crate::diagnostic::{Fix, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ViewRule};
use crate::script_facts::{AccessTarget, PropertyAccess};

pub static META: RuleMeta = RuleMeta {
    code: "VIEW015",
    name: "unused-custom-property",
    category: Category::View,
    default_severity: Severity::Warning,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    summary: "Custom property or view parameter is never read",
    explanation: r#"## What it does

Reports custom properties (of the view and of its components) and view
input parameters that nothing in the view reads.

A property counts as read when it is resolved by an exact reference, not
by a text search:

* `{...}` references in any expression of the view (expression bindings,
  expression transforms, expression-structure leaves, query parameters,
  tag expressions, map transforms), resolved against the component tree
  (`{this.custom.x}`, `{view.params.p}`, `{../Card.custom.x}`,
  `{/root/Card.custom.x}`); `{view.custom}` reads every view custom
  property;
* property binding paths and indirect tag binding references;
* scripts of the view (events, transforms, change scripts, message
  handlers, custom methods, extension functions), parsed with the Jython
  parser: `self.view.custom.x`, `self.custom.x`, `self.getSibling('A').custom.x`,
  `self.parent.getChild('A').custom.x`, `self.custom['x']`, and local
  aliases (`view = self.view`). A script that uses a property container in
  a way that cannot be followed (`self.view.custom[key]`, passing `self`
  to a library function, a script that does not parse) counts as reading
  every property it could reach, so the rule stays silent rather than
  guess.

These never count as unused:

* properties with a binding or a change script (`propConfig` `binding` or
  `onChange`): the binding or script runs whether or not anything reads
  the value;
* `output` parameters (whether they are written is `VIEW019`'s concern);
  `inout` parameters that the view writes;
* names starting with `_`, the convention for intentionally unused
  properties.

Input parameters set by an embedding view or a page URL are still
reported: passing a value in does not make the view use it.

## Why is this bad?

Unused properties are dead configuration: readers have to work out that
nothing depends on them, and unused parameters make callers pass values
that are ignored.

## Example

```json
{"custom": {"unusedCounter": 0, "title": "Pumps"},
 "root": {"type": "ia.display.label", "meta": {"name": "root"},
          "propConfig": {"props.text": {"binding": {"type": "property", "config": {"path": "view.custom.title"}}}}}}
```

`unusedCounter` is reported. Use instead: remove it.

## Fix

Safe, for custom properties: removes the property and its `propConfig`
entries. View parameters are never removed automatically (callers pass
them).

## ign-lint

Equivalent of `UnusedCustomPropertiesRule`, with the same messages in
`iglint ign-lint`. ign-lint searches substrings, so `view.custom.foo` is
credited by `{view.custom.foobar}`, `this.custom.x` anywhere credits every
component's `x`, same-named components share one definition, and component
`propConfig` entries become bogus definitions (bugs 21 to 23). iglint
resolves every reference instead.
"#,
    ign_lint_names: &["UnusedCustomPropertiesRule"],
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::View(&UnusedCustomProperty)],
    options: None,
};

struct UnusedCustomProperty;

/// What reads (or writes) which properties of a view.
pub(crate) struct Usage {
    /// Per entry of `view.properties`.
    pub read: Vec<bool>,
    pub written: Vec<bool>,
    /// Whole containers read or written.
    pub all_read: Vec<(Option<Owner>, PropCategory)>,
    pub all_written: Vec<(Option<Owner>, PropCategory)>,
    /// Names read or written on components that cannot be determined
    /// (`None` key in the tuple: every name).
    pub any_component_read: Vec<Option<String>>,
    pub any_component_written: Vec<Option<String>>,
}

impl Usage {
    /// Whether `def` (index `i`) is read.
    pub fn is_read(&self, i: usize, def: &PropertyDef<'_>) -> bool {
        self.read[i]
            || covers(&self.all_read, def)
            || (def.category == PropCategory::Custom
                && matches!(def.owner, Owner::Component(_))
                && self
                    .any_component_read
                    .iter()
                    .any(|n| n.as_deref().is_none_or(|n| n == def.name)))
    }

    /// Whether `def` (index `i`) is written.
    pub fn is_written(&self, i: usize, def: &PropertyDef<'_>) -> bool {
        self.written[i]
            || covers(&self.all_written, def)
            || (def.category == PropCategory::Custom
                && matches!(def.owner, Owner::Component(_))
                && self
                    .any_component_written
                    .iter()
                    .any(|n| n.as_deref().is_none_or(|n| n == def.name)))
    }
}

fn covers(all: &[(Option<Owner>, PropCategory)], def: &PropertyDef<'_>) -> bool {
    all.iter().any(|(o, c)| {
        *c == def.category
            && match o {
                Some(o) => *o == def.owner,
                None => matches!(def.owner, Owner::Component(_)),
            }
    })
}

fn credit(view: &View<'_>, usage: &mut Usage, resolution: RefResolution, write: bool) {
    let RefResolution::Resolved(r) = resolution else {
        return;
    };
    let owner = match r.target {
        RefTarget::View => Owner::View,
        RefTarget::Component(c) => Owner::Component(c),
    };
    let category = match r.category.as_deref() {
        Some("custom") => PropCategory::Custom,
        Some("params") => PropCategory::Params,
        _ => return,
    };
    match r.definition {
        Some(d) => {
            if write {
                usage.written[d] = true;
            } else {
                usage.read[d] = true;
            }
        }
        None if r.name.is_none() => {
            let list = if write {
                &mut usage.all_written
            } else {
                &mut usage.all_read
            };
            list.push((Some(owner), category));
        }
        None => {}
    }
    let _ = view;
}

/// Computes what the view reads and writes.
pub(crate) fn usage(view: &View<'_>, accesses: &[PropertyAccess]) -> Usage {
    let n = view.properties.len();
    let mut usage = Usage {
        read: vec![false; n],
        written: vec![false; n],
        all_read: Vec::new(),
        all_written: Vec::new(),
        any_component_read: Vec::new(),
        any_component_written: Vec::new(),
    };
    // Expressions.
    for e in &view.expressions {
        for (_, reference) in e.references() {
            if let Reference::Property(p) = reference {
                credit(view, &mut usage, view.resolve_ref(e.owner, &p), false);
            }
        }
        if !e.parsed.is_valid() {
            // A broken expression may hide references: credit every
            // property whose name it mentions.
            let text = e.text();
            for (i, def) in view.properties.iter().enumerate() {
                if text.contains(def.name) {
                    usage.read[i] = true;
                }
            }
        }
    }
    // Property bindings (read, and written back when bidirectional) and
    // indirect tag references.
    for b in &view.bindings {
        match &b.typed {
            BindingConfig::Property(p) => {
                if let Some(path) = p.path {
                    match parse_property_ref(path.value(), TextSize::ZERO) {
                        Some(r) => {
                            credit(view, &mut usage, view.resolve_ref(b.owner, &r), false);
                            if p.bidirectional {
                                credit(view, &mut usage, view.resolve_ref(b.owner, &r), true);
                            }
                        }
                        None => {
                            for (i, def) in view.properties.iter().enumerate() {
                                if path.value().contains(def.name) {
                                    usage.read[i] = true;
                                }
                            }
                        }
                    }
                }
            }
            BindingConfig::Tag(t) => {
                for r in &t.references {
                    match &r.reference {
                        Some(p) => credit(view, &mut usage, view.resolve_ref(b.owner, p), false),
                        None => {
                            if let Some(text) = r.value.as_str() {
                                for (i, def) in view.properties.iter().enumerate() {
                                    if text.contains(def.name) {
                                        usage.read[i] = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        // A binding writes the property it is on.
        if let Some((cat, name)) = match b.property.split_once('.') {
            Some(("custom", rest)) => Some((PropCategory::Custom, rest)),
            Some(("params", rest)) => Some((PropCategory::Params, rest)),
            _ => None,
        } {
            let name = name.split(['.', '[']).next().unwrap_or("");
            if let Some(i) = view
                .properties
                .iter()
                .position(|p| p.owner == b.owner && p.category == cat && p.name == name)
            {
                usage.written[i] = true;
            }
        }
    }
    // Scripts.
    for a in accesses {
        let owner = match a.target {
            AccessTarget::View => Some(Owner::View),
            AccessTarget::Component(c) => Some(Owner::Component(c)),
            AccessTarget::AnyComponent => None,
        };
        let Some(owner) = owner else {
            if a.category == PropCategory::Custom {
                if a.read {
                    usage.any_component_read.push(a.name.clone());
                }
                if a.write {
                    usage.any_component_written.push(a.name.clone());
                }
            }
            continue;
        };
        match &a.name {
            Some(name) => {
                if let Some(i) = view
                    .properties
                    .iter()
                    .position(|p| p.owner == owner && p.category == a.category && p.name == name)
                {
                    if a.read {
                        usage.read[i] = true;
                    }
                    if a.write {
                        usage.written[i] = true;
                    }
                }
            }
            None => {
                if a.read {
                    usage.all_read.push((Some(owner), a.category));
                }
                if a.write {
                    usage.all_written.push((Some(owner), a.category));
                }
            }
        }
    }
    usage
}

/// Whether a property has a binding or a change script in `propConfig`.
fn is_active(view: &View<'_>, def: &PropertyDef<'_>) -> bool {
    view.prop_configs.iter().any(|c| {
        c.owner == def.owner
            && c.path.category == def.category.name()
            && c.path.name == def.name
            && (c.binding.is_some() || c.on_change.is_some())
    })
}

impl ViewRule for UnusedCustomProperty {
    fn check_view(&self, cx: &mut Checker<'_, '_>, view: &View<'_>) {
        let ModelData::View { file, .. } = cx.data() else {
            return;
        };
        let file = *file;
        let (accesses, complete) = cx.script_property_accesses();
        if !complete && !view.scripts.is_empty() {
            return;
        }
        let usage = usage(view, accesses);
        for (i, def) in view.properties.iter().enumerate() {
            if def.name.starts_with('_') || def.name.is_empty() || is_active(view, def) {
                continue;
            }
            let (kind, ign_kind) = match def.category {
                PropCategory::Custom => ("Custom property", "custom property"),
                PropCategory::Params => match def.direction {
                    Some(ParamDirection::Output) => continue,
                    Some(ParamDirection::InOut) if usage.is_written(i, def) => continue,
                    _ => ("View parameter", "view parameter"),
                },
            };
            if usage.is_read(i, def) {
                continue;
            }
            let fix = match def.category {
                PropCategory::Custom => fix(cx, view, file, def),
                PropCategory::Params => None,
            };
            cx.report_in(
                file,
                def_range(view, def),
                format!("{kind} `{}` is never read", def.name),
            )
            .node_path(def.node_path.clone())
            .ign_lint_path(Some(def.ign_lint_path.clone()))
            .ign_lint_message(format!(
                "{ign_kind} '{}' is defined but never referenced",
                def.name
            ))
            .fix_opt(fix);
        }
    }
}

fn fix(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    file: iglint_project::FileId,
    def: &PropertyDef<'_>,
) -> Option<Fix> {
    let editor = cx.json_editor(file, view.root)?;
    let mut edits = Vec::new();
    if def.key.is_some() {
        let custom = owner_object(view, def.owner)?.get("custom")?;
        edits.push(editor.remove_member_by_key(custom, def.name).ok()?);
    }
    if let Some(pc) = owner_prop_config(view, def.owner)
        && let Some(o) = pc.as_object()
    {
        let exact = format!("custom.{}", def.name);
        let indices: Vec<usize> = o
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                let k = m.key();
                k == exact
                    || k.strip_prefix(exact.as_str())
                        .is_some_and(|r| r.starts_with('.') || r.starts_with('['))
            })
            .map(|(i, _)| i)
            .collect();
        if !indices.is_empty() {
            edits.extend(editor.remove_members(pc, &indices).ok()?);
        }
    }
    if edits.is_empty() {
        return None;
    }
    Some(Fix::safe(
        format!("Remove unused custom property '{}'", def.name),
        cx.file_edits(file, edits),
    ))
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::*;

    fn unused(view: &str) -> Vec<String> {
        Fixture::view(view)
            .lint(&["VIEW015"])
            .iter()
            .map(|d| d.node_path.clone().unwrap())
            .collect()
    }

    fn label(name: &str, extra: &str) -> String {
        format!(r#"{{"meta": {{"name": "{name}"}}, "type": "ia.display.label"{extra}}}"#)
    }

    fn view(custom: &str, params: &str, children: &[String], prop_config: &str) -> String {
        format!(
            r#"{{"custom": {custom}, "params": {params}, "propConfig": {prop_config},
  "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{}]}}}}"#,
            children.join(",")
        )
    }

    #[test]
    fn reports_unread_properties() {
        let v = view(
            r#"{"used": 1, "unused": 2, "_private": 3}"#,
            r#"{"inParam": 1, "readParam": 2}"#,
            &[label(
                "A",
                r#", "custom": {"x": 1, "y": 2}, "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{view.custom.used} + {view.params.readParam} + {this.custom.x}"}}}}"#,
            )],
            r#"{"params.inParam": {"paramDirection": "input"}, "params.readParam": {"paramDirection": "input"}}"#,
        );
        assert_eq!(
            unused(&v),
            vec![
                "view.custom.unused",
                "view.params.inParam",
                "root/A.custom.y"
            ]
        );
    }

    #[test]
    fn exact_matching_not_prefixes() {
        let v = view(
            r#"{"foo": 1, "foobar": 2}"#,
            "{}",
            &[label(
                "A",
                r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{view.custom.foobar}"}}}}"#,
            )],
            "{}",
        );
        assert_eq!(unused(&v), vec!["view.custom.foo"]);
    }

    #[test]
    fn this_custom_only_credits_its_own_component() {
        let v = view(
            "{}",
            "{}",
            &[
                label(
                    "A",
                    r#", "custom": {"x": 1}, "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{this.custom.x}"}}}}"#,
                ),
                label("B", r#", "custom": {"x": 1}"#),
            ],
            "{}",
        );
        assert_eq!(unused(&v), vec!["root/B.custom.x"]);
    }

    #[test]
    fn relative_absolute_and_property_bindings() {
        let v = view(
            r#"{"viaPath": 1}"#,
            "{}",
            &[
                label("A", r#", "custom": {"x": 1, "z": 1}"#),
                label(
                    "B",
                    r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "{../A.custom.x}"}}}, "props.tooltip": {"binding": {"type": "property", "config": {"path": "/root/A.custom.z"}}}, "meta.visible": {"binding": {"type": "property", "config": {"path": "view.custom.viaPath"}}}}"#,
                ),
            ],
            "{}",
        );
        assert!(unused(&v).is_empty(), "{:?}", unused(&v));
    }

    #[test]
    fn bound_and_watched_properties_and_output_params_are_not_reported() {
        let v = view(
            r#"{"watched": 1}"#,
            r#"{"out": 1, "io": 1}"#,
            &[],
            r#"{"custom.bound": {"binding": {"type": "expr", "config": {"expression": "1"}}}, "custom.watched": {"onChange": {"enabled": true, "script": "\tpass"}}, "params.out": {"paramDirection": "output"}, "params.io": {"paramDirection": "inout"}}"#,
        );
        // `io` is neither read nor written.
        assert_eq!(unused(&v), vec!["view.params.io"]);
    }

    #[test]
    fn scripts_are_followed() {
        let event = |script: &str| {
            format!(
                r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
            )
        };
        let v = view(
            r#"{"a": 1, "b": 2, "c": 3}"#,
            "{}",
            &[
                label("A", r#", "custom": {"x": 1}"#),
                label(
                    "Btn",
                    &event(
                        "\\tv = self.view\\n\\tprint v.custom.a, self.getSibling('A').custom.x\\n\\tself.view.custom.b = 1",
                    ),
                ),
            ],
            "{}",
        );
        // `b` is only written.
        assert_eq!(unused(&v), vec!["view.custom.b", "view.custom.c"]);
        let dynamic = view(
            r#"{"a": 1}"#,
            "{}",
            &[label("Btn", &event("\\tprint self.view.custom[name]"))],
            "{}",
        );
        assert!(unused(&dynamic).is_empty());
        let escaping = view(
            r#"{"a": 1}"#,
            "{}",
            &[label("Btn", &event("\\tlib.handle(self)"))],
            "{}",
        );
        assert!(unused(&escaping).is_empty());
        let broken = view(
            r#"{"a": 1}"#,
            "{}",
            &[label("Btn", &event("\\tif :"))],
            "{}",
        );
        assert!(unused(&broken).is_empty());
    }

    #[test]
    fn whole_container_reference_and_tag_references() {
        let v = view(
            r#"{"a": 1, "b": 2}"#,
            r#"{"p": 1}"#,
            &[label(
                "A",
                r#", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "toStr({view.custom})"}}}, "props.data": {"binding": {"type": "tag", "config": {"mode": "indirect", "tagPath": "[default]{m}/x", "references": {"m": "{view.params.p}"}}}}}"#,
            )],
            "{}",
        );
        assert!(unused(&v).is_empty());
    }

    #[test]
    fn fix_removes_property_and_config() {
        let text = r#"{
  "custom": {
    "keep": 1,
    "unused": 2
  },
  "propConfig": {
    "custom.keep": {
      "persistent": true
    },
    "custom.unused": {
      "persistent": false
    },
    "custom.unused.deep": {
      "access": "PRIVATE"
    }
  },
  "root": {
    "meta": {
      "name": "root"
    },
    "propConfig": {
      "props.text": {
        "binding": {
          "config": {
            "path": "view.custom.keep"
          },
          "type": "property"
        }
      }
    },
    "type": "ia.display.label"
  }
}"#;
        let f = Fixture::view(text);
        let fixed = f.fixed(&[("VIEW015", "")], false, &f.view_path("Main"));
        assert_eq!(
            fixed,
            text.replace(",\n    \"unused\": 2", "").replace(
                ",\n    \"custom.unused\": {\n      \"persistent\": false\n    },\n    \"custom.unused.deep\": {\n      \"access\": \"PRIVATE\"\n    }",
                ""
            )
        );
    }
}
