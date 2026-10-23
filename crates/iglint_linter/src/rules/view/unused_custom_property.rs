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
use std::borrow::Cow;
use std::sync::Arc;

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
  parser. The analysis is default-deny: a component, the view, a page or
  session, a property container (`self.custom`, `self.view.params`) or a
  local alias of one must be used in one of the enumerated forms
  (`self.view.custom.x`, `self.custom['x']`, `self.custom.get('x')`,
  `'x' in self.custom`, `self.getSibling('A').custom.x`,
  `self.parent.getChild('A').custom.x`, `view = self.view`, an argument
  of a library function, a custom method of the view, ...); any other
  use (passed to unknown code, iterated, stored, compared, formatted,
  `self.custom.keys` used without a call, `self.custom[key]`, a script
  that does not parse) counts as reading and writing every property it
  could reach, so the rule stays silent rather than guess. The list of
  forms is in `docs/internal/fix-safety.md`;
* builtins that read the caller's variables by name: a script that calls
  `locals()`, `vars()`, `dir()`, `eval` or `execfile`, runs `exec`,
  reaches frames (`sys._getframe()`, `inspect.currentframe()`,
  `.f_locals`) or reaches such a builtin in any other way
  (`__builtin__.eval`, `from __builtin__ import locals as l`, `e = eval`)
  reads everything. Every library name a script calls or uses as a value
  must be a plain `def` function (or class, or literal constant), or not
  exist: a library variable or import may hold `eval` under another name
  (`ev = eval` in a module, `lib.util.ev('self.custom.x')` reads the
  caller's `self`);
* project library functions a script passes `self`, the view or a
  property container to (`lib.util.handle(self)`, `handle(view=self.view)`):
  the function's body is followed with the same default-deny rules
  (`param.view.custom.x`, `x = param.custom.y` even when `x` is never
  used, `param.getSibling('A').custom.y`, passing it on to other library
  functions, up to 4 calls deep). A function that does not exist reads
  nothing (the call fails, which `REF002` reports). A function that uses
  the value in a way that cannot be followed (it stores the value where
  it outlives the call, in a `global`, a module or class attribute, a
  default argument or a container it does not own, returns, yields or
  raises it, captures it in a nested function, or reaches stack frames
  with `sys._getframe()` or `inspect`), a function a child project
  overrides, a function that may be replaced at run time (its module runs
  `exec`, `eval` or `execfile`, writes `globals()` or
  `sys.modules[__name__]`, sets its `func_code`, or may overwrite it with
  a later definition or `from x import *`; a script assigns or `setattr`s
  it; its module is used as a value, imported with
  `importlib.import_module`, reached through `sys.modules` or code run
  from strings; a function or class of its module is used as a value
  anywhere (`alias = handle`, `register(handle)`) or its `func_globals`
  is reached), and calls of anything but a library function count as
  reading everything.

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
entries. Safe when nothing writes the property and the view's own
scripts prove it unread, without following values into library
functions. Unsafe when a script or a bidirectional binding writes it (the
write recreates the property, without the `access` and `persistent`
settings of its `propConfig`), when the finding relies on what a library
function does with a value of the view (code in other resources, which
other projects or run-time changes can invalidate), and for a view linted
outside its project whose scripts call names that may be library
functions. View parameters are never removed automatically (callers pass
them).

Like every fix, it is unsafe while code in the workspace may read the
files a fix edits (a read whose path is not a literal outside the
workspace, or code that is not vetted); the project setting `[fix]
assume-scripts-do-not-read-workspace-files = true` declares that no script
does (see `docs/internal/fix-safety.md`, "File reads").

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
        let (followed, local) = follow_library_calls(cx, view, accesses);
        // Callee checks that could not be resolved: names the scripts call
        // may be anything, so no fix is safe.
        let unchecked =
            cx.project_index().is_none() && accesses.iter().any(PropertyAccess::is_callee_check);
        // What the view's scripts prove on their own, without following
        // values into library functions.
        let local = usage(view, &local);
        let usage = usage(view, &followed);
        // Code that is not vetted may read the property in ways iglint
        // does not know (`cPickle.loads` of an `eval` opcode).
        let mut unknown_code = None;
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
            // The fix deletes data: it is safe only when nothing writes the
            // property and the view proves it unused without following
            // values into library functions (code outside the view, which
            // other resources or run-time changes can invalidate).
            // Code that is not vetted in the view's scripts, or (closed
            // world) anywhere in the shared runtime, may read this property
            // by name.
            let vetting = unknown_code
                .get_or_insert_with(|| crate::rules::vetting::unit_reason(cx))
                .clone();
            let interprocedural =
                unchecked || local.is_read(i, def) || local.is_written(i, def) || vetting.is_some();
            let fix = match def.category {
                PropCategory::Custom => fix(
                    cx,
                    view,
                    file,
                    def,
                    usage.is_written(i, def) || interprocedural,
                )
                .map(|f| f.because(vetting)),
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
    unsafe_fix: bool,
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
    Some(if unsafe_fix {
        Fix::unsafe_edit(message, edits)
    } else {
        Fix::safe(message, edits)
    })
}

/// Whether `body` calls or uses a library name that is not a plain
/// function, class or literal constant: it may hold `eval` or `locals`,
/// checked through the project index; without the index, any such name
/// counts. `model`
/// resolves the names `body` uses through their scope.
pub(crate) fn library_names_may_read_frames(
    cx: &Checker<'_, '_>,
    model: &iglint_python_semantic::SemanticModel<'_>,
    body: &[iglint_python::ast::Stmt],
) -> bool {
    let names = crate::script_facts::access::library_names_used(model, body);
    if names.is_empty() {
        return false;
    }
    let Some((index, project)) = cx.project_index() else {
        return true;
    };
    names.iter().any(|n| {
        matches!(
            resolve_callee(index, project, n, None, true, 0),
            Callee::Unknown
        )
    })
}

/// `accesses` with the callee checks ([`PropertyAccess::is_callee_check`])
/// resolved through the project index, for rules that do not follow
/// library calls: a name that is not a plain function, class or literal
/// constant may be a frame builtin and accesses everything. Without the
/// index (a view linted outside its project) the checks stay: every such
/// name may be one.
pub(crate) fn resolve_callee_checks<'a>(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    accesses: &'a [PropertyAccess],
) -> Cow<'a, [PropertyAccess]> {
    if !accesses.iter().any(PropertyAccess::is_callee_check) {
        return Cow::Borrowed(accesses);
    }
    let Some((index, project)) = cx.project_index() else {
        return Cow::Borrowed(accesses);
    };
    let resolve = |callee: &str, context: Option<CalleeContext>, check: bool| {
        resolve_callee(index, project, callee, context, check, 0)
    };
    let mut out = Vec::with_capacity(accesses.len());
    let mut done: Vec<&Arc<LibraryCall>> = Vec::new();
    for a in accesses {
        match &a.via {
            Some(call) if call.is_callee_check() => {
                if done.iter().any(|f| Arc::ptr_eq(f, call)) {
                    continue;
                }
                done.push(call);
                out.extend(follow_call(view, call, a.range, &resolve));
            }
            _ => out.push(a.clone()),
        }
    }
    Cow::Owned(out)
}

/// `accesses` with the assumptions made for library calls
/// ([`PropertyAccess::via`]) replaced by what the called functions do, and
/// `accesses` with only the callee checks ([`LibraryCall::is_callee_check`])
/// resolved: what the view's own scripts prove without following values
/// into library functions.
fn follow_library_calls<'a>(
    cx: &Checker<'_, '_>,
    view: &View<'_>,
    accesses: &'a [PropertyAccess],
) -> (Cow<'a, [PropertyAccess]>, Cow<'a, [PropertyAccess]>) {
    if accesses.iter().all(|a| a.via.is_none()) {
        return (Cow::Borrowed(accesses), Cow::Borrowed(accesses));
    }
    let Some((index, project)) = cx.project_index() else {
        // Outside a project the library is unknown. Callee checks are
        // dropped so that findings are still reported; the caller makes
        // their fixes unsafe.
        let known: Vec<PropertyAccess> = accesses
            .iter()
            .filter(|a| !a.is_callee_check())
            .cloned()
            .collect();
        return (Cow::Owned(known.clone()), Cow::Owned(known));
    };
    let resolve = |callee: &str, context: Option<CalleeContext>, check: bool| {
        resolve_callee(index, project, callee, context, check, 0)
    };
    let mut followed: Vec<PropertyAccess> = Vec::with_capacity(accesses.len());
    let mut local: Vec<PropertyAccess> = Vec::with_capacity(accesses.len());
    let mut done: Vec<&Arc<LibraryCall>> = Vec::new();
    for a in accesses {
        match &a.via {
            None => {
                followed.push(a.clone());
                local.push(a.clone());
            }
            Some(call) => {
                if !call.is_callee_check() {
                    local.push(a.clone());
                }
                // The assumed accesses of one call share it.
                if done.iter().any(|f| Arc::ptr_eq(f, call)) {
                    continue;
                }
                done.push(call);
                let result = follow_call(view, call, a.range, &resolve);
                if call.is_callee_check() {
                    local.extend(result.iter().cloned());
                }
                followed.extend(result);
            }
        }
    }
    (Cow::Owned(followed), Cow::Owned(local))
}

/// Resolves a name called with a value of a view, or (`check`) called or
/// used as a value while values of the view are live: written in the view
/// script (`context` `None`) or in the body of a library function.
pub(crate) fn resolve_callee<'i>(
    index: &'i ProjectIndex<'_>,
    project: ProjectId,
    callee: &str,
    context: Option<CalleeContext>,
    check: bool,
    depth: usize,
) -> Callee<'i> {
    // Not followed, but certainly not a frame builtin.
    let opaque = if check {
        Callee::Opaque
    } else {
        Callee::Unknown
    };
    if depth > 8 || callee.starts_with('.') {
        return Callee::Unknown;
    }
    let (head, rest) = match callee.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (callee, None),
    };
    let library = index.library(project);
    let dotted = match context {
        None if library.is_global(head) => callee.to_owned(),
        // A builtin, an Ignition global or an import of something that is
        // not a library module (`json.dumps`).
        None => return opaque,
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
                Some(SymbolKind::Class) => return opaque,
                // A literal or one of its methods (`NAMES.index(x)`).
                Some(SymbolKind::Variable)
                    if check
                        && symbols.constants.iter().any(|c| c == head)
                        && !index.library_name_rebound(project, &format!("{module}.{head}")) =>
                {
                    return opaque;
                }
                Some(_) => return Callee::Unknown,
                None if symbols.dynamic => return Callee::Unknown,
                None if library.is_global(head) => callee.to_owned(),
                // A builtin or an Ignition global.
                None => return opaque,
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
            rest,
        } => {
            // A child project that inherits the view may override the
            // module with another definition.
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
            if !same_everywhere {
                return Callee::Unknown;
            }
            let symbols = index.module_symbols(resource);
            match (&symbol.kind, rest) {
                (SymbolKind::Function(signature), 0) => {
                    Callee::Function(signature, CalleeContext { module: resource })
                }
                (SymbolKind::Class, _) => opaque,
                (SymbolKind::Variable, _)
                    if check && symbols.is_some_and(|s| s.constants.contains(&symbol.name)) =>
                {
                    opaque
                }
                (
                    SymbolKind::Import {
                        target: Some(target),
                    },
                    _,
                ) if check => {
                    // Re-exported: what the import names.
                    let tail = segments[segments.len() - rest..].join(".");
                    let target = if tail.is_empty() {
                        target.clone()
                    } else {
                        format!("{target}.{tail}")
                    };
                    resolve_callee(index, project, &target, None, check, depth + 1)
                }
                _ => Callee::Unknown,
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

    /// Every way a followed parameter can leave the function: the value
    /// outlives the call (globals, module and class attributes, default
    /// arguments, containers reachable from outside), reaches unknown code
    /// (returned, yielded, raised, passed on, captured by a closure) or is
    /// read through the frame. Each counts as reading everything.
    #[test]
    fn library_values_that_escape_read_everything() {
        for util in [
            // Globals, also through aliases and other kinds of stores.
            "_c = None\ndef handle(p):\n    global _c\n    _c = p\ndef later():\n    return _c.view.custom.a\n",
            "def handle(p):\n    global _c\n    _c = p.view\n",
            "def handle(p):\n    global _c\n    v = p\n    _c = v\n",
            "def handle(p):\n    global _c\n    _c, _d = p, 1\n",
            "def handle(p):\n    global _c\n    for _c in [p]:\n        pass\n",
            "def handle(p):\n    global _c\n    [0 for _c in (p,)]\n",
            "_l = []\ndef handle(p):\n    global _l\n    _l += [p]\n",
            "def handle(p):\n    def g():\n        global _c\n        _c = p\n    g()\n",
            "def handle(p):\n    globals()['_c'] = p\n",
            // Attributes and items of objects that outlive the call.
            "import lib.util as m\ndef handle(p):\n    m.saved = p\n",
            "def handle(p):\n    lib.util.saved = p\n",
            "import sys\ndef handle(p):\n    setattr(sys.modules[__name__], '_c', p)\n",
            "class K:\n    pass\ndef handle(p):\n    K.saved = p\n",
            "_s = {}\ndef handle(p):\n    _s['k'] = p\n",
            "_s = []\ndef handle(p):\n    _s.append(p)\n",
            "_s = []\ndef handle(p):\n    l = [p]\n    _s.extend(l)\n",
            "_s = {}\ndef handle(p):\n    _s.update({'k': p})\n",
            "def handle(p, cache={}):\n    cache['k'] = p\n",
            "def handle(p, cache=[]):\n    cache.append(p.view)\n",
            "def handle(p, **lib):\n    lib.update(k=p)\n",
            // Unknown code: returned, yielded, raised, passed on, captured.
            "def handle(p):\n    v = p.view\n    return v\n",
            "def handle(p):\n    yield p\n",
            "def handle(p):\n    raise ValueError(p)\n",
            "def handle(p):\n    system.util.invokeLater(lambda: p.view.custom.a)\n",
            "def handle(p):\n    return (p for _ in [1])\n",
            "def handle(p):\n    v = p.view\n    return lambda: v\n",
            "def handle(p):\n    class C:\n        q = p\n    return C\n",
            // Frames.
            "import sys\ndef handle(p):\n    return sys._getframe(0).f_locals\n",
            "import inspect\ndef handle(p):\n    return inspect.currentframe()\n",
        ] {
            let d = with_library("\\tlib.util.handle(self)", &[("lib/util", util)]);
            assert!(d.is_empty(), "{util}: {:?}", paths(&d));
        }
        // A local that is not global is followed.
        let util = "_c = None\ndef handle(p):\n    _c = p\n    print _c.view.custom.a\n";
        let d = with_library("\\tlib.util.handle(self)", &[("lib/util", util)]);
        assert_eq!(
            paths(&d),
            [
                "view.custom.b",
                "view.custom.c",
                "root/A.custom.x",
                "root/A.custom.y"
            ]
        );
        // The same for the view script itself.
        for script in [
            "\\tglobal v\\n\\tv = self.view",
            "\\timport sys\\n\\tsys._getframe(0)",
        ] {
            let d = with_library(script, &[]);
            assert!(d.is_empty(), "{script}: {:?}", paths(&d));
        }
    }

    /// The library walker on every Python 2.7 context of
    /// [`CONTEXTS`](crate::script_facts::access::tests::CONTEXTS): a
    /// container (`p.view.custom`) or a component (`p.getSibling('A')`)
    /// derived from the parameter is used in an understood way or reads
    /// everything it reaches.
    #[test]
    fn library_contexts_are_understood_or_read_everything() {
        use crate::script_facts::access::tests::{
            CONTEXTS, understood_component, understood_container,
        };
        for context in CONTEXTS {
            let mut parsed_once = false;
            for v in ["p.view.custom", "p.getSibling('A')"] {
                let body = context
                    .replace("{v}", v)
                    .replace("{{", "{")
                    .replace("}}", "}");
                let util = format!("def handle(p):\n\t{body}\n");
                if !iglint_python::parse_module(&util).errors.is_empty() {
                    continue;
                }
                parsed_once = true;
                let diagnostics = with_library("\\tlib.util.handle(self)", &[("lib/util", &util)]);
                let found = paths(&diagnostics);
                let container = v == "p.view.custom";
                let understood = if container {
                    understood_container(context)
                } else {
                    understood_component(context)
                };
                if understood.is_none() {
                    // Everything the value reaches counts as read (more is
                    // allowed: the whole view).
                    // A whole container written (`c[k] = 1`) leaves its
                    // properties written but never read.
                    assert!(
                        diagnostics.iter().all(|d| container
                            && (!d.node_path.as_deref().unwrap().starts_with("view.custom.")
                                || d.message.contains("written"))),
                        "{util}: {found:?}"
                    );
                    continue;
                }
                let expected: Vec<String> = match understood {
                    // The view script's `B.custom.N` uses are the
                    // library's `view.custom.N`.
                    Some(uses) => {
                        let read_all = uses.iter().any(|u| u.starts_with("B.custom.* r"));
                        let read: Vec<&str> = uses
                            .iter()
                            .filter(|u| u.contains(" r"))
                            .filter_map(|u| u.strip_prefix("B.custom."))
                            .map(|u| u.split(' ').next().unwrap())
                            .collect();
                        ALL.iter()
                            .filter(|p| {
                                !(container
                                    && p.starts_with("view.custom.")
                                    && (read_all || read.contains(&&p["view.custom.".len()..])))
                            })
                            .map(|p| (*p).to_owned())
                            .collect()
                    }
                    None => unreachable!(),
                };
                assert_eq!(found, expected, "{util}");
            }
            assert!(parsed_once, "{context}");
        }
    }

    /// The dynamic patterns of the safe-fix audit (`fix-safety.md`): each
    /// reads everything, so nothing is reported.
    #[test]
    fn dynamic_patterns_read_everything() {
        let util = "ev = eval\ndef handle(p):\n    pass\ndef _real(p):\n    print p.view.custom.a\nhandle.func_code = _real.func_code\n";
        for script in [
            "\\tlib.util.ev('self.view.custom.a')",
            "\\timport __builtin__\\n\\t__builtin__.eval(s)",
            "\\tlib.util.handle(self)",
            "\\tgetattr(self.view.custom, n)",
            "\\tgetattr(self, n)",
            "\\texec s",
            "\\tf = getattr(lib.util, n)\\n\\tf(s)",
            "\\tlib.util.HANDLERS['x'](s)",
            "\\tfactory()(s)",
            // A nested function's parameter must not hide the outer
            // `lib.util.ev` reference (reviewer repro R5-G1).
            "\\tdef inner(lib):\\n\\t\\tpass\\n\\tlib.util.ev('self.view.custom.a')",
        ] {
            let d = with_library(script, &[("lib/util", util)]);
            assert!(
                paths(&d).iter().all(|p| !p.starts_with("view.custom.")),
                "{script}: {:?}",
                paths(&d)
            );
        }
    }

    /// A library function that reaches the caller's frame (any route of
    /// the threat model) can read `self` and any property through it, even
    /// when the script hands it no component. It is not "plain", so VIEW015
    /// credits every property and reports nothing. (Reviewer repro R5-C.)
    #[test]
    fn frame_reading_library_function_reads_everything() {
        for logger in [
            // The exact reviewer repro: exc_info traceback, then the frame
            // and locals reached through getattr string literals.
            "import sys\ndef log(m):\n    try:\n        raise ValueError(m)\n    except ValueError:\n        tb = sys.exc_info()[2]\n    fr = getattr(getattr(tb, 'tb_frame'), 'f_back')\n    me = getattr(fr, 'f_locals')['self']\n    system.perspective.print(getattr(me.view.custom, 'b'))\n",
            // getattr with a computed name (route 2).
            "import sys\ndef log(m):\n    tb = sys.exc_info()[2]\n    return getattr(tb, 'tb_' + 'frame')\n",
            // operator.attrgetter with a dotted string (route 1).
            "import operator, sys\ndef log(m):\n    tb = sys.exc_info()[2]\n    return operator.attrgetter('tb_frame.f_back.f_back.f_locals')(tb)\n",
            // __getattribute__ with an introspection string (route 1).
            "import sys\ndef log(m):\n    tb = sys.exc_info()[2]\n    return tb.__getattribute__('tb_frame')\n",
            // org.python reflection import (route 3).
            "from org.python.core import Py\ndef log(m):\n    return getattr(Py.getFrame(), 'f_back')\n",
        ] {
            let d = with_library(
                "\\tlib.util.log('x')\\n\\tprint self.view.custom.a",
                &[("lib/util", logger)],
            );
            assert!(d.is_empty(), "logger {logger:?} gave {:?}", paths(&d));
        }
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

    /// Findings that rely on following values into library functions keep
    /// being reported, with unsafe fixes; findings the view proves on its
    /// own keep safe fixes, also when its scripts call library functions.
    #[test]
    fn fixes_relying_on_library_functions_are_unsafe() {
        let applicability = |d: &[Diagnostic]| -> Vec<(String, crate::Applicability)> {
            d.iter()
                .map(|d| {
                    (
                        d.node_path.clone().unwrap(),
                        d.fix.as_ref().unwrap().applicability,
                    )
                })
                .collect()
        };
        // `self` handed to a function that reads `a`: the others are
        // unused only because the function was followed.
        let d = with_library(
            "\\tlib.util.handle(self)",
            &[("lib/util", "def handle(p):\n    print p.view.custom.a\n")],
        );
        assert!(
            applicability(&d)
                .iter()
                .all(|(_, a)| *a == crate::Applicability::Unsafe),
            "{:?}",
            applicability(&d)
        );
        assert_eq!(paths(&d).len(), 4);
        // Only a container handed over: the component's properties are
        // proven unused by the view alone.
        let d = with_library(
            "\\tlib.util.handle(self.view.custom)",
            &[("lib/util", "def handle(c):\n    print c.a\n")],
        );
        assert_eq!(
            applicability(&d),
            [
                ("view.custom.b".to_owned(), crate::Applicability::Unsafe),
                ("view.custom.c".to_owned(), crate::Applicability::Unsafe),
                ("root/A.custom.x".to_owned(), crate::Applicability::Safe),
                ("root/A.custom.y".to_owned(), crate::Applicability::Safe),
            ]
        );
        // Library calls without values of the view: checked callees only.
        let d = with_library(
            "\\tlib.util.log('x')\\n\\tx = self.view.custom.a",
            &[("lib/util", "def log(m):\n    print m\nLIMIT = 3\n")],
        );
        assert!(
            applicability(&d)
                .iter()
                .all(|(_, a)| *a == crate::Applicability::Safe),
            "{:?}",
            applicability(&d)
        );
        assert_eq!(paths(&d).len(), 4);
    }

    /// Code that is not vetted keeps every finding of the view, with
    /// unsafe fixes: `cPickle` runs `eval` in the caller's frame
    /// (reviewer repro R6-B), a library module the script calls imports
    /// Reviewer round 7, finding 1: a view event that reaches Java
    /// reflection (through a bean property, a `getClass`/`getClassLoader`
    /// call, or a reflective member named in a string) must never let a
    /// custom property be removed by a safe fix: the reflection route can
    /// read any property. No finding may carry a safe fix.
    #[test]
    fn java_reflection_blocks_view015_safe_fix() {
        for script in [
            "\\tr = self.getClass().getClassLoader()",
            "\\tr = self.view.custom.class",
            "\\tr = getattr(self, 'declaredMethods')",
            "\\tfrom java.util import ArrayList\\n\\tclass B(ArrayList):\\n\\t\\tpass\\n\\tr = B().class",
        ] {
            let d = with_library(script, &[]);
            assert!(
                d.iter().all(|d| d
                    .fix
                    .as_ref()
                    .is_none_or(|f| f.applicability == crate::Applicability::Unsafe)),
                "a safe fix survived reflection: {script}\n{d:?}"
            );
        }
    }

    /// something unvetted.
    #[test]
    fn unvetted_code_makes_fixes_unsafe() {
        let all_unsafe = |d: &[Diagnostic]| {
            !d.is_empty()
                && d.iter()
                    .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Unsafe)
        };
        let d = with_library(
            "\\timport cPickle\\n\\tsystem.perspective.print(cPickle.loads(\\\"c__builtin__\\\\neval\\\\n(S'self.view.custom.a'\\\\ntR.\\\"))",
            &[],
        );
        assert!(all_unsafe(&d), "{d:?}");
        let d = with_library(
            "\\tlib.util.log('x')",
            &[("lib/util", "import warnings\ndef log(m):\n    print m\n")],
        );
        assert!(all_unsafe(&d), "{d:?}");
        // Vetted modules keep the safe fix.
        let d = with_library(
            "\\timport json\\n\\tlib.util.log(json.dumps(1))",
            &[(
                "lib/util",
                "import math\ndef log(m):\n    print math.sqrt(m)\n",
            )],
        );
        assert!(
            d.iter()
                .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Safe),
            "{d:?}"
        );
    }

    /// Reviewer round 10 (Codex) blocker: a view event that reads its own
    /// `view.json` sees the removed property. Reads of possibly-workspace
    /// files make every fix unsafe, with the site named; a read of a
    /// literal path outside the workspace does not.
    #[test]
    fn file_reads_make_fixes_unsafe() {
        let d = with_library(
            "\\timport json\\n\\td = json.loads(open('/usr/local/bin/ignition/data/projects/p/com.inductiveautomation.perspective/views/Main/view.json').read())\\n\\tsystem.perspective.print('a' in d['custom'])",
            &[],
        );
        assert!(!d.is_empty());
        for d in &d {
            let fix = d.fix.as_ref().unwrap();
            assert_eq!(fix.applicability, crate::Applicability::Unsafe, "{d:?}");
            let site = fix.unsafe_reason.as_ref().unwrap().site.as_ref().unwrap();
            assert!(site.what.starts_with("open("), "{site:?}");
            assert!(site.path.ends_with("view.json"), "{site:?}");
        }
        let d = with_library(
            "\\tlib.util.dump(1)",
            &[(
                "lib/util",
                "def dump(x):\n    print open('/tmp/x.json').read()\n",
            )],
        );
        assert!(
            !d.is_empty()
                && d.iter()
                    .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Safe),
            "{d:?}"
        );
    }

    /// Reviewer round 8 (Codex) blocker: a view script that defines
    /// `capture()` before `import re` reaches `eval` through `re.sys`; the
    /// vetting verdict must not depend on that order.
    #[test]
    fn functions_before_imports_are_vetted() {
        for script in [
            "\\tdef capture():\\n\\t\\treturn re.sys.modules['__builtin__']\\n\\timport re\\n\\tcapture().eval('self.view.custom.a')",
            "\\timport re\\n\\tdef capture():\\n\\t\\treturn re.sys.modules['__builtin__']\\n\\tcapture().eval('self.view.custom.a')",
        ] {
            let d = with_library(script, &[]);
            assert!(
                !d.is_empty()
                    && d.iter().all(|d| d
                        .fix
                        .as_ref()
                        .is_none_or(|f| f.applicability == crate::Applicability::Unsafe)),
                "a safe fix survived: {script}\n{d:?}"
            );
            // Each unsafe fix says why: the view script's `re.sys`.
            for d in &d {
                let reason = d.fix.as_ref().unwrap().unsafe_reason.as_ref().unwrap();
                let site = reason.site.as_ref().unwrap();
                assert_eq!(site.what, "re.sys", "{reason:?}");
                assert!(site.path.ends_with("view.json"), "{reason:?}");
            }
        }
        // A library module whose function precedes its import.
        let d = with_library(
            "\\tlib.util.log('x')",
            &[("lib/util", "def log(m):\n    return re.sys\nimport re\n")],
        );
        assert!(
            !d.is_empty()
                && d.iter()
                    .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Unsafe),
            "{d:?}"
        );
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

    /// View `A` with custom `foo` (unused) and `bar` (read by its button),
    /// view `B` whose button runs `script`, plus `extra` resources
    /// (`(dir, file, text)`): VIEW015 on `A`'s `foo`.
    fn cross_view(script: &str, extra: &[(&str, &str, &str)]) -> Diagnostic {
        let a = view(
            r#"{"foo": 1, "bar": 2}"#,
            "{}",
            &[label(
                "Btn",
                &event("\\tsystem.perspective.print(self.view.custom.bar)"),
            )],
            "{}",
        );
        let b = view("{}", "{}", &[label("Btn", &event(script))], "{}");
        let mut f = Fixture::new().with_view("A", &a).with_view("B", &b);
        for (dir, file, text) in extra {
            f = f.resource(dir, &[(file, text)]);
        }
        let d = f.lint(&["VIEW015"]);
        let mut found: Vec<Diagnostic> = d
            .into_iter()
            .filter(|d| d.node_path.as_deref() == Some("view.custom.foo"))
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        found.remove(0)
    }

    fn applicability(d: &Diagnostic) -> crate::Applicability {
        d.fix.as_ref().expect("a fix").applicability
    }

    /// Reviewer round 9 (Opus) blocker: a page or session hands out the
    /// other views of the page (`page.views`, `getViews()`, `page.view`)
    /// and other pages (`session.pages`, `getPages()`, `getPage(id)`), with
    /// their custom properties. Such a use anywhere in the workspace is
    /// unknown code for the closed world: the fix in view `A` is unsafe and
    /// names the site.
    #[test]
    fn other_views_through_pages_and_sessions_make_fixes_unsafe() {
        let unsafe_at = |d: &Diagnostic, what: &str| {
            assert_eq!(applicability(d), crate::Applicability::Unsafe, "{d:?}");
            let reason = d
                .fix
                .as_ref()
                .unwrap()
                .unsafe_reason
                .as_ref()
                .expect("a reason");
            let site = reason.site.as_ref().expect("a site");
            assert!(site.what.contains(what), "{site:?}");
        };
        // View to view.
        for (script, what) in [
            (
                "\\tfor v in self.page.views:\\n\\t\\tif v.id.startswith('A@'):\\n\\t\\t\\tsystem.perspective.print(v.custom.foo)",
                "views",
            ),
            (
                "\\tvs = self.page.getViews()\\n\\tvs[0].custom.foo",
                "getViews",
            ),
            ("\\tself.page.view.custom.foo", "view"),
            ("\\tself.getPage().getView().custom.foo", "getView"),
            ("\\tself.session.getPage('x').views", "getPage"),
            ("\\tfor p in self.session.pages:\\n\\t\\tpass", "pages"),
            ("\\tself.view.session.getPages()", "getPages"),
            ("\\tgetattr(self.page, 'views')", "self.page"),
            ("\\tsystem.util.getGlobals()['p'] = self.page", "self.page"),
            ("\\tsystem.util.getGlobals()['c'] = self", "self"),
            ("\\tself.page.__getattribute__('views')", "__getattribute__"),
        ] {
            unsafe_at(&cross_view(script, &[]), what);
        }
        // A session message handler, a session property transform's
        // session and a page startup script.
        let perspective = "projects/p/com.inductiveautomation.perspective";
        unsafe_at(
            &cross_view(
                "\\tsystem.perspective.print(1)",
                &[(
                    &format!("{perspective}/message/m"),
                    "handleMessage.py",
                    "def handleMessage(session, payload):\n\tfor p in session.getPages():\n\t\tfor v in p.views:\n\t\t\tprint v.custom.foo\n",
                )],
            ),
            "getPages",
        );
        unsafe_at(
            &cross_view(
                "\\tsystem.perspective.print(1)",
                &[(
                    &format!("{perspective}/page-startup"),
                    "onPageStartup.py",
                    "def onPageStartup(page):\n\tprint page.views[0].custom.foo\n",
                )],
            ),
            "views",
        );
        // A library function that receives the page, or a component, and
        // reaches other views with it; the site is the call.
        let library = "projects/p/ignition/script-python/lib/util";
        for (script, code) in [
            (
                "\\tlib.util.dump(self.page)",
                "def dump(p):\n    for v in p.views:\n        print v.custom.foo\n",
            ),
            (
                "\\tlib.util.dump(self)",
                "def dump(c):\n    return c.page.getViews()\n",
            ),
            // Passed on to code that is not followed.
            (
                "\\tlib.util.dump(self)",
                "def dump(c):\n    keep(c)\nkeep = None\n",
            ),
            // Returned to the caller, which may do anything with it.
            (
                "\\tlib.util.dump(self).page.views",
                "def dump(c):\n    return c\n",
            ),
        ] {
            unsafe_at(&cross_view(script, &[(library, "code.py", code)]), "self");
        }
    }

    /// The forms that stay inside the page and session keep the safe fix:
    /// their own properties, names and actions, the view's own tree, and
    /// library functions that only use those.
    #[test]
    fn page_and_session_members_that_stay_local_keep_the_fix() {
        let library = "projects/p/ignition/script-python/lib/util";
        for (script, code) in [
            ("\\tself.page.props.path", ""),
            ("\\tself.session.custom.user = self.page.id", ""),
            ("\\tself.session.props.id\\n\\tself.page.print('x')", ""),
            (
                "\\tif self.page is not None and self.session:\\n\\t\\tpass",
                "",
            ),
            ("\\tself.page.session.custom.x", ""),
            (
                "\\tfor c in self.getChildren():\\n\\t\\tc.props.text = 'x'",
                "",
            ),
            (
                "\\tlib.util.show(self.page)\\n\\tlib.util.show2(self)",
                "def show(p):\n    p.props.path\ndef show2(c):\n    c.view.custom.bar\n",
            ),
        ] {
            let extra: Vec<(&str, &str, &str)> = if code.is_empty() {
                Vec::new()
            } else {
                vec![(library, "code.py", code)]
            };
            let d = cross_view(script, &extra);
            assert_eq!(
                applicability(&d),
                crate::Applicability::Safe,
                "{script}: {d:?}"
            );
        }
    }
}
