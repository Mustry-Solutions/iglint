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
use crate::rules::refs::unknown_library_member::resolve;
use crate::script_facts::access::{Callee, CalleeContext, LibraryCall, follow_call};
use crate::script_facts::{AccessTarget, PropertyAccess};
use iglint_project::library::{LibraryLookup, SymbolKind};
use iglint_project::script::IMPLICIT_GLOBALS;
use iglint_project::{ProjectId, ProjectIndex};

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
  to a function iglint cannot follow, a script that does not parse) counts
  as reading every property it could reach, so the rule stays silent
  rather than guess;
* project library functions a script passes `self`, the view or a
  property container to (`lib.util.handle(self)`, `handle(view=self.view)`):
  the function's body is followed with the same rules (`param.view.custom.x`,
  `param.getSibling('A').custom.y`, passing it on to other library
  functions, up to 4 calls deep). A function that does not exist reads
  nothing (the call fails, which `REF002` reports). A function that uses
  the value in a way that cannot be followed, a function a child project
  overrides, a function that may be replaced at run time (its module runs
  `exec`, `eval` or `execfile`, writes `globals()` or
  `sys.modules[__name__]`, sets its `func_code`, or may overwrite it with
  a later definition or `from x import *`; a script assigns or `setattr`s
  it; its module is used as a value or reached through `sys.modules` or
  code run from strings), and calls of anything but a library function
  count as reading everything.

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

For custom properties: removes the property and its `propConfig`
entries. Safe when nothing writes the property; unsafe when a script or a
bidirectional binding writes it (the write recreates the property, without
the `access` and `persistent` settings of its `propConfig`). View
parameters are never removed automatically (callers pass them).

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
    // Library functions that scripts hand `self` to are resolved through
    // the project index.
    phase: Phase::Project,
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

/// Property references written as `{path}` inside a string.
fn braced_refs(text: &str) -> Vec<iglint_expression::PropertyRef> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find(['{', '}']) else {
            break;
        };
        if after.as_bytes()[end] == b'}'
            && let Some(r) = parse_property_ref(&after[..end], TextSize::ZERO)
        {
            out.push(r);
        }
        rest = &after[end..];
    }
    out
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
    // `{...}` references in any other string of the view: settings of
    // bindings that are not modelled as expressions (tag history date
    // ranges, HTTP parameters), action parameters, ... Crediting them can
    // only hide findings, never add one.
    let mut braced: Vec<(Owner, iglint_expression::PropertyRef)> = Vec::new();
    let mut collect = |owner: Owner, value: &iglint_json::Value<'_>, skip: &str| {
        let Some(o) = value.as_object() else {
            return;
        };
        for m in o.iter().filter(|m| m.key() != skip) {
            m.value.walk(|_, v| {
                if let Some(text) = v.as_str() {
                    braced.extend(braced_refs(text).into_iter().map(|r| (owner, r)));
                }
                true
            });
        }
    };
    collect(Owner::View, view.root, "root");
    for c in &view.components {
        collect(Owner::Component(c.id), c.value, "children");
    }
    for (owner, r) in braced {
        credit(view, &mut usage, view.resolve_ref(owner, &r), false);
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
        let accesses = follow_library_calls(cx, view, accesses);
        let usage = usage(view, &accesses);
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
                PropCategory::Custom => fix(cx, view, file, def, usage.is_written(i, def)),
                PropCategory::Params => None,
            };
            let message = if usage.is_written(i, def) {
                format!("{kind} `{}` is written but never read", def.name)
            } else {
                format!("{kind} `{}` is never read", def.name)
            };
            cx.report_in(file, def_range(view, def), message)
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
    written: bool,
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
    let message = format!("Remove unused custom property '{}'", def.name);
    let edits = cx.file_edits(file, edits);
    // A write recreates the property at run time, without the `access`
    // and `persistent` settings of the removed `propConfig` entry.
    Some(if written {
        Fix::unsafe_edit(message, edits)
    } else {
        Fix::safe(message, edits)
    })
}

/// `accesses` with the assumptions made for library calls
/// ([`PropertyAccess::via`]) replaced by what the called functions do.
fn follow_library_calls<'a>(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    accesses: &'a [PropertyAccess],
) -> std::borrow::Cow<'a, [PropertyAccess]> {
    if accesses.iter().all(|a| a.via.is_none()) {
        return std::borrow::Cow::Borrowed(accesses);
    }
    let Some((index, project)) = cx.project_index() else {
        return std::borrow::Cow::Borrowed(accesses);
    };
    let resolve = |callee: &str, context: Option<CalleeContext>| {
        resolve_callee(index, project, callee, context)
    };
    let mut out: Vec<PropertyAccess> = Vec::with_capacity(accesses.len());
    let mut followed: Vec<&std::sync::Arc<LibraryCall>> = Vec::new();
    for a in accesses {
        match &a.via {
            None => out.push(a.clone()),
            Some(call) => {
                // The assumed accesses of one call share it.
                if followed.iter().any(|f| std::sync::Arc::ptr_eq(f, call)) {
                    continue;
                }
                followed.push(call);
                out.extend(follow_call(view, call, a.range, &resolve));
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Resolves a name called with a value of a view: written in the view
/// script (`context` `None`) or in the body of a library function.
fn resolve_callee<'i>(
    index: &'i ProjectIndex<'_>,
    project: ProjectId,
    callee: &str,
    context: Option<CalleeContext>,
) -> Callee<'i> {
    let (head, rest) = match callee.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (callee, None),
    };
    let library = index.library(project);
    let dotted = match context {
        None if library.is_global(head) => callee.to_owned(),
        None => return Callee::Unknown,
        Some(context) => {
            let Some(symbols) = index.module_symbols(context.module) else {
                return Callee::Unknown;
            };
            let module = index
                .workspace()
                .resource(context.module)
                .path
                .replace('/', ".");
            match symbols.get(head).map(|s| &s.kind) {
                Some(SymbolKind::Function(_)) => format!("{module}.{callee}"),
                // Another resource may rebind the module's name of the
                // import (`app.views.fmt = f`).
                Some(SymbolKind::Import { .. })
                    if std::iter::once(project)
                        .chain(index.workspace().project_descendants(project))
                        .any(|p| index.library_name_rebound(p, &format!("{module}.{head}"))) =>
                {
                    return Callee::Unknown;
                }
                Some(SymbolKind::Import {
                    target: Some(target),
                }) => match rest {
                    Some(rest) => format!("{target}.{rest}"),
                    None => target.clone(),
                },
                Some(_) => return Callee::Unknown,
                None if symbols.dynamic => return Callee::Unknown,
                None if library.is_global(head) => callee.to_owned(),
                None => return Callee::Unknown,
            }
        }
    };
    // A name that may be rebound, or created, at run time says nothing
    // about what the call does.
    if std::iter::once(project)
        .chain(index.workspace().project_descendants(project))
        .any(|p| index.library_name_rebound(p, &dotted))
    {
        return Callee::Unknown;
    }
    let segments: Vec<&str> = dotted.split('.').collect();
    match resolve(index, project, &dotted) {
        LibraryLookup::Missing { .. } | LibraryLookup::HiddenByModule { .. } => Callee::Missing,
        LibraryLookup::MissingAttribute { index: i, .. } => {
            let name = segments.get(i).copied().unwrap_or("");
            if (name.starts_with("__") && name.ends_with("__"))
                || IMPLICIT_GLOBALS.contains(&name)
                || library.is_global(name)
            {
                Callee::Unknown
            } else {
                Callee::Missing
            }
        }
        LibraryLookup::Symbol {
            resource,
            symbol,
            rest: 0,
        } => {
            let SymbolKind::Function(signature) = &symbol.kind else {
                return Callee::Unknown;
            };
            // A child project that inherits the view may override the
            // module with another function.
            let same_everywhere = index
                .workspace()
                .project_descendants(project)
                .into_iter()
                .all(|d| {
                    matches!(
                        index.resolve_library(d, &dotted),
                        LibraryLookup::Symbol { resource: r, rest: 0, .. } if r == resource
                    )
                });
            if same_everywhere {
                Callee::Function(signature, CalleeContext { module: resource })
            } else {
                Callee::Unknown
            }
        }
        _ => Callee::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use crate::Diagnostic;
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
    fn references_in_other_settings() {
        let action = view(
            r#"{"url": 1}"#,
            "{}",
            &[label(
                "A",
                r#", "events": {"dom": {"onClick": {"type": "nav", "scope": "C", "config": {"url": "{view.custom.url}", "newTab": true}}}}"#,
            )],
            "{}",
        );
        assert!(unused(&action).is_empty());
    }

    #[test]
    fn references_in_other_binding_settings() {
        let v = view(
            r#"{"range": 1}"#,
            "{}",
            &[label(
                "A",
                r#", "propConfig": {"props.data": {"binding": {"type": "tag-history", "config": {"dateRange": {"mostRecent": "{view.custom.range}", "mostRecentUnits": "HOUR"}, "tags": []}}}}"#,
            )],
            "{}",
        );
        assert!(unused(&v).is_empty());
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
    fn event(script: &str) -> String {
        format!(
            r#", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}"#
        )
    }

    /// Unused properties of a view whose button runs `script`, in a project
    /// whose library has `modules` (`(path, code)`).
    fn with_library(script: &str, modules: &[(&str, &str)]) -> Vec<Diagnostic> {
        let v = view(
            r#"{"a": 1, "b": 2, "c": 3}"#,
            "{}",
            &[
                label("A", r#", "custom": {"x": 1, "y": 2}"#),
                label("Btn", &event(script)),
            ],
            "{}",
        );
        let mut f = Fixture::view(&v);
        for (path, code) in modules {
            f = f.resource(
                &format!("projects/p/ignition/script-python/{path}"),
                &[("code.py", code)],
            );
        }
        f.lint(&["VIEW015"])
    }

    fn paths(d: &[Diagnostic]) -> Vec<String> {
        d.iter().map(|d| d.node_path.clone().unwrap()).collect()
    }

    const ALL: [&str; 5] = [
        "view.custom.a",
        "view.custom.b",
        "view.custom.c",
        "root/A.custom.x",
        "root/A.custom.y",
    ];

    #[test]
    fn library_functions_that_do_not_exist_read_nothing() {
        // `util` has no `missing`: the call fails before anything is read.
        let d = with_library(
            "\\tlib.util.missing(self=self)",
            &[("lib/util", "def other(a):\n    return a\n")],
        );
        assert_eq!(paths(&d), ALL);
        let d = with_library("\\tlib.nope.handle(self)", &[("lib/util", "")]);
        assert_eq!(paths(&d), ALL);
    }

    #[test]
    fn library_functions_are_followed() {
        let util = "def handle(self, x=1):\n    if self is None:\n        return\n    v = self.view\n    print v.custom.a, self.getSibling('A').custom['x']\n";
        let d = with_library("\\tlib.util.handle(self)", &[("lib/util", util)]);
        assert_eq!(
            paths(&d),
            ["view.custom.b", "view.custom.c", "root/A.custom.y"]
        );
        // Keyword arguments, containers, calls passed on within the module
        // and to other modules.
        let util = "import lib.other as o\ndef handle(c, comp=None):\n    print c.b\n    inner(comp)\n    o.go(comp)\ndef inner(q):\n    return q.custom.x\n";
        let other = "def go(p):\n    p.custom.y += 1\n";
        let d = with_library(
            "\\tlib.util.handle(comp=self.getSibling('A'), c=self.view.custom)",
            &[("lib/util", util), ("lib/other", other)],
        );
        assert_eq!(paths(&d), ["view.custom.a", "view.custom.c"]);
    }

    #[test]
    fn library_uses_that_cannot_be_followed_read_everything() {
        for util in [
            // Dynamic access, the parameter returned, rebound, used in a
            // nested function, passed to something that is not a library
            // function, `*args`.
            "def handle(p):\n    return getattr(p, 'view')\n",
            "def handle(p):\n    return p\n",
            "def handle(p):\n    p = p.parent\n    print p.custom.x\n",
            "def handle(p):\n    def f():\n        return p.view.custom.a\n    return f()\n",
            "def handle(p):\n    system.perspective.print(p)\n",
            "def handle(*args):\n    pass\n",
            "def handle(p):\n    exec 'print 1'\n",
            "handle = lambda p: None\n",
        ] {
            let d = with_library("\\tlib.util.handle(self)", &[("lib/util", util)]);
            assert!(d.is_empty(), "{util}: {:?}", paths(&d));
        }
        // Calls more than 4 deep.
        let util = "def f1(p):\n    f2(p)\ndef f2(p):\n    f3(p)\ndef f3(p):\n    f4(p)\ndef f4(p):\n    f5(p)\ndef f5(p):\n    pass\n";
        let d = with_library("\\tlib.util.f1(self)", &[("lib/util", util)]);
        assert!(d.is_empty(), "{:?}", paths(&d));
        // A name the script binds is not the library.
        let d = with_library(
            "\\tlib = other\\n\\tlib.util.missing(self)",
            &[("lib/util", "")],
        );
        assert!(d.is_empty(), "{:?}", paths(&d));
    }

    #[test]
    fn library_functions_a_child_project_overrides_read_everything() {
        let v = view(
            r#"{"a": 1}"#,
            "{}",
            &[label("Btn", &event("\\tlib.util.handle(self)"))],
            "{}",
        );
        let f = Fixture::view(&v)
            .file(
                "projects/p/project.json",
                r#"{"title": "p", "enabled": true, "inheritable": true}"#,
            )
            .resource(
                "projects/p/ignition/script-python/lib/util",
                &[("code.py", "def handle(p):\n    pass\n")],
            )
            .file(
                "projects/child/project.json",
                r#"{"title": "child", "enabled": true, "parent": "p"}"#,
            );
        // Without an override the child runs the same function.
        assert_eq!(paths(&f.lint(&["VIEW015"])), ["view.custom.a"]);
        let f = f.resource(
            "projects/child/ignition/script-python/lib/util",
            &[("code.py", "def handle(p):\n    print p.view.custom.a\n")],
        );
        assert!(f.lint(&["VIEW015"]).is_empty());
    }

    #[test]
    fn fix_is_unsafe_for_written_properties() {
        let v = view(
            r#"{"a": 1, "b": 2}"#,
            "{}",
            &[label("Btn", &event("\\tself.view.custom.a = 2"))],
            r#"{"custom.a": {"access": "PRIVATE"}}"#,
        );
        let d = Fixture::view(&v).lint(&["VIEW015"]);
        let fixes: Vec<(String, crate::Applicability)> = d
            .iter()
            .map(|d| {
                (
                    d.node_path.clone().unwrap(),
                    d.fix.as_ref().unwrap().applicability,
                )
            })
            .collect();
        assert_eq!(
            fixes,
            [
                ("view.custom.a".to_owned(), crate::Applicability::Unsafe),
                ("view.custom.b".to_owned(), crate::Applicability::Safe),
            ]
        );
        // Written by a library function.
        let d = with_library(
            "\\tlib.util.handle(self)",
            &[("lib/util", "def handle(p):\n    p.view.custom.c = 1\n")],
        );
        let c = d
            .iter()
            .find(|d| d.node_path.as_deref() == Some("view.custom.c"))
            .unwrap();
        assert_eq!(c.message, "Custom property `c` is written but never read");
        assert_eq!(
            c.fix.as_ref().unwrap().applicability,
            crate::Applicability::Unsafe
        );
    }
}
