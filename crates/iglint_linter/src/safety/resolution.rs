//! The reference-resolution map of a view, compared by the safety net
//! before and after the safe fixes of a `view.json` (review round 21, B1).
//!
//! Every reference of the view, in document order: the property
//! references of every modelled expression and of property and indirect
//! tag bindings (resolved as `BIND005` resolves them), tag paths, the
//! `getSibling`/`getChild` lookups of every script (followed as `SCR305`
//! follows them), and the literal view paths. Each entry holds what the
//! reference resolves to, with components identified by their place in
//! the view (their JSON pointer), not by their names. So a rename that
//! updates every reference to the renamed component leaves the map as it
//! was, and anything else a fix does to resolution (a reference that
//! found nothing now finds a component, one that found a component now
//! finds another or nothing) shows as a difference, and the net drops the
//! fix.

use std::path::Path;
use std::sync::Arc;

use iglint_expression::Reference;
use iglint_project::view::{BindingConfig, RefResolution, RefTarget, Unresolved, View};
use iglint_project::{JsonDocument, Owner};

use crate::script_facts::AccessTarget;

/// One reference: where it is and what it resolves to.
#[derive(Clone, Debug)]
struct Entry {
    /// The kind of reference, its owner and its position among the
    /// references of the view: what identifies it across the fix.
    key: String,
    /// The reference as written, for the message only (a rename changes
    /// it).
    text: String,
    /// What it resolves to.
    resolution: String,
}

/// The resolution map of a view.
#[derive(Clone, Debug)]
pub(crate) struct Resolutions(Vec<Entry>);

impl Resolutions {
    /// The map of `text` when `path` is a `view.json` that parses; `None`
    /// for other files.
    pub(crate) fn of(path: &Path, text: &str) -> Option<Resolutions> {
        if path.file_name().and_then(|n| n.to_str()) != Some("view.json") {
            return None;
        }
        let doc = JsonDocument::parse(Arc::from(text));
        if doc.error().is_some() {
            return Some(Resolutions(vec![Entry {
                key: "document".to_owned(),
                text: String::new(),
                resolution: "does not parse".to_owned(),
            }]));
        }
        let root = doc.root()?;
        let view = iglint_project::view::build_view(root);
        Some(Resolutions(entries(&view)))
    }

    /// The first difference between the map before a fix (`self`) and
    /// after it, described.
    pub(crate) fn difference(&self, after: &Resolutions) -> Option<String> {
        if self.0.len() != after.0.len() {
            return Some("it would add or remove a reference of the view".to_owned());
        }
        for (b, a) in self.0.iter().zip(&after.0) {
            if b.key != a.key {
                return Some("it would add or remove a reference of the view".to_owned());
            }
            if b.resolution != a.resolution {
                return Some(format!(
                    "it would change what {} resolves to: {}, then {}",
                    a.text, b.resolution, a.resolution
                ));
            }
        }
        None
    }
}

fn entries(view: &View<'_>) -> Vec<Entry> {
    let mut out = Vec::new();
    let pointer =
        |c: iglint_project::ComponentId| format!("component `{}`", view.component(c).pointer);
    let owner = |o: Owner| match o {
        Owner::Component(c) => pointer(c),
        Owner::View => "the view".to_owned(),
        other => format!("{other:?}"),
    };
    let property = |from: Owner, r: &iglint_expression::PropertyRef| -> String {
        match view.resolve_ref(from, r) {
            RefResolution::Resolved(r) => format!(
                "{}{}{}{}{}{}",
                match r.target {
                    RefTarget::View => "the view".to_owned(),
                    RefTarget::Component(c) => pointer(c),
                },
                r.category.map(|c| format!(" {c}")).unwrap_or_default(),
                r.name.map(|n| format!(" `{n}`")).unwrap_or_default(),
                if r.definition.is_some() {
                    " (defined)"
                } else {
                    ""
                },
                if r.ambiguous { " (ambiguous)" } else { "" },
                if r.unchecked { "" } else { " (checked)" },
            ),
            RefResolution::Unresolved(u) => match u {
                Unresolved::NoComponent => "unresolved (no component)".to_owned(),
                Unresolved::AboveRoot { up } => format!("unresolved ({up} above the root)"),
                Unresolved::MissingComponent { name, .. } => {
                    format!("unresolved (no component `{name}`)")
                }
                Unresolved::MissingProperty { category, name, .. } => {
                    format!("unresolved (no {category} `{name}`)")
                }
            },
            RefResolution::Scope(s) => format!(
                "{} {} {}",
                s.scope,
                s.category.unwrap_or_default(),
                s.name.unwrap_or_default()
            ),
            RefResolution::Dynamic => "dynamic".to_owned(),
        }
    };
    let mut push = |kind: &str, from: Owner, text: &str, resolution: String| {
        let key = format!("{kind} {} of {}", out.len(), owner(from));
        out.push(Entry {
            key,
            text: text.to_owned(),
            resolution,
        });
    };
    for e in &view.expressions {
        for (bound, r) in e.references() {
            let resolution = match &r {
                Reference::Property(p) => property(e.owner, p),
                Reference::Tag(_) => format!("tag `{}`", bound.path),
                Reference::TransformValue => "the transform's value".to_owned(),
                Reference::Other(o) => format!("`{o}`"),
            };
            push(
                "expression reference",
                e.owner,
                &format!("`{}`", bound.path),
                resolution,
            );
        }
    }
    for b in &view.bindings {
        match &b.typed {
            BindingConfig::Property(p) => {
                let Some(path) = p.path else { continue };
                let resolution = match &p.reference {
                    Some(r) => property(b.owner, r),
                    None => format!("not a property reference: `{}`", path.value()),
                };
                push(
                    "property binding",
                    b.owner,
                    &format!("`{}`", path.value()),
                    resolution,
                );
            }
            BindingConfig::Tag(t) => {
                if let Some(path) = t.tag_path {
                    push(
                        "tag binding",
                        b.owner,
                        &format!("`{}`", path.value()),
                        format!("tag `{}`", path.value()),
                    );
                }
                for ir in &t.references {
                    let text = ir.value.as_json_string().map_or("", |s| s.value());
                    let resolution = match &ir.reference {
                        Some(r) => property(b.owner, r),
                        None => format!("`{text}`"),
                    };
                    push(
                        "indirect tag reference",
                        b.owner,
                        &format!("`{text}`"),
                        resolution,
                    );
                }
            }
            _ => {}
        }
    }
    for s in &view.scripts {
        // Only a script that names a lookup method can hold one.
        if !(s.source.contains("getSibling") || s.source.contains("getChild")) {
            continue;
        }
        let parsed = iglint_python::parse_module(&s.source);
        for (receiver, result) in
            crate::script_facts::access::component_lookups(s.owner, &parsed, view)
        {
            let describe = |t: Option<AccessTarget>| match t {
                Some(AccessTarget::Component(c)) => pointer(c),
                Some(AccessTarget::View) => "the view".to_owned(),
                Some(AccessTarget::AnyComponent) => "any component".to_owned(),
                None => "nothing found or not followed".to_owned(),
            };
            push(
                "script lookup",
                s.owner,
                &format!("a component lookup in {}", s.node_path),
                format!("{} on {}", describe(result), describe(receiver)),
            );
        }
    }
    for r in &view.view_refs {
        push(
            "view path",
            r.owner,
            &format!("`{}`", r.path.value()),
            format!("view `{}`", r.path.value()),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(old: &str, children: &str) -> String {
        format!(
            r#"{{"root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{{"meta": {{"name": "{old}"}}, "type": "ia.display.label"}}, {children}]}}}}"#
        )
    }

    fn map(text: &str) -> Resolutions {
        Resolutions::of(Path::new("view.json"), text).unwrap()
    }

    const LOOKUP: &str = r#"{"meta": {"name": "Btn"}, "type": "ia.input.button", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlbl = self.getSibling('MyLabel')\n\tprint lbl is None"}}}}}"#;
    const BINDING: &str = r#"{"meta": {"name": "Other"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "property", "config": {"path": "../MyLabel.props.text"}}}}}"#;
    const EXPRESSION: &str = r#"{"meta": {"name": "Other"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "coalesce({/root/MyLabel.props.text}, 'none')"}}}}}"#;

    /// Review round 21, B1: renaming a component to the name that broken
    /// references use makes them resolve, which the map shows; a rename
    /// that updates every reference to the component does not change it.
    #[test]
    fn a_rename_that_makes_references_resolve_changes_the_map() {
        for (old, children) in [
            ("my_label", format!("{LOOKUP}, {BINDING}")),
            ("my_label", EXPRESSION.to_owned()),
            ("myLabel", LOOKUP.to_owned()),
        ] {
            let before = view(old, &children);
            let after = before.replace(&format!(r#""name": "{old}""#), r#""name": "MyLabel""#);
            let problem = map(&before).difference(&map(&after));
            assert!(
                problem
                    .as_deref()
                    .is_some_and(|p| p.contains("resolves to")),
                "{problem:?}"
            );
            // The same references to the component under its old name are
            // updated by the rename: nothing changes.
            let consistent = before.replace("MyLabel", old);
            let renamed = consistent.replace(old, "MyLabel");
            assert_eq!(map(&consistent).difference(&map(&renamed)), None);
        }
        // Not a view: no map.
        assert!(Resolutions::of(Path::new("data.json"), "{}").is_none());
    }
}
