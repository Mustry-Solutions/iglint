//! Property accesses of view scripts, found on the Jython syntax tree.
//!
//! # Design: default deny
//!
//! The analysis follows *values*: the objects a script gets from `self`
//! (and a library function from its parameters) that lead to properties
//! of the view. Every use of a followed value must match one of the forms
//! enumerated below; **any other use counts as reading and writing
//! everything the value reaches** (the whole container, or every property
//! of the view and of every component for a component, the view, a page
//! or a session). New syntax, new Ignition API or a construct nobody
//! thought of therefore makes the rule silent instead of wrong.
//!
//! Followed values:
//!
//! | value | reached by |
//! |---|---|
//! | component | `self` in a component script, `getSibling`, `getChild`, `parent`, ... |
//! | view | `self` in a view script, `<component>.view` |
//! | host | `<component or view>.page`, `.session` (and `getPage()`, `getSession()`), the `session` and `page` of session events, `self` in session property scripts: they reach every view of the page or session |
//! | container | `<component or view>.custom`, `<view>.params` |
//!
//! Understood uses of a **component or view** (anything else: everything):
//!
//! | form | meaning |
//! |---|---|
//! | `.custom`, `.params` (view) | the container |
//! | `.view`, `.getView()`, `.parent`, `.getParent()`, `.children`, `.getChildren()`, `.rootContainer`, `.getRootContainer()` | that object (`AnyComponent` when it cannot be determined) |
//! | `.getSibling('A')`, `.getChild('A/B')` | that component; any component when the argument is not a string literal; nothing when no such component exists (the call returns `None`) |
//! | `.page`, `.session`, `.getPage()`, `.getSession()` | the host |
//! | `.props`, `.meta`, `.position`, `.id`, `.getId()`, `.print(...)`, `.focus()`, `.requestTooltip()`, `.removeTooltip()`, `.requestContextMenu()`, `.removeContextMenu()`, `.requestPrint(...)`, `.refreshBinding(...)`; `.name`, `.getName()` of the view | nothing (other property trees, names, actions) |
//! | `.name`, `.getName()` of a component (the members measured to return its name, `iglint_project::perspective_fields::component_name_members`) | a read of its name, recorded as `meta` `name` (for `NAME001`) |
//! | `.myMethod(...)`, a custom method the object defines in the view (any component's, for an unknown component) | nothing: the method's script is analysed as a script of the view; its arguments are values |
//! | `.custom = ...` | writes every custom property |
//! | `children[i]` of any component | any component |
//!
//! Understood uses of a **host**: its own property trees (`.props`,
//! `.custom`, ...), `.id`, `.name`, `.info`, `.project`, `.projectInfo`
//! and their getters, `.getPropertyTreeOf(...)`, `.print(...)`,
//! `.close(...)`, `.isConnected()`, `.refreshBinding(...)`,
//! `.invokeOnQueue(...)`: nothing; `.session`, `.getSession()`: the host.
//! Any other member of a page or session (`.view`, `.views`,
//! `.getViews()`, `.pages`, `.getPages()`, `.getPage(id)`, ...) reaches
//! other views (see "Other views"); any other name is everything.
//!
//! # Other views
//!
//! A page hands out the views it shows (`page.views`, `getViews()`, its
//! primary `page.view`), a session its pages (`session.pages`,
//! `getPages()`, `getPage(id)`), and the views arrive as view wrappers
//! with their `custom` and `params` trees and `getChild(...)`. What a
//! script does with another view is not a property access of the view
//! being linted, so the analysis records the first such use
//! (`Analysis::cross`) and the script layer counts it as unknown code
//! for the closed world (`AstScriptFacts::summarize`): `VIEW015` and
//! `NAME001` fixes anywhere in the workspace become unsafe and name the
//! site. A use reaches other views when it is
//!
//! * a member of a page or session other than the understood ones above
//!   (every member is measured in the Ignition image,
//!   `iglint_python_semantic::perspective`, and classified;
//!   `every_measured_perspective_member_is_classified`), a special name
//!   (`__getattribute__`, ...) of any followed object, or an item of a
//!   host;
//! * a component, the view, a page or a session that escapes: used as a
//!   value anywhere the analysis does not follow it (stored, put in a
//!   container, passed to a builtin, returned), since code that holds any
//!   of them can reach the page;
//! * a component, view, page or session passed to a name that may be a
//!   library function, when the function (followed with the project
//!   index, [`resolve_perspective_calls`]) reaches other views with it,
//!   passes it on to code that may, or is not a plain function.
//!
//! The followed objects are `self` in view, component and session
//! property scripts and the `session` and `page` parameters of the
//! session events; nothing else hands out a page or session (no
//! `system.perspective` function returns one, `getSessionInfo` returns
//! data). A loop over the children (`for c in self.getChildren():`, `c`
//! bound only there) follows `c` as any component, and truth and identity
//! tests (`if c:`, `c is None`) use no property, so neither is an
//! escape.
//!
//! Understood uses of a **container** (anything else, among them every
//! use as a value: `len(c)`, `for k in c`, `c == d`, `'%s' % c`, `f(c)`,
//! `return c`, `c.keys` without a call, `c[k]` with a computed key, is
//! reading and writing every property of the container):
//!
//! | form | meaning |
//! |---|---|
//! | `c.name` | property `name` (read, written or both by its context) |
//! | `c['name']` (string literal) | property `name` |
//! | `'name' in c`, `'name' not in c` | reads `name` |
//! | `c.get('name'...)`, `c.has_key('name')`, `c.__contains__('name')`, `c.__finditem__('name')` | reads `name` |
//! | `c.pop('name'...)`, `c.setdefault('name'...)` | reads and writes `name` |
//! | `c.keys()`, `.values()`, `.items()`, `.iterkeys()`, `.itervalues()`, `.iteritems()`, `.__iter__()`, `.__len__()`, `.__repr__()`, `.__str__()` | reads every property |
//!
//! The property wrapper (`PropertyTreeScriptWrapper` in Ignition 8.3)
//! looks a property up before its methods: `c.keys` is property `keys`
//! when it exists and the method otherwise (verified on the gateway's
//! classes). So an attribute named like a wrapper method
//! ([`CONTAINER_METHODS`]) is both: property `name`, and the method, which
//! must then be called in one of the forms above.
//!
//! Understood uses common to every followed value:
//!
//! | form | meaning |
//! |---|---|
//! | `x = <value>`, `x` assigned only there | an alias: every use of `x` is analysed like the value; the assignment itself reads nothing (in library functions it is recorded, [`PathEnd::Alias`], so that reads in its steps count) |
//! | `for x in <components>:`, `x` bound only there | `x` is any component |
//! | `if v:`, `while v:`, `not v`, `v is w`, `v is not None` (also in `and`/`or` of a test) | nothing for a component, view, page or session (a container is read) |
//! | an argument of a call of a dotted name the script does not bind (`lib.util.handle(self)`) | the library function is followed ([`LibraryCall`], [`param_uses`]) |
//! | in library functions: `p is None`, `p is not q`, `if p:`, `not p` | nothing (identity and truth of the parameter itself; of a value derived from it, [`PathEnd::Test`]) |
//!
//! # Frames and builtins
//!
//! Some code reads variables without naming them: the frame builtins
//! (`locals()`, `eval`, `vars()`, `dir()`, `execfile`, `exec`), frames
//! (`sys._getframe()`, `inspect`, `.f_locals`), the builtins module
//! (`__builtin__.eval`, `from __builtin__ import locals as l`) and any of
//! these used as a value (`e = eval`). A script (or library function)
//! that contains one of them
//! ([`iglint_python_semantic::dynamic::reads_frames`]) reads and writes
//! everything.
//!
//! A frame builtin can also arrive under another name from a library
//! module (`ev = eval` in a module, `lib.util.ev('self.custom.x')` in the
//! script: verified on Jython, the builtin reads the frame of the
//! caller). Every dotted name the script calls or uses as a value that
//! may be a library name (its first name is not bound by the script, not
//! a builtin and not an Ignition global, or it is bound by an import) is
//! checked through the project index ([`ArgSlot::Callee`]): unless it is a
//! plain `def` function or a class of a module without dynamic code, or
//! does not exist, the script reads and writes everything. Items of such
//! names (`lib.util.HANDLERS['x']`) are checked the same way. Calling a
//! value that does not come from a name (the result of a call, `f()()`,
//! `getattr(o, name)()`, or an item of a container, `handlers[k]()`)
//! reads and writes everything. Methods of other values (`x.m()` where
//! `x` is a local, an event or payload value or a call result) are
//! assumed not to be frame builtins.
//!
//! # Library functions
//!
//! A followed value passed to a call of a dotted name that the script
//! does not bind escapes, but the assumed accesses carry the call
//! ([`LibraryCall`]). Rules that have the project index resolve the
//! callee: a library function that does not exist reads nothing (the call
//! fails before it runs), and an existing one is followed through
//! [`ParamUse`], the summary of what the function does with each
//! parameter ([`param_uses`]), computed with the same rules on its body.
//! Parameter uses are recorded as paths of steps from the parameter and
//! replayed with the value of the call ([`follow_call`]).

use std::sync::Arc;

use iglint_project::library::{ArgSlot, ParamPath, ParamUse, PathEnd, PathStep};
use iglint_project::perspective_fields::is_component_name_member;
use iglint_project::view::{PropCategory, View};
use iglint_project::{ComponentId, Owner};
use iglint_python::ast::{
    CmpOp, Expr, ExprCall, ExprContext, Slice, Stmt, StmtFunctionDef, UnaryOp,
};
use iglint_python::visitor::{Visitor, walk_expr, walk_stmt};
use iglint_python_semantic::SemanticModel;
use iglint_python_semantic::dynamic::reads_frames;
use iglint_text::TextRange;
use rustc_hash::{FxHashMap, FxHashSet};

use super::{AccessTarget, PropertyAccess, ScriptFacts};
use crate::rule::ScriptUnit;
use iglint_project::summary::ResourceSummary;

/// Script facts from the syntax tree and the semantic model.
pub struct AstScriptFacts;

impl ScriptFacts for AstScriptFacts {
    fn complete(&self) -> bool {
        true
    }

    fn provides_references(&self) -> bool {
        true
    }

    fn summarize(
        &self,
        unit: &ScriptUnit<'_, '_>,
        view: Option<&View<'_>>,
        ctx: &super::FactsContext<'_>,
        summary: &mut ResourceSummary,
    ) {
        let library = unit.script.kind == iglint_project::ScriptKind::LibraryModule;
        if !unit.parsed.errors.is_empty() {
            // The script does not compile and never runs; a module that
            // does not compile has unknown symbols.
            return;
        }
        let model = iglint_python_semantic::SemanticModel::build(
            &unit.parsed.module,
            &iglint_python_semantic::SemanticOptions {
                injected: unit.script.implicit_globals().collect(),
                unknown_globals: false,
                module_has_name: unit.script.has_module_name(),
            },
        );
        let mut references = iglint_project::summary::References::default();
        super::refs::collect(&model, ctx, &mut references);
        // Place the sites of code that is not vetted in the file.
        let place = |site: &mut iglint_project::summary::UnknownSite| {
            if site.file == iglint_project::summary::UnknownSite::DETACHED {
                site.what = snippet(&unit.script.source, site.range);
                site.file = unit.file;
                site.range = unit.script.file_range(site.range);
            }
        };
        if let Some(site) = &mut references.unknown_site {
            place(site);
        }
        if let Some(site) = &mut references.file_read_site {
            place(site);
        }
        // The gateway context of a WebDev handler: every gateway manager,
        // tag imports and resource writes included (`webdev`).
        if matches!(
            unit.script.kind,
            iglint_project::ScriptKind::EventFile(iglint_project::EventScriptKind::WebDev(_))
        ) && let Some(range) = super::webdev::context_use(&unit.parsed.module.body)
        {
            let mut site = iglint_project::summary::UnknownSite::detached(range);
            place(&mut site);
            references.unknown_code = true;
            references.reads_files = true;
            if references.unknown_site.is_none() {
                references.unknown_site = Some(site.clone());
            }
            if references.file_read_site.is_none() {
                references.file_read_site = Some(site);
            }
        }
        // A Vision client script holds the client's objects, whose members
        // are not measured for file handles (`JAVA_FILE_MEMBERS` covers the
        // gateway): it may read files.
        if unit.script.run_scope == iglint_project::RunScope::VisionClient {
            references.reads_files = true;
            if references.file_read_site.is_none() {
                let mut site = iglint_project::summary::UnknownSite::detached(TextRange::empty(
                    unit.script.source_map.body_start(),
                ));
                place(&mut site);
                site.what = "a Vision client script".to_owned();
                references.file_read_site = Some(site);
            }
        }
        for (_, site) in &mut references.unvetted_imports {
            place(site);
        }
        for (_, site) in &mut references.unvetted_names {
            place(site);
            site.what
                .push_str(" (not in the Ignition 8.3 scripting API)");
        }
        // Perspective objects that may reach other views.
        let roots = perspective_roots(unit.script, view.is_some());
        if !roots.is_empty() && reads_frames(&unit.parsed.module.body).is_none() {
            let mut found = Vec::new();
            let mut a = Analysis::with_roots(
                Some(&model),
                view,
                roots,
                &unit.parsed.module.body,
                &mut found,
            );
            a.visit_module(&unit.parsed.module);
            if let Some(range) = a.cross {
                references.unknown_code = true;
                if references.unknown_site.is_none() {
                    let mut site = iglint_project::summary::UnknownSite::detached(range);
                    place(&mut site);
                    references.unknown_site = Some(site);
                }
            }
            let mut done: Vec<&Arc<LibraryCall>> = Vec::new();
            for access in &found {
                let Some(call) = &access.via else { continue };
                let host = match call.value {
                    Escaped::Host => true,
                    Escaped::Object(_) => false,
                    Escaped::Container(..) => continue,
                };
                if call.is_callee_check() || done.iter().any(|c| Arc::ptr_eq(c, call)) {
                    continue;
                }
                done.push(call);
                let mut site = iglint_project::summary::UnknownSite::detached(access.range);
                place(&mut site);
                references
                    .perspective_calls
                    .push(iglint_project::summary::PerspectiveCall {
                        callee: call.callee.clone(),
                        slot: call.slot.clone(),
                        host,
                        site,
                    });
            }
        }
        summary.references.extend(references);
        if library {
            summary.set_module_symbols(super::symbols::module_symbols(&model, &unit.parsed.module));
        }
    }

    fn property_accesses(
        &self,
        unit: &ScriptUnit<'_, '_>,
        view: &View<'_>,
        out: &mut Vec<PropertyAccess>,
    ) {
        let this = match unit.script.owner {
            Owner::View => Val::Object(AccessTarget::View),
            Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
            _ => return,
        };
        let anchor = unit.script.anchor();
        if !unit.parsed.is_valid() {
            // A script that does not compile never runs, so it writes
            // nothing; it is still taken as reading everything so that
            // unused-property findings do not depend on a script the user
            // has to fix anyway (SCR001 reports it).
            let start = out.len();
            let mut a = Analysis::new(None, Some(view), this, &[], out);
            a.everything(anchor);
            for access in &mut a.out[start..] {
                access.write = false;
            }
            return;
        }
        let body = &unit.parsed.module.body;
        // Resolve names through their real Python 2.7 scope, so a name
        // bound only in a nested function or class cannot hide a library
        // reference. Injected library packages are not needed here (they
        // are never a local), so the script's own implicit globals suffice.
        let model = script_model(unit);
        let mut found = Vec::new();
        if let Some(range) = reads_frames(body) {
            let mut a = Analysis::new(model.as_ref(), Some(view), this, &[], &mut found);
            a.everything(range);
        } else {
            let mut a = Analysis::new(model.as_ref(), Some(view), this, body, &mut found);
            a.visit_module(&unit.parsed.module);
        }
        for mut access in found {
            access.range = unit.script.file_range(access.range);
            out.push(access);
        }
    }
}

/// A value the analysis follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Val {
    /// A component or the view.
    Object(AccessTarget),
    /// A page or session: reaches every view in it.
    Host,
    Container(AccessTarget, PropCategory),
}

/// How an expression's value is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The value goes somewhere unknown.
    Value,
    /// The value is the base of an attribute, subscript or call that the
    /// caller interprets.
    Base,
}

/// The meaning of an attribute or method of a component, the view or a
/// host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Another followed value.
    Follow(Val),
    /// Something that reaches no property of the view (another property
    /// tree, a name, an action), or nothing at all (a lookup that fails).
    Opaque,
    /// Not understood: everything the base reaches may be accessed.
    Everything,
    /// A member of a page or session that hands out views, pages or
    /// components, or that is not understood: it may reach every view
    /// (see "Other views").
    Cross,
    /// A member of a component whose value derives from the component's
    /// name (`name`, `getName()`; measured,
    /// [`iglint_project::perspective_fields::component_name_members`]): a
    /// read of the name, recorded as [`PropCategory::Meta`] `name`.
    Name(AccessTarget),
}

/// Methods (and other attributes) of Perspective's property wrapper, from
/// `dir()` of `PropertyTreeScriptWrapper` and its `ObjectWrapper` in
/// Ignition 8.3. A property of the same name hides the method.
pub const CONTAINER_METHODS: &[&str] = &[
    "__class__",
    "__contains__",
    "__delattr__",
    "__delitem__",
    "__doc__",
    "__ensure_finalizer__",
    "__findattr_ex__",
    "__finditem__",
    "__format__",
    "__getattribute__",
    "__hash__",
    "__init__",
    "__iter__",
    "__len__",
    "__new__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__setitem__",
    "__str__",
    "__subclasshook__",
    "clear",
    "get",
    "has_key",
    "items",
    "iteritems",
    "iterkeys",
    "itervalues",
    "keys",
    "pop",
    "popitem",
    "setdefault",
    "update",
    "values",
];

/// Attributes of components and views that reach no custom property or
/// parameter: other property trees, names and actions
/// (`ComponentModelScriptWrapper.SafetyWrapper`,
/// `ViewModelScriptWrapper.SafetyWrapper`).
const OPAQUE_OBJECT_ATTRIBUTES: &[&str] = &[
    "props",
    "meta",
    "position",
    "name",
    "id",
    "getName",
    "getId",
    "print",
    "focus",
    "requestTooltip",
    "removeTooltip",
    "requestContextMenu",
    "removeContextMenu",
    "requestPrint",
    "refreshBinding",
];

/// Members of pages and sessions that reach no view
/// (`PageScriptWrapper.SafetyWrapper`, `SessionScriptWrapper.SafetyWrapper`,
/// measured in `iglint_python_semantic::perspective`): names, actions, the
/// page's or session's own property trees, and session data. `info` and
/// `projectInfo` are JSON data (`Gson.toJsonTree` of the session and
/// project info, read in the gateway's bytecode); `invokeOnQueue` calls
/// the callable it is given (its argument is a value like any other).
/// `id` is not a session member (it raises).
const OPAQUE_HOST_ATTRIBUTES: &[&str] = &[
    "close",
    "connected",
    "getId",
    "getInfo",
    "getName",
    "getProject",
    "getProjectInfo",
    "getPropertyTreeOf",
    "id",
    "info",
    "invokeOnQueue",
    "isConnected",
    "name",
    "print",
    "project",
    "projectInfo",
    "refreshBinding",
];

/// Members of pages and sessions that return the session: a host again.
/// (`page.page` and `getPage()` return the page itself, but a session's
/// `getPage(id)` and `pages` return other pages, so they reach other
/// views.)
const HOST_SESSION_MEMBERS: &[&str] = &["getSession", "session"];

/// What member `name` of a page or session is: its own property trees and
/// [`OPAQUE_HOST_ATTRIBUTES`] reach no view, [`HOST_SESSION_MEMBERS`] the
/// session, any other member of either wrapper (`views`, `getViews()`,
/// `view`, `pages`, `getPages()`, `getPage(id)`, ...) and any special
/// name reach other views; any other name raises (`AttributeError`) and
/// is treated, as before, as reaching everything of this view.
fn host_member(name: &str) -> Step {
    use iglint_python_semantic::perspective;
    if perspective::is_property_type(name) || OPAQUE_HOST_ATTRIBUTES.contains(&name) {
        return Step::Opaque;
    }
    if HOST_SESSION_MEMBERS.contains(&name) {
        return Step::Follow(Val::Host);
    }
    if is_special(name)
        || perspective::member_kind("page", name).is_some()
        || perspective::member_kind("session", name).is_some()
    {
        return Step::Cross;
    }
    Step::Everything
}

/// A special name (`__getattribute__`, `__dict__`, ...): Jython may resolve
/// it on the wrapper itself, and it may hand out any member.
fn is_special(name: &str) -> bool {
    name.starts_with("__") && name.ends_with("__")
}

/// Whether attribute `name` of a container may be something other than a
/// property: a method of the wrapper, or any special name (`__dict__`,
/// ...), which Jython may resolve on the object itself.
fn is_container_method(name: &str) -> bool {
    CONTAINER_METHODS.contains(&name) || (name.starts_with("__") && name.ends_with("__"))
}

/// A property access of a container use: the property (`None`: every
/// property), read, written.
type ContainerUse = (Option<String>, bool, bool);

/// What calling method `name` of a container with `literal` as first
/// argument accesses; `name` is also a property when one has that name.
fn container_call(name: &str, literal: Option<String>) -> Vec<ContainerUse> {
    let mut uses = vec![(Some(name.to_owned()), true, false)];
    if !is_container_method(name) {
        // Calling the property's value.
        return uses;
    }
    uses.push(match (name, literal) {
        ("get" | "has_key" | "__contains__" | "__finditem__", Some(l)) => (Some(l), true, false),
        ("pop" | "setdefault", Some(l)) => (Some(l), true, true),
        (
            "keys" | "values" | "items" | "iterkeys" | "itervalues" | "iteritems" | "__iter__"
            | "__len__" | "__repr__" | "__str__",
            _,
        ) => (None, true, false),
        _ => (None, true, true),
    });
    uses
}

/// What attribute `name` of a container accesses in context `ctx` (when it
/// is not called on the spot).
fn container_attribute(name: &str, ctx: ExprContext) -> Vec<ContainerUse> {
    let (read, write) = rw(ctx);
    let mut uses = vec![(Some(name.to_owned()), read, write)];
    if read && is_container_method(name) {
        // The method as a value: anything may be done with it.
        uses.push((None, true, true));
    }
    uses
}

fn rw(ctx: ExprContext) -> (bool, bool) {
    match ctx {
        ExprContext::Load => (true, false),
        ExprContext::AugLoad | ExprContext::AugStore => (true, true),
        ExprContext::Store | ExprContext::Del | ExprContext::Param => (false, true),
    }
}

/// Names the analysis treats as a call or use of a dotted name that may be
/// a library name (see "Frames and builtins").
struct Names {
    /// Extra names the caller declares bound (a library function's
    /// parameters), used as a fallback when no semantic model resolves an
    /// occurrence.
    bound: FxHashSet<String>,
    /// Names bound by imports, with the dotted names they may stand for.
    imports: FxHashMap<String, Vec<String>>,
    /// Range starts of `Load` names that the semantic model resolves to a
    /// real binding visible at the use site (a local, a closure variable,
    /// a module-level name or a builtin): such a name is not a library
    /// name, even when another, shadowing name of the same spelling is
    /// bound in a nested scope. Empty when no model is available.
    bound_heads: FxHashSet<u32>,
}

impl Names {
    fn new(
        model: Option<&SemanticModel<'_>>,
        body: &[Stmt],
        extra: impl IntoIterator<Item = String>,
    ) -> Names {
        Names {
            bound: extra.into_iter().collect(),
            imports: import_names(body),
            bound_heads: model
                .map(|m| bound_head_ranges(m, body))
                .unwrap_or_default(),
        }
    }

    /// The dotted names `e` may stand for when it is a chain of
    /// attributes on a name that may be a library name. The head is a
    /// library name unless the semantic model resolves it to a binding
    /// visible in its own scope ([`Names::bound_heads`]): a name bound only
    /// in a nested function, class or comprehension does not make it one.
    fn library_names(&self, e: &Expr, followed: &dyn Fn(&str) -> bool) -> Vec<String> {
        let Some(head) = dotted_head(e) else {
            return Vec::new();
        };
        let dotted = dotted_name(e).expect("a dotted head implies a dotted name");
        let rest = dotted.split_once('.').map(|(_, r)| r);
        let name = &*head.id;
        if followed(name) {
            return Vec::new();
        }
        if let Some(targets) = self.imports.get(name) {
            return targets
                .iter()
                .map(|t| match rest {
                    Some(r) => format!("{t}.{r}"),
                    None => t.clone(),
                })
                .collect();
        }
        if self.bound_heads.contains(&head.range.start().to_u32())
            || self.bound.contains(name)
            || is_known_global(name)
        {
            return Vec::new();
        }
        vec![dotted]
    }
}

/// Range starts of every `Load` name the model resolves to a binding of
/// the occurrence's own scope: a local, a closure variable (a name of an
/// enclosing function) or a builtin. Module-level names (`Global`) are
/// left in as library-name candidates, so a library module's own
/// functions and imports are still followed and resolved through the
/// project index; injected names (the project's library packages) and
/// unresolved or dynamic names stay candidates too. The point is that a
/// name bound only in a *nested* function, class or comprehension no
/// longer hides a free reference of the same spelling in the outer scope.
fn bound_head_ranges(model: &SemanticModel<'_>, body: &[Stmt]) -> FxHashSet<u32> {
    use iglint_python_semantic::Resolution;
    struct Heads<'m, 'a> {
        model: &'m SemanticModel<'a>,
        out: FxHashSet<u32>,
    }
    impl<'e> Visitor<'e> for Heads<'_, '_> {
        fn visit_expr(&mut self, expr: &'e Expr) {
            if let Expr::Name(n) = expr
                && n.ctx == ExprContext::Load
                && let Some(r) = self.model.reference_at(n.range)
                && matches!(
                    r.resolution,
                    Resolution::Local | Resolution::Free(_) | Resolution::Builtin
                )
            {
                self.out.insert(n.range.start().to_u32());
            }
            walk_expr(self, expr);
        }
    }
    let mut v = Heads {
        model,
        out: FxHashSet::default(),
    };
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.out
}

/// A semantic model of a parsed script, for resolving names through their
/// scope. The project's library packages are not injected: they are never
/// a local binding, so they resolve as library names whether injected or
/// not.
fn script_model<'s>(unit: &ScriptUnit<'s, '_>) -> Option<SemanticModel<'s>> {
    if !unit.parsed.is_valid() {
        return None;
    }
    Some(SemanticModel::build(
        &unit.parsed.module,
        &iglint_python_semantic::SemanticOptions {
            injected: unit.script.implicit_globals().collect(),
            unknown_globals: false,
            module_has_name: unit.script.has_module_name(),
        },
    ))
}

/// The text of `range` in `source`, on one line and shortened, to name a
/// construct in a message.
pub(crate) fn snippet(source: &str, range: TextRange) -> String {
    let text = source
        .get(range.start().to_usize()..range.end().to_usize())
        .unwrap_or("");
    let line = text.lines().next().unwrap_or("").trim();
    let mut out: String = line.chars().take(60).collect();
    if out.len() < line.len() || text.trim_end().contains('\n') {
        out.push_str("...");
    }
    out
}

/// The leftmost `Load` name of a `Load` attribute chain (`a` in
/// `a.b.c`), if `e` is one.
fn dotted_head(e: &Expr) -> Option<&iglint_python::ast::ExprName> {
    match e {
        Expr::Name(n) if n.ctx == ExprContext::Load => Some(n),
        Expr::Attribute(a) if a.ctx == ExprContext::Load => dotted_head(&a.value),
        _ => None,
    }
}

/// A builtin (other than the frame builtins, handled separately) or an
/// Ignition global: never a library name.
fn is_known_global(name: &str) -> bool {
    iglint_project::script::IMPLICIT_GLOBALS.contains(&name)
        || iglint_stubs::jython_builtins()
            .binary_search_by(|b| b.name.cmp(name))
            .is_ok()
}

struct Analysis<'v, 'a, 'o> {
    /// The view the script belongs to; `None` when it is not known (a
    /// library function followed for [`resolve_perspective_calls`]):
    /// component lookups then find any component.
    view: Option<&'v View<'a>>,
    /// The names that hold followed values from the start (`self`, the
    /// `session` or `page` of a session event).
    roots: Vec<(String, Val)>,
    aliases: FxHashMap<String, Option<Val>>,
    names: Names,
    out: &'o mut Vec<PropertyAccess>,
    /// The first use that may reach another view, page or session (see
    /// "Other views" in the module documentation).
    cross: Option<TextRange>,
}

impl<'v, 'a, 'o> Analysis<'v, 'a, 'o> {
    fn new(
        model: Option<&SemanticModel<'_>>,
        view: Option<&'v View<'a>>,
        this: Val,
        body: &[Stmt],
        out: &'o mut Vec<PropertyAccess>,
    ) -> Analysis<'v, 'a, 'o> {
        Analysis::with_roots(model, view, vec![("self".to_owned(), this)], body, out)
    }

    fn with_roots(
        model: Option<&SemanticModel<'_>>,
        view: Option<&'v View<'a>>,
        roots: Vec<(String, Val)>,
        body: &[Stmt],
        out: &'o mut Vec<PropertyAccess>,
    ) -> Analysis<'v, 'a, 'o> {
        let mut a = Analysis {
            view,
            roots,
            aliases: FxHashMap::default(),
            names: Names::new(model, body, []),
            out,
            cross: None,
        };
        a.collect_aliases(body);
        a
    }

    /// `range` may reach another view: record it.
    fn cross_view(&mut self, range: TextRange) {
        if self.cross.is_none_or(|c| range.start() < c.start()) {
            self.cross = Some(range);
        }
    }

    fn is_root(&self, name: &str) -> bool {
        self.roots.iter().any(|(n, _)| n == name)
    }

    // ---- aliases ----

    /// Names assigned exactly once, from a followed value.
    fn collect_aliases(&mut self, body: &[Stmt]) {
        let stores = single_assignments(body);
        // Resolve in a few rounds so aliases of aliases work.
        for _ in 0..3 {
            for (name, value) in &stores {
                if self.is_root(name) {
                    continue;
                }
                let v = match value {
                    Some(Assigned::Value(value)) => self.peek(value),
                    // `for c in <components>:`
                    Some(Assigned::Item(iter)) => match self.peek(iter) {
                        Some(any @ Val::Object(AccessTarget::AnyComponent)) => Some(any),
                        _ => None,
                    },
                    None => None,
                };
                self.aliases.insert(name.clone(), v);
            }
        }
    }

    /// The value of `e` without recording anything.
    fn peek(&self, e: &Expr) -> Option<Val> {
        match e {
            Expr::Name(n) if n.ctx == ExprContext::Load => self.name_value(&n.id),
            Expr::Attribute(a) => match self.attribute(self.peek(&a.value)?, a.attr.as_str()) {
                Step::Follow(v) => Some(v),
                _ => None,
            },
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else {
                    return None;
                };
                let base = self.peek(&a.value)?;
                match self.method(base, a.attr.as_str(), first_literal(&c.args)) {
                    Step::Follow(v) => Some(v),
                    _ => None,
                }
            }
            Expr::Subscript(s) => match self.peek(&s.value)? {
                Val::Object(AccessTarget::AnyComponent) => {
                    Some(Val::Object(AccessTarget::AnyComponent))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn name_value(&self, name: &str) -> Option<Val> {
        if let Some((_, v)) = self.roots.iter().find(|(n, _)| n == name) {
            return Some(*v);
        }
        self.aliases.get(name).copied().flatten()
    }

    fn is_followed_name(&self, name: &str) -> bool {
        self.is_root(name) || self.aliases.get(name).is_some_and(Option::is_some)
    }

    // ---- navigation ----

    fn parent(&self, t: AccessTarget) -> AccessTarget {
        match t {
            AccessTarget::Component(c) => match self.view.and_then(|v| v.component(c).parent) {
                Some(p) => AccessTarget::Component(p),
                None => AccessTarget::AnyComponent,
            },
            _ => AccessTarget::AnyComponent,
        }
    }

    fn child_path(&self, from: Option<ComponentId>, path: &str) -> Option<AccessTarget> {
        let Some(view) = self.view else {
            return Some(AccessTarget::AnyComponent);
        };
        let mut segments = path.split('/').filter(|s| !s.is_empty());
        let mut at = match from {
            Some(c) => c,
            None => {
                // From the view: the first segment names the root.
                let root = view.root_component()?;
                if segments.next()? != root.name {
                    return None;
                }
                root.id
            }
        };
        for segment in segments {
            at = view.children(at).find(|c| c.name == segment)?.id;
        }
        Some(AccessTarget::Component(at))
    }

    fn root(&self) -> Step {
        match self.view.and_then(View::root_component) {
            Some(r) => Step::Follow(Val::Object(AccessTarget::Component(r.id))),
            None => Step::Follow(Val::Object(AccessTarget::AnyComponent)),
        }
    }

    /// Attribute `attr` of a component, the view or a host, read (not
    /// called on the spot). Containers are handled by the caller.
    fn attribute(&self, base: Val, attr: &str) -> Step {
        let t = match base {
            Val::Object(t) => t,
            Val::Host => return host_member(attr),
            Val::Container(..) => return Step::Everything,
        };
        let component = matches!(t, AccessTarget::Component(_) | AccessTarget::AnyComponent);
        match attr {
            "custom" => Step::Follow(Val::Container(t, PropCategory::Custom)),
            "params" if t == AccessTarget::View => {
                Step::Follow(Val::Container(t, PropCategory::Params))
            }
            "rootContainer" if t == AccessTarget::View => self.root(),
            // `self.view` also works on the view itself (view-level
            // bindings and events).
            "view" => Step::Follow(Val::Object(AccessTarget::View)),
            "parent" if component => Step::Follow(Val::Object(self.parent(t))),
            "children" if component => Step::Follow(Val::Object(AccessTarget::AnyComponent)),
            "page" | "session" => Step::Follow(Val::Host),
            _ if component && is_component_name_member(attr) => Step::Name(t),
            _ if OPAQUE_OBJECT_ATTRIBUTES.contains(&attr) => Step::Opaque,
            // `self.__getattribute__`, ...: any member, the page included.
            _ if is_special(attr) => Step::Cross,
            _ => Step::Everything,
        }
    }

    /// Method `name` of a component, the view or a host, called on the
    /// spot with `literal` as its first argument.
    fn method(&self, base: Val, name: &str, literal: Option<String>) -> Step {
        let t = match base {
            Val::Object(t) => t,
            Val::Host => return host_member(name),
            Val::Container(..) => return Step::Everything,
        };
        let component = matches!(t, AccessTarget::Component(_) | AccessTarget::AnyComponent);
        let found = |t: Option<AccessTarget>| match t {
            Some(t) => Step::Follow(Val::Object(t)),
            // The lookup returns `None`.
            None => Step::Opaque,
        };
        match (t, name) {
            (AccessTarget::Component(c), "getSibling") => {
                let Some(name) = literal else {
                    return Step::Follow(Val::Object(AccessTarget::AnyComponent));
                };
                let Some(view) = self.view else {
                    return Step::Follow(Val::Object(AccessTarget::AnyComponent));
                };
                let Some(parent) = view.component(c).parent else {
                    return Step::Opaque;
                };
                // `getSibling` looks the name up in the parent, so a
                // component finds itself too.
                found(
                    view.children(parent)
                        .find(|s| s.name == name)
                        .map(|s| AccessTarget::Component(s.id)),
                )
            }
            (AccessTarget::Component(c), "getChild") => match literal {
                Some(path) => found(self.child_path(Some(c), &path)),
                None => Step::Follow(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::View, "getChild") => match literal {
                Some(path) => found(self.child_path(None, &path)),
                None => Step::Follow(Val::Object(AccessTarget::AnyComponent)),
            },
            (AccessTarget::AnyComponent, "getSibling" | "getChild") => {
                Step::Follow(Val::Object(AccessTarget::AnyComponent))
            }
            (_, "getParent") if component => Step::Follow(Val::Object(self.parent(t))),
            (_, "getChildren") if component => {
                Step::Follow(Val::Object(AccessTarget::AnyComponent))
            }
            (AccessTarget::View, "getRootContainer") => self.root(),
            (_, "getView") => Step::Follow(Val::Object(AccessTarget::View)),
            (_, "getPage" | "getSession") => Step::Follow(Val::Host),
            _ if component && is_component_name_member(name) => Step::Name(t),
            _ if OPAQUE_OBJECT_ATTRIBUTES.contains(&name) => Step::Opaque,
            // A custom method of the view: its script is analysed as a
            // script of the view (with its own `self`).
            _ if is_special(name) => Step::Cross,
            _ if self.is_custom_method(t, name) => Step::Opaque,
            _ => Step::Everything,
        }
    }

    /// Whether `t` has a custom method `name` (any component, for
    /// [`AccessTarget::AnyComponent`]).
    fn is_custom_method(&self, t: AccessTarget, name: &str) -> bool {
        let Some(view) = self.view else {
            // Any name that is not a member of the wrapper may be a custom
            // method of some component (or a model delegate's method).
            let wrapper = if t == AccessTarget::View {
                "view"
            } else {
                "component"
            };
            return iglint_python_semantic::perspective::member_kind(wrapper, name).is_none();
        };
        view.custom_methods.iter().any(|m| {
            m.name.is_some_and(|n| n.value() == name)
                && match t {
                    AccessTarget::View => m.owner == Owner::View,
                    AccessTarget::Component(c) => m.owner == Owner::Component(c),
                    AccessTarget::AnyComponent => matches!(m.owner, Owner::Component(_)),
                }
        })
    }

    // ---- recording ----

    fn record(
        &mut self,
        t: AccessTarget,
        category: PropCategory,
        name: Option<String>,
        ctx: ExprContext,
        range: TextRange,
    ) {
        let (read, write) = rw(ctx);
        self.push(t, category, (name, read, write), range);
    }

    fn push(&mut self, t: AccessTarget, category: PropCategory, u: ContainerUse, range: TextRange) {
        let (name, read, write) = u;
        self.out.push(PropertyAccess {
            target: t,
            category,
            name,
            read,
            write,
            range,
            via: None,
        });
    }

    fn uses(
        &mut self,
        t: AccessTarget,
        c: PropCategory,
        uses: Vec<ContainerUse>,
        range: TextRange,
    ) {
        for u in uses {
            self.push(t, c, u, range);
        }
    }

    /// A followed value escapes: everything reachable may be accessed.
    fn escape(&mut self, val: Val, range: TextRange) {
        if matches!(val, Val::Object(_) | Val::Host) {
            // Code that holds a component, the view, a page or a session
            // can reach the page and every view in it.
            self.cross_view(range);
        }
        self.escape_locally(val, range);
    }

    /// [`Analysis::escape`] for this view only.
    fn escape_locally(&mut self, val: Val, range: TextRange) {
        match val {
            // A container passed on may be read and written.
            Val::Container(t, c) => self.record(t, c, None, ExprContext::AugLoad, range),
            Val::Object(_) | Val::Host => self.everything(range),
        }
    }

    fn everything(&mut self, range: TextRange) {
        for (t, c) in [
            (AccessTarget::View, PropCategory::Custom),
            (AccessTarget::View, PropCategory::Params),
            (AccessTarget::AnyComponent, PropCategory::Custom),
        ] {
            self.out.push(PropertyAccess {
                target: t,
                category: c,
                name: None,
                read: true,
                write: true,
                range,
                via: None,
            });
        }
    }

    /// [`Analysis::escape`] into a call of `callee`: the assumed accesses
    /// carry the call.
    fn escape_into(&mut self, val: Val, range: TextRange, callee: &str, slot: ArgSlot) {
        let start = self.out.len();
        // Whether the call reaches other views is decided where the callee
        // is resolved ([`resolve_perspective_calls`]).
        self.escape_locally(val, range);
        let call = Arc::new(LibraryCall {
            callee: callee.to_owned(),
            slot,
            value: val.into(),
        });
        for access in &mut self.out[start..] {
            access.via = Some(Arc::clone(&call));
        }
    }

    /// `e` is called or used as a value: when it may be a library name
    /// that is not a plain function, it may be a frame builtin.
    fn check_callee(&mut self, e: &Expr) {
        let range = e_range(e);
        let followed = |n: &str| self.is_followed_name(n);
        for callee in self.names.library_names(e, &followed) {
            self.escape_into(
                Val::Object(AccessTarget::View),
                range,
                &callee,
                ArgSlot::Callee,
            );
        }
    }

    /// The dotted name a call calls, when it may be a library function
    /// that followed values are passed to.
    fn library_callee(&self, func: &Expr) -> Option<String> {
        let followed = |n: &str| self.is_followed_name(n);
        let mut names = self.names.library_names(func, &followed);
        (names.len() == 1).then(|| names.remove(0))
    }

    // ---- walking ----

    /// Walks an expression; returns its followed value when `role` is
    /// [`Role::Base`]. In [`Role::Value`] a followed value escapes.
    fn walk(&mut self, e: &Expr, role: Role, aug: bool) -> Option<Val> {
        let result = match e {
            Expr::Name(n) => match n.ctx {
                ExprContext::Load | ExprContext::AugLoad => self.name_value(&n.id),
                _ => None,
            },
            Expr::Attribute(a) => self.walk_attribute(a, aug),
            Expr::Subscript(s) => {
                let base = self.walk(&s.value, Role::Base, false);
                if base.is_none() {
                    // An item of a library value may be anything.
                    self.check_callee(&s.value);
                }
                let key = literal_key(&s.slice);
                self.walk_slice(&s.slice);
                let ctx = if aug { ExprContext::AugStore } else { s.ctx };
                match base {
                    Some(Val::Container(t, c)) => {
                        self.record(t, c, key, ctx, s.range);
                        None
                    }
                    Some(any @ Val::Object(AccessTarget::AnyComponent))
                        if ctx == ExprContext::Load =>
                    {
                        Some(any)
                    }
                    Some(other) => {
                        self.escape(other, s.range);
                        None
                    }
                    None => None,
                }
            }
            Expr::Call(c) => self.walk_call(c),
            Expr::Compare(c) if c.ops.iter().all(|o| matches!(o, CmpOp::Is | CmpOp::IsNot)) => {
                self.test(&c.left);
                for comparator in &c.comparators {
                    self.test(comparator);
                }
                None
            }
            Expr::UnaryOp(u) if u.op == UnaryOp::Not => {
                self.test(&u.operand);
                None
            }
            Expr::Compare(c) if is_literal_in(c) => {
                let base = self.walk(&c.comparators[0], Role::Base, false);
                match base {
                    Some(Val::Container(t, cat)) => {
                        let Expr::Str(s) = &*c.left else {
                            unreachable!()
                        };
                        let name = s.s.as_str().map(str::to_owned);
                        self.record(t, cat, name, ExprContext::Load, c.range);
                    }
                    Some(other) => self.escape(other, c.range),
                    None => {}
                }
                None
            }
            other => {
                // Any other expression: walk its children as values.
                let mut children = Children::default();
                walk_expr(&mut children, other);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                None
            }
        };
        match (role, result) {
            (Role::Value, Some(v)) => {
                self.escape(v, e_range(e));
                None
            }
            (Role::Value, None) => {
                if matches!(e, Expr::Name(_) | Expr::Attribute(_)) {
                    self.check_callee(e);
                }
                None
            }
            _ => result,
        }
    }

    fn walk_attribute(&mut self, a: &iglint_python::ast::ExprAttribute, aug: bool) -> Option<Val> {
        let base = self.walk(&a.value, Role::Base, false)?;
        let attr = a.attr.as_str();
        let range = a.attr.range;
        let ctx = if aug { ExprContext::AugStore } else { a.ctx };
        match base {
            Val::Container(t, c) => {
                self.uses(t, c, container_attribute(attr, ctx), range);
                None
            }
            Val::Object(t) if ctx != ExprContext::Load => {
                if attr == "custom" {
                    // `self.custom = {...}` replaces every property.
                    let ctx = if aug {
                        ExprContext::AugStore
                    } else {
                        ExprContext::Store
                    };
                    self.record(t, PropCategory::Custom, None, ctx, range);
                } else if !OPAQUE_OBJECT_ATTRIBUTES.contains(&attr) {
                    self.everything(range);
                    if is_special(attr) {
                        self.cross_view(a.range);
                    }
                }
                None
            }
            Val::Host if ctx != ExprContext::Load => {
                if host_member(attr) != Step::Opaque {
                    self.everything(range);
                    self.cross_view(a.range);
                }
                None
            }
            b => {
                let step = self.attribute(b, attr);
                self.apply(step, range, a.range)
            }
        }
    }

    /// An expression whose truth value or identity is all that is used
    /// (`if x:`, `not x`, `x is None`): a component, the view, a page or a
    /// session is not handed to anything (its `__nonzero__` reads no
    /// property). A container still counts as read (its truth value
    /// depends on its properties).
    fn test(&mut self, e: &Expr) {
        if let Expr::BoolOp(b) = e {
            for value in &b.values {
                self.test(value);
            }
            return;
        }
        match self.walk(e, Role::Base, false) {
            Some(Val::Object(_) | Val::Host) => {}
            Some(c @ Val::Container(..)) => self.escape(c, e_range(e)),
            None => {
                if matches!(e, Expr::Name(_) | Expr::Attribute(_)) {
                    self.check_callee(e);
                }
            }
        }
    }

    /// Records what a step does at `range` (`site`: the whole expression,
    /// to name a use that reaches other views); the followed value, if
    /// any.
    fn apply(&mut self, step: Step, range: TextRange, site: TextRange) -> Option<Val> {
        match step {
            Step::Follow(v) => Some(v),
            Step::Opaque => None,
            Step::Everything => {
                self.everything(range);
                None
            }
            Step::Cross => {
                self.everything(range);
                self.cross_view(site);
                None
            }
            Step::Name(t) => {
                self.name_read(t, range);
                None
            }
        }
    }

    /// The name of component `t` is read at `range`.
    fn name_read(&mut self, t: AccessTarget, range: TextRange) {
        self.out.push(PropertyAccess {
            target: t,
            category: PropCategory::Meta,
            name: Some("name".to_owned()),
            read: true,
            write: false,
            range,
            via: None,
        });
    }

    fn walk_call(&mut self, c: &ExprCall) -> Option<Val> {
        let mut value = None;
        let mut library = None;
        match &*c.func {
            Expr::Attribute(a) => match self.walk(&a.value, Role::Base, false) {
                Some(Val::Container(t, cat)) => {
                    let uses = container_call(a.attr.as_str(), first_literal(&c.args));
                    self.uses(t, cat, uses, a.range);
                }
                Some(base) => {
                    let step = self.method(base, a.attr.as_str(), first_literal(&c.args));
                    value = self.apply(step, a.range, c.range);
                }
                None => {
                    self.check_callee(&c.func);
                    library = self.library_callee(&c.func);
                }
            },
            Expr::Name(n) => {
                match self.name_value(&n.id) {
                    // A followed value called.
                    Some(v) => self.escape(v, n.range),
                    None => {
                        self.check_callee(&c.func);
                        library = self.library_callee(&c.func);
                    }
                }
            }
            other => {
                if calls_unnamed_value(other) {
                    // The result of a call or an item of a container,
                    // called: it may be a frame builtin.
                    self.everything(e_range(other));
                }
                self.walk(other, Role::Value, false);
            }
        }
        match library {
            Some(callee) => self.walk_call_arguments(c, &callee),
            None => {
                for arg in &c.args {
                    self.walk(arg, Role::Value, false);
                }
                for k in &c.keywords {
                    self.walk(&k.value, Role::Value, false);
                }
            }
        }
        if let Some(s) = &c.starargs {
            self.walk(s, Role::Value, false);
        }
        if let Some(k) = &c.kwargs {
            self.walk(k, Role::Value, false);
        }
        value
    }

    fn walk_slice(&mut self, slice: &Slice) {
        let mut children = Children::default();
        children.visit_slice(slice);
        for child in children.0 {
            self.walk(child, Role::Value, false);
        }
    }

    /// The positional and keyword arguments of a call of `callee`: followed
    /// values escape into the call.
    fn walk_call_arguments(&mut self, c: &ExprCall, callee: &str) {
        for (i, arg) in c.args.iter().enumerate() {
            if let Some(v) = self.walk(arg, Role::Base, false) {
                self.escape_into(v, e_range(arg), callee, ArgSlot::Position(i));
            }
        }
        for k in &c.keywords {
            if let Some(v) = self.walk(&k.value, Role::Base, false) {
                let slot = ArgSlot::Keyword(k.arg.as_str().to_owned());
                self.escape_into(v, e_range(&k.value), callee, slot);
            }
        }
    }

    /// What `path` does when the parameter it starts from holds `start`
    /// (a value of this view). Calls it passes the value on to go to
    /// `passed`.
    fn replay(
        &mut self,
        start: Val,
        path: &ParamPath,
        range: TextRange,
        passed: &mut dyn FnMut(&mut Self, &str, &ArgSlot, Val),
    ) {
        let mut v = start;
        let last = path.steps.len().saturating_sub(1);
        for (k, step) in path.steps.iter().enumerate() {
            let ctx = if k < last {
                ExprContext::Load
            } else {
                match path.end {
                    PathEnd::Store => ExprContext::Store,
                    PathEnd::Update => ExprContext::AugStore,
                    _ => ExprContext::Load,
                }
            };
            let next = match (v, step) {
                (Val::Container(t, c), PathStep::Attr(n)) => {
                    self.uses(t, c, container_attribute(n, ctx), range);
                    return;
                }
                (Val::Container(t, c), PathStep::Index(key)) => {
                    self.record(t, c, key.clone(), ctx, range);
                    return;
                }
                (Val::Container(t, c), PathStep::Method { name, literal }) => {
                    self.uses(t, c, container_call(name, literal.clone()), range);
                    return;
                }
                (Val::Object(t), PathStep::Attr(n)) if ctx != ExprContext::Load => {
                    if n == "custom" {
                        // `param.custom = {...}` replaces every property.
                        self.record(t, PropCategory::Custom, None, ctx, range);
                    } else if !OPAQUE_OBJECT_ATTRIBUTES.contains(&n.as_str()) {
                        self.everything(range);
                        if is_special(n) {
                            self.cross_view(range);
                        }
                    }
                    return;
                }
                (Val::Host, PathStep::Attr(n)) if ctx != ExprContext::Load => {
                    if host_member(n) != Step::Opaque {
                        self.everything(range);
                        self.cross_view(range);
                    }
                    return;
                }
                (_, PathStep::Attr(n)) => self.attribute(v, n),
                (_, PathStep::Method { name, literal }) => self.method(v, name, literal.clone()),
                (Val::Object(AccessTarget::AnyComponent), PathStep::Index(_))
                    if ctx == ExprContext::Load =>
                {
                    Step::Follow(v)
                }
                (Val::Host, PathStep::Index(_)) => Step::Cross,
                (_, PathStep::Index(_)) => Step::Everything,
            };
            match self.apply(next, range, range) {
                Some(n) => v = n,
                None => return,
            }
        }
        match &path.end {
            PathEnd::Passed { callee, slot } => passed(self, callee, slot, v),
            PathEnd::Alias => {}
            PathEnd::Contains(name) => match v {
                Val::Container(t, c) => {
                    self.record(t, c, Some(name.clone()), ExprContext::Load, range);
                }
                _ => self.escape(v, range),
            },
            PathEnd::Test => {
                if let Val::Container(..) = v {
                    self.escape(v, range);
                }
            }
            PathEnd::Load | PathEnd::Store | PathEnd::Update => self.escape(v, range),
        }
    }
}

impl<'e> Visitor<'e> for Analysis<'_, '_, '_> {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                if let [Expr::Name(n)] = s.targets.as_slice()
                    && self.aliases.get(&*n.id).copied().flatten().is_some()
                {
                    // An alias definition: the value is followed, not lost.
                    self.walk(&s.value, Role::Base, false);
                    return;
                }
                for t in &s.targets {
                    self.walk(t, Role::Value, false);
                }
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::AugAssign(s) => {
                self.walk(&s.target, Role::Value, true);
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::Exec(s) => {
                self.everything(s.range);
                walk_stmt(self, stmt);
            }
            Stmt::For(f)
                if matches!(&*f.target, Expr::Name(n)
                    if self.aliases.get(&*n.id).copied().flatten().is_some()) =>
            {
                // `for c in self.getChildren():` with `c` bound only
                // here: `c` is followed as any component.
                self.walk(&f.iter, Role::Base, false);
                for stmt in f.body.iter().chain(&f.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::If(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::While(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.walk(expr, Role::Value, false);
    }
}

/// The names of a script that hold Perspective objects from the start:
/// `self` in view, component and session property scripts, and the
/// `session` and `page` parameters of the Perspective session events
/// (`has_view`: the view of a view script is known).
fn perspective_roots(
    script: &iglint_project::EmbeddedScript<'_>,
    has_view: bool,
) -> Vec<(String, Val)> {
    use iglint_project::{EventScriptKind as E, ScriptKind};
    let this = match script.owner {
        Owner::View => Some(Val::Object(AccessTarget::View)),
        Owner::Component(c) if has_view => Some(Val::Object(AccessTarget::Component(c))),
        Owner::Component(_) => Some(Val::Object(AccessTarget::AnyComponent)),
        Owner::Session => Some(Val::Host),
        Owner::Tag(_) | Owner::Resource => None,
    };
    if let Some(this) = this {
        return vec![("self".to_owned(), this)];
    }
    let ScriptKind::EventFile(kind) = script.kind else {
        return Vec::new();
    };
    let perspective = matches!(
        kind,
        E::SessionStartup
            | E::SessionShutdown
            | E::PageStartup
            | E::SessionMessage
            | E::KeyEvent
            | E::Barcode
            | E::Bluetooth
            | E::NfcScan
            | E::Accelerometer
            | E::AuthChallenge
            | E::FormSubmission
    );
    if !perspective {
        return Vec::new();
    }
    script
        .params
        .iter()
        .filter(|p| matches!(&***p, "session" | "page"))
        .map(|p| (p.to_string(), Val::Host))
        .collect()
}

/// Resolves the [`iglint_project::summary::PerspectiveCall`]s of the
/// workspace through the index: a call whose callee may reach another
/// view, page or session with the object it is handed (the library
/// function does, it passes the object on to code that may, or the callee
/// is not a library function that can be followed) counts as code that is
/// not vetted, at the call ([`ProjectIndex::mark_unknown_code`]). Run once
/// after every [`ProjectIndex::build`].
pub fn resolve_perspective_calls(index: &mut iglint_project::ProjectIndex<'_>) {
    let calls = index.perspective_calls();
    if calls.is_empty() {
        return;
    }
    let mut found = Vec::new();
    {
        let index = &*index;
        for (project, call) in calls {
            let resolve = |callee: &str, context: Option<CalleeContext>, check: bool| {
                crate::rules::view::unused_custom_property::resolve_callee(
                    index, project, callee, context, check, 0,
                )
            };
            let mut out = Vec::new();
            let mut a = Analysis::with_roots(None, None, Vec::new(), &[], &mut out);
            let value = if call.host {
                Val::Host
            } else {
                Val::Object(AccessTarget::AnyComponent)
            };
            follow(
                &mut a,
                &call.callee,
                None,
                &call.slot,
                value,
                TextRange::default(),
                &resolve,
                0,
            );
            if a.cross.is_some() {
                found.push((project, call.site));
            }
        }
    }
    for (project, site) in found {
        index.mark_unknown_code(project, site);
    }
}

/// A followed value of a view script that is passed to a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Escaped {
    /// A component or the view.
    Object(AccessTarget),
    /// A page or session.
    Host,
    /// A property container (`self.view.custom`).
    Container(AccessTarget, PropCategory),
}

impl From<Val> for Escaped {
    fn from(v: Val) -> Escaped {
        match v {
            Val::Object(t) => Escaped::Object(t),
            Val::Host => Escaped::Host,
            Val::Container(t, c) => Escaped::Container(t, c),
        }
    }
}

impl From<Escaped> for Val {
    fn from(v: Escaped) -> Val {
        match v {
            Escaped::Object(t) => Val::Object(t),
            Escaped::Host => Val::Host,
            Escaped::Container(t, c) => Val::Container(t, c),
        }
    }
}

/// A call that a followed value of a view script is passed to (or, with
/// [`ArgSlot::Callee`], a name the script calls or uses as a value that
/// must be checked not to be a frame builtin).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LibraryCall {
    /// The dotted name called, as written (`lib.util.handle`).
    pub callee: String,
    /// The parameter the value binds to.
    pub slot: ArgSlot,
    pub value: Escaped,
}

impl LibraryCall {
    /// Whether this only checks the callee ([`ArgSlot::Callee`]).
    pub fn is_callee_check(&self) -> bool {
        self.slot == ArgSlot::Callee
    }
}

/// How a callee resolves, for [`follow_call`].
pub enum Callee<'i> {
    /// It does not exist: the call fails before anything is read.
    Missing,
    /// A library function; `context` resolves the names it calls.
    Function(&'i iglint_project::library::Signature, CalleeContext),
    /// Not a frame builtin, but not followed either (a class, a name that
    /// is not a library name): a value passed to it may be accessed in
    /// any way; calling it reads no frame.
    Opaque,
    /// Anything else (a variable, an import of something unknown, a name
    /// that may be rebound): it may even be a frame builtin.
    Unknown,
}

/// Where a function is defined, to resolve the names its body calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalleeContext {
    pub module: iglint_project::ResourceId,
}

/// How many calls deep [`follow_call`] follows a value before it assumes
/// everything is accessed.
pub const MAX_CALL_DEPTH: usize = 4;

/// The accesses a call makes with `value` of `view`, bound to parameter
/// `slot` of `callee`. `resolve` turns a callee name (relative to the
/// calling module, `None` for the view script) into a [`Callee`]; its
/// last argument tells that only the callee is checked
/// ([`ArgSlot::Callee`]). Unknown
/// callees, dynamic parameters and calls nested deeper than
/// [`MAX_CALL_DEPTH`] access everything the value reaches.
pub fn follow_call<'i>(
    view: &View<'_>,
    call: &LibraryCall,
    range: TextRange,
    resolve: &dyn Fn(&str, Option<CalleeContext>, bool) -> Callee<'i>,
) -> Vec<PropertyAccess> {
    let mut out = Vec::new();
    let mut a = Analysis::new(
        None,
        Some(view),
        Val::Object(AccessTarget::View),
        &[],
        &mut out,
    );
    follow(
        &mut a,
        &call.callee,
        None,
        &call.slot,
        call.value.into(),
        range,
        resolve,
        0,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn follow<'i>(
    a: &mut Analysis<'_, '_, '_>,
    callee: &str,
    context: Option<CalleeContext>,
    slot: &ArgSlot,
    value: Val,
    range: TextRange,
    resolve: &dyn Fn(&str, Option<CalleeContext>, bool) -> Callee<'i>,
    depth: usize,
) {
    // A callee check follows nothing further, so it is not limited.
    if depth >= MAX_CALL_DEPTH && *slot != ArgSlot::Callee {
        a.escape(value, range);
        return;
    }
    match (resolve(callee, context, *slot == ArgSlot::Callee), slot) {
        (Callee::Missing, _) => {}
        (Callee::Function(..) | Callee::Opaque, ArgSlot::Callee) => {}
        (Callee::Unknown, _) | (Callee::Opaque, _) => a.escape(value, range),
        (Callee::Function(signature, inner), _) => match signature.use_of(slot) {
            ParamUse::Dynamic => a.escape(value, range),
            ParamUse::Paths(paths) => {
                for path in paths {
                    a.replay(value, path, range, &mut |a, callee, slot, v| {
                        follow(a, callee, Some(inner), slot, v, range, resolve, depth + 1);
                    });
                }
            }
        },
    }
}

/// Whether calling `func` calls a value that does not come from a name:
/// the result of a call (`f()()`, `getattr(o, name)()`) or an item of a
/// container (`handlers[k]()`, `vars()['f']()`). Such a value may be a
/// frame builtin. `getattr(o, 'literal')(...)` is a method call.
fn calls_unnamed_value(func: &Expr) -> bool {
    match func {
        Expr::Subscript(_) => true,
        Expr::Call(c) => {
            !(matches!(&*c.func, Expr::Name(n) if &*n.id == "getattr")
                && c.args.len() == 2
                && matches!(&c.args[1], Expr::Str(_)))
        }
        _ => false,
    }
}

/// The first argument of a call, when it is a string literal.
fn first_literal(args: &[Expr]) -> Option<String> {
    match args.first() {
        Some(Expr::Str(s)) => s.s.as_str().map(str::to_owned),
        _ => None,
    }
}

/// The key of a subscript when it is a string literal.
fn literal_key(slice: &Slice) -> Option<String> {
    match slice {
        Slice::Index(i) => match &*i.value {
            Expr::Str(st) => st.s.as_str().map(str::to_owned),
            _ => None,
        },
        _ => None,
    }
}

/// `'name' in x` or `'name' not in x`.
fn is_literal_in(c: &iglint_python::ast::ExprCompare) -> bool {
    matches!(c.ops.as_slice(), [CmpOp::In | CmpOp::NotIn])
        && matches!(&*c.left, Expr::Str(s) if s.s.as_str().is_some())
}

/// `a.b.c` for a chain of attribute reads on a name.
fn dotted_name(e: &Expr) -> Option<String> {
    match e {
        Expr::Name(n) if n.ctx == ExprContext::Load => Some(n.id.to_string()),
        Expr::Attribute(a) if a.ctx == ExprContext::Load => {
            let mut base = dotted_name(&a.value)?;
            base.push('.');
            base.push_str(a.attr.as_str());
            Some(base)
        }
        _ => None,
    }
}

/// What a name is bound to by its only binding.
#[derive(Clone, Copy, Debug)]
enum Assigned<'e> {
    /// `name = value`.
    Value(&'e Expr),
    /// `for name in iter:`: each item of `iter`.
    Item(&'e Expr),
}

/// Names bound by exactly one plain `name = value` statement (or one
/// `for name in ...:` loop) and bound in no other way (nested scopes
/// included), with that value; other bound names map to `None`. A name
/// declared `global` is never an alias: a value stored in it outlives the
/// call.
fn single_assignments(body: &[Stmt]) -> Vec<(String, Option<Assigned<'_>>)> {
    #[derive(Default)]
    struct Stores<'e> {
        count: FxHashMap<String, (usize, Option<Assigned<'e>>)>,
        /// Directly in a class body: names bound there are attributes of
        /// the class, which outlive the call.
        class_body: bool,
    }
    impl<'e> Visitor<'e> for Stores<'e> {
        fn visit_stmt(&mut self, stmt: &'e Stmt) {
            match stmt {
                Stmt::ClassDef(c) => {
                    self.count.entry(c.name.as_str().to_owned()).or_default().0 += 2;
                    let outer = std::mem::replace(&mut self.class_body, true);
                    walk_stmt(self, stmt);
                    self.class_body = outer;
                    return;
                }
                Stmt::FunctionDef(_) => {
                    let outer = std::mem::replace(&mut self.class_body, false);
                    if let Stmt::FunctionDef(f) = stmt {
                        self.count.entry(f.name.as_str().to_owned()).or_default().0 += 2;
                        for n in [&f.args.vararg, &f.args.kwarg].into_iter().flatten() {
                            self.count.entry(n.as_str().to_owned()).or_default().0 += 2;
                        }
                    }
                    walk_stmt(self, stmt);
                    self.class_body = outer;
                    return;
                }
                Stmt::Assign(s)
                    if !self.class_body && matches!(s.targets.as_slice(), [Expr::Name(_)]) =>
                {
                    let [Expr::Name(n)] = s.targets.as_slice() else {
                        unreachable!()
                    };
                    let e = self.count.entry(n.id.to_string()).or_default();
                    e.0 += 1;
                    e.1 = Some(Assigned::Value(&s.value));
                    self.visit_expr(&s.value);
                    return;
                }
                Stmt::For(f) if !self.class_body && matches!(&*f.target, Expr::Name(_)) => {
                    let Expr::Name(n) = &*f.target else {
                        unreachable!()
                    };
                    let e = self.count.entry(n.id.to_string()).or_default();
                    e.0 += 1;
                    e.1 = Some(Assigned::Item(&f.iter));
                    self.visit_expr(&f.iter);
                    for stmt in f.body.iter().chain(&f.orelse) {
                        self.visit_stmt(stmt);
                    }
                    return;
                }
                // Other kinds of bindings: never an alias.
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    for name in import_names(std::slice::from_ref(stmt)).into_keys() {
                        self.count.entry(name).or_default().0 += 2;
                    }
                }
                Stmt::Global(g) => {
                    for n in &g.names {
                        self.count.entry(n.as_str().to_owned()).or_default().0 += 2;
                    }
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'e Expr) {
            if let Expr::Name(n) = expr
                && n.ctx != ExprContext::Load
            {
                self.count.entry(n.id.to_string()).or_default().0 += 2;
            }
            if let Expr::Lambda(l) = expr {
                for n in [&l.args.vararg, &l.args.kwarg].into_iter().flatten() {
                    self.count.entry(n.as_str().to_owned()).or_default().0 += 2;
                }
            }
            walk_expr(self, expr);
        }
    }
    let mut stores = Stores::default();
    for stmt in body {
        stores.visit_stmt(stmt);
    }
    let mut out: Vec<(String, Option<Assigned<'_>>)> = stores
        .count
        .into_iter()
        .map(|(name, (count, value))| (name, if count == 1 { value } else { None }))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Names bound by imports in `body` (nested scopes included), with the
/// dotted names they stand for (`import a.b` binds `a` to `a`;
/// `import a.b as c` binds `c` to `a.b`; `from a import b as c` binds `c`
/// to `a.b`). Relative imports stand for nothing known.
fn import_names(body: &[Stmt]) -> FxHashMap<String, Vec<String>> {
    #[derive(Default)]
    struct Imports(FxHashMap<String, Vec<String>>);
    impl<'e> Visitor<'e> for Imports {
        fn visit_stmt(&mut self, stmt: &'e Stmt) {
            match stmt {
                Stmt::Import(i) => {
                    for alias in &i.names {
                        let full = alias.name.as_str();
                        let (name, target) = match &alias.asname {
                            Some(a) => (a.as_str(), full),
                            None => {
                                let head = full.split('.').next().unwrap_or(full);
                                (head, head)
                            }
                        };
                        self.0
                            .entry(name.to_owned())
                            .or_default()
                            .push(target.to_owned());
                    }
                }
                Stmt::ImportFrom(i) => {
                    for alias in &i.names {
                        if alias.name.as_str() == "*" {
                            continue;
                        }
                        let name = alias
                            .asname
                            .as_ref()
                            .map_or_else(|| alias.name.as_str(), |a| a.as_str());
                        let target = match &i.module {
                            Some(m) if i.level == 0 => {
                                format!("{}.{}", m.as_str(), alias.name.as_str())
                            }
                            // Relative to an unknown package: may be anything.
                            _ => format!(".{}", alias.name.as_str()),
                        };
                        self.0.entry(name.to_owned()).or_default().push(target);
                    }
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
    }
    let mut v = Imports::default();
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.0
}

/// Every name bound in `body`, nested scopes included: assignment and loop
/// targets, parameters, imports, `def`, `class`, `global`.
fn bound_names(body: &[Stmt]) -> FxHashSet<String> {
    #[derive(Default)]
    struct Bound(FxHashSet<String>);
    impl<'e> Visitor<'e> for Bound {
        fn visit_stmt(&mut self, stmt: &'e Stmt) {
            match stmt {
                Stmt::FunctionDef(f) => {
                    self.0.insert(f.name.as_str().to_owned());
                    if let Some(v) = &f.args.vararg {
                        self.0.insert(v.as_str().to_owned());
                    }
                    if let Some(k) = &f.args.kwarg {
                        self.0.insert(k.as_str().to_owned());
                    }
                }
                Stmt::ClassDef(c) => {
                    self.0.insert(c.name.as_str().to_owned());
                }
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    self.0
                        .extend(import_names(std::slice::from_ref(stmt)).into_keys());
                }
                Stmt::Global(g) => {
                    for n in &g.names {
                        self.0.insert(n.as_str().to_owned());
                    }
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'e Expr) {
            if let Expr::Name(n) = expr
                && n.ctx != ExprContext::Load
            {
                self.0.insert(n.id.to_string());
            }
            if let Expr::Lambda(l) = expr {
                if let Some(v) = &l.args.vararg {
                    self.0.insert(v.as_str().to_owned());
                }
                if let Some(k) = &l.args.kwarg {
                    self.0.insert(k.as_str().to_owned());
                }
            }
            walk_expr(self, expr);
        }
    }
    let mut bound = Bound::default();
    for stmt in body {
        bound.visit_stmt(stmt);
    }
    bound.0
}

/// Every dotted name `body` (all scopes) calls or uses as a value that may
/// be a library name, as [`Analysis::check_callee`] sees them: rules
/// whose fixes depend on names read by name check these through the
/// project index (a library variable may hold `eval` or `locals`).
pub fn library_names_used(model: &SemanticModel<'_>, body: &[Stmt]) -> Vec<String> {
    struct Used<'n> {
        names: &'n Names,
        out: Vec<String>,
    }
    impl<'e> Visitor<'e> for Used<'_> {
        fn visit_expr(&mut self, expr: &'e Expr) {
            if matches!(expr, Expr::Name(_) | Expr::Attribute(_)) && dotted_name(expr).is_some() {
                for n in self.names.library_names(expr, &|_| false) {
                    if !self.out.contains(&n) {
                        self.out.push(n);
                    }
                }
                // The chain's prefixes are not used on their own.
                return;
            }
            walk_expr(self, expr);
        }
    }
    let names = Names::new(Some(model), body, []);
    let mut v = Used {
        names: &names,
        out: Vec::new(),
    };
    for stmt in body {
        v.visit_stmt(stmt);
    }
    v.out
}

// ---- library functions ----

/// What `f` does with each of its positional parameters, when they hold a
/// Perspective object (see [`ParamUse`]). `model` is the semantic model of
/// the module `f` belongs to, used to resolve the names `f` uses through
/// their scope.
pub fn param_uses(model: &SemanticModel<'_>, f: &StmtFunctionDef) -> Vec<ParamUse> {
    let names: Vec<Option<String>> = f
        .args
        .args
        .iter()
        .map(|a| match a {
            Expr::Name(n) => Some(n.id.to_string()),
            _ => None,
        })
        .collect();
    // Every name the arguments bind (`*args`, `**kwargs` and tuple
    // parameters included): such a name is never a library name.
    struct ArgNames(Vec<String>);
    impl<'e> Visitor<'e> for ArgNames {
        fn visit_expr(&mut self, expr: &'e Expr) {
            if let Expr::Name(n) = expr {
                self.0.push(n.id.to_string());
            }
            walk_expr(self, expr);
        }
    }
    let mut arguments = ArgNames(Vec::new());
    for a in &f.args.args {
        arguments.visit_expr(a);
    }
    arguments.0.extend(
        [&f.args.vararg, &f.args.kwarg]
            .into_iter()
            .flatten()
            .map(|n| n.as_str().to_owned()),
    );
    let mut w = ParamWalker {
        params: FxHashMap::default(),
        aliases: FxHashMap::default(),
        names: Names::new(Some(model), &f.body, arguments.0),
        uses: names.iter().map(|_| ParamUse::Paths(Vec::new())).collect(),
    };
    let rebound = bound_names(&f.body);
    for (i, name) in names.iter().enumerate() {
        match name {
            // A parameter bound again in the body (or in a nested scope,
            // which may shadow it) is not followed.
            Some(n) if !rebound.contains(n) => {
                w.params.insert(n.clone(), i);
            }
            _ => w.uses[i] = ParamUse::Dynamic,
        }
    }
    if w.params.is_empty() {
        return w.uses;
    }
    if reads_frames(&f.body).is_some() {
        w.dynamic_all();
        return w.uses;
    }
    // Defaults are evaluated where the function is defined.
    w.collect_aliases(&f.body);
    for stmt in &f.body {
        w.visit_stmt(stmt);
    }
    w.uses
}

/// A value derived from a parameter: the parameter and the steps.
type Sym = (usize, Vec<PathStep>);

struct ParamWalker {
    params: FxHashMap<String, usize>,
    /// Names assigned exactly once from a parameter path.
    aliases: FxHashMap<String, Option<Sym>>,
    names: Names,
    uses: Vec<ParamUse>,
}

impl ParamWalker {
    fn collect_aliases(&mut self, body: &[Stmt]) {
        let stores = single_assignments(body);
        for _ in 0..3 {
            for (name, value) in &stores {
                let v = match value {
                    Some(Assigned::Value(value)) => self.peek(value),
                    _ => None,
                };
                self.aliases.insert(name.clone(), v);
            }
        }
    }

    fn peek(&self, e: &Expr) -> Option<Sym> {
        match e {
            Expr::Name(n) if n.ctx == ExprContext::Load => self.name_value(&n.id),
            Expr::Attribute(a) => {
                let (p, mut steps) = self.peek(&a.value)?;
                steps.push(PathStep::Attr(a.attr.as_str().to_owned()));
                Some((p, steps))
            }
            Expr::Call(c) => {
                let Expr::Attribute(a) = &*c.func else {
                    return None;
                };
                let (p, mut steps) = self.peek(&a.value)?;
                steps.push(PathStep::Method {
                    name: a.attr.as_str().to_owned(),
                    literal: first_literal(&c.args),
                });
                Some((p, steps))
            }
            Expr::Subscript(s) => {
                let (p, mut steps) = self.peek(&s.value)?;
                steps.push(PathStep::Index(literal_key(&s.slice)));
                Some((p, steps))
            }
            _ => None,
        }
    }

    fn name_value(&self, name: &str) -> Option<Sym> {
        if let Some(p) = self.params.get(name) {
            return Some((*p, Vec::new()));
        }
        self.aliases.get(name).cloned().flatten()
    }

    fn is_followed_name(&self, name: &str) -> bool {
        self.params.contains_key(name) || self.aliases.get(name).is_some_and(Option::is_some)
    }

    fn record(&mut self, (p, steps): Sym, end: PathEnd) {
        if let ParamUse::Paths(paths) = &mut self.uses[p] {
            let path = ParamPath { steps, end };
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }

    fn dynamic_all(&mut self) {
        self.uses.fill(ParamUse::Dynamic);
    }

    /// `e` is called or used as a value: every parameter is live, so a
    /// frame builtin under a library name could read them all.
    fn check_callee(&mut self, e: &Expr) {
        let followed = |n: &str| self.is_followed_name(n);
        let callees = self.names.library_names(e, &followed);
        for callee in callees {
            for p in 0..self.uses.len() {
                let end = PathEnd::Passed {
                    callee: callee.clone(),
                    slot: ArgSlot::Callee,
                };
                self.record((p, Vec::new()), end);
            }
        }
    }

    /// Like [`Analysis::walk`], over parameter paths.
    fn walk(&mut self, e: &Expr, role: Role, aug: bool) -> Option<Sym> {
        let result = match e {
            Expr::Name(n) => match n.ctx {
                ExprContext::Load | ExprContext::AugLoad => self.name_value(&n.id),
                _ => None,
            },
            Expr::Attribute(a) => match self.walk(&a.value, Role::Base, false) {
                Some((p, mut steps)) => {
                    steps.push(PathStep::Attr(a.attr.as_str().to_owned()));
                    self.place((p, steps), aug, a.ctx)
                }
                None => None,
            },
            Expr::Subscript(s) => {
                let base = self.walk(&s.value, Role::Base, false);
                if base.is_none() {
                    self.check_callee(&s.value);
                }
                let mut children = Children::default();
                children.visit_slice(&s.slice);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                match base {
                    Some((p, mut steps)) => {
                        steps.push(PathStep::Index(literal_key(&s.slice)));
                        self.place((p, steps), aug, s.ctx)
                    }
                    None => None,
                }
            }
            Expr::Call(c) => self.walk_call(c),
            Expr::Compare(c) if is_literal_in(c) => {
                if let Some(sym) = self.walk(&c.comparators[0], Role::Base, false) {
                    let Expr::Str(s) = &*c.left else {
                        unreachable!()
                    };
                    let name = s.s.as_str().unwrap_or_default().to_owned();
                    self.record(sym, PathEnd::Contains(name));
                }
                None
            }
            // Identity tests and truth values read no property.
            Expr::Compare(c) if c.ops.iter().all(|o| matches!(o, CmpOp::Is | CmpOp::IsNot)) => {
                self.test(&c.left);
                for comparator in &c.comparators {
                    self.test(comparator);
                }
                None
            }
            Expr::UnaryOp(u) if u.op == UnaryOp::Not => {
                self.test(&u.operand);
                None
            }
            Expr::Lambda(_) | Expr::GeneratorExp(_) | Expr::SetComp(_) | Expr::DictComp(_)
                if self.mentions_param(e) =>
            {
                // Nested scopes are not followed.
                self.dynamic_all();
                None
            }
            other => {
                let mut children = Children::default();
                walk_expr(&mut children, other);
                for child in children.0 {
                    self.walk(child, Role::Value, false);
                }
                None
            }
        };
        match (role, result) {
            (Role::Value, Some(sym)) => {
                self.record(sym, PathEnd::Load);
                None
            }
            (Role::Value, None) => {
                if matches!(e, Expr::Name(_) | Expr::Attribute(_)) {
                    self.check_callee(e);
                }
                None
            }
            (_, result) => result,
        }
    }

    /// A path in context `ctx`: loads are returned to the caller, stores
    /// and updates recorded.
    fn place(&mut self, sym: Sym, aug: bool, ctx: ExprContext) -> Option<Sym> {
        match (aug, ctx) {
            (true, _) => {
                self.record(sym, PathEnd::Update);
                None
            }
            (false, ExprContext::Load) => Some(sym),
            _ => {
                self.record(sym, PathEnd::Store);
                None
            }
        }
    }

    fn walk_call(&mut self, c: &ExprCall) -> Option<Sym> {
        let mut value = None;
        let mut callee = None;
        match &*c.func {
            Expr::Attribute(a) => {
                if let Some((p, mut steps)) = self.walk(&a.value, Role::Base, false) {
                    steps.push(PathStep::Method {
                        name: a.attr.as_str().to_owned(),
                        literal: first_literal(&c.args),
                    });
                    value = Some((p, steps));
                } else {
                    self.check_callee(&c.func);
                    callee = self.library_callee(&c.func);
                }
            }
            Expr::Name(n) => match self.name_value(&n.id) {
                // A followed value called.
                Some(sym) => self.record(sym, PathEnd::Load),
                None => {
                    self.check_callee(&c.func);
                    callee = self.library_callee(&c.func);
                }
            },
            other => {
                if calls_unnamed_value(other) {
                    self.dynamic_all();
                }
                self.walk(other, Role::Value, false);
            }
        }
        for (i, arg) in c.args.iter().enumerate() {
            match &callee {
                Some(callee) => {
                    if let Some(sym) = self.walk(arg, Role::Base, false) {
                        let slot = ArgSlot::Position(i);
                        let callee = callee.clone();
                        self.record(sym, PathEnd::Passed { callee, slot });
                    }
                }
                None => {
                    self.walk(arg, Role::Value, false);
                }
            }
        }
        for k in &c.keywords {
            match &callee {
                Some(callee) => {
                    if let Some(sym) = self.walk(&k.value, Role::Base, false) {
                        let slot = ArgSlot::Keyword(k.arg.as_str().to_owned());
                        let callee = callee.clone();
                        self.record(sym, PathEnd::Passed { callee, slot });
                    }
                }
                None => {
                    self.walk(&k.value, Role::Value, false);
                }
            }
        }
        if let Some(s) = &c.starargs {
            self.walk(s, Role::Value, false);
        }
        if let Some(k) = &c.kwargs {
            self.walk(k, Role::Value, false);
        }
        value
    }

    /// The dotted name a call calls, when it may be a library function or
    /// a top-level function of the module that followed values are passed
    /// to.
    fn library_callee(&self, func: &Expr) -> Option<String> {
        let followed = |n: &str| self.is_followed_name(n);
        let mut names = self.names.library_names(func, &followed);
        (names.len() == 1).then(|| names.remove(0))
    }

    /// An expression whose truth value or identity is all that is used:
    /// the parameter itself is not read, anything derived from it is.
    fn test(&mut self, e: &Expr) {
        if let Expr::BoolOp(b) = e {
            for value in &b.values {
                self.test(value);
            }
            return;
        }
        match self.walk(e, Role::Base, false) {
            Some(sym) if !sym.1.is_empty() => self.record(sym, PathEnd::Test),
            Some(_) => {}
            None => {
                if matches!(e, Expr::Name(_) | Expr::Attribute(_)) {
                    self.check_callee(e);
                }
            }
        }
    }

    /// Whether `e` reads a parameter or an alias of one.
    fn mentions_param(&self, e: &Expr) -> bool {
        let mut m = MentionsStmt(self, false);
        m.visit_expr(e);
        m.1
    }
}

impl<'e> Visitor<'e> for ParamWalker {
    fn visit_stmt(&mut self, stmt: &'e Stmt) {
        match stmt {
            Stmt::Assign(s) => {
                if let [Expr::Name(n)] = s.targets.as_slice()
                    && self.aliases.get(&*n.id).is_some_and(Option::is_some)
                {
                    // An alias definition: the steps are evaluated here
                    // (`x = p.custom.a` reads `a` even if `x` is never
                    // used), the value is followed through the alias.
                    if let Some(sym) = self.walk(&s.value, Role::Base, false) {
                        self.record(sym, PathEnd::Alias);
                    }
                    return;
                }
                for t in &s.targets {
                    self.walk(t, Role::Value, false);
                }
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::AugAssign(s) => {
                self.walk(&s.target, Role::Value, true);
                self.walk(&s.value, Role::Value, false);
            }
            Stmt::Exec(_) => self.dynamic_all(),
            Stmt::If(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::While(s) => {
                self.test(&s.test);
                for stmt in s.body.iter().chain(&s.orelse) {
                    self.visit_stmt(stmt);
                }
            }
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {
                let mut m = MentionsStmt(self, false);
                m.visit_stmt(stmt);
                if m.1 {
                    self.dynamic_all();
                } else {
                    // The nested code runs while the parameters are live:
                    // what it calls is checked, its values are not
                    // followed.
                    walk_stmt(self, stmt);
                }
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'e Expr) {
        self.walk(expr, Role::Value, false);
    }
}

/// Whether a nested `def`, `class` or expression reads a parameter
/// (closures are not followed).
struct MentionsStmt<'w>(&'w ParamWalker, bool);

impl<'e> Visitor<'e> for MentionsStmt<'_> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        if let Expr::Name(n) = expr
            && self.0.is_followed_name(&n.id)
        {
            self.1 = true;
        }
        walk_expr(self, expr);
    }
}

/// A component lookup with a literal name that finds nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnresolvedLookup {
    /// The name argument in the script source.
    pub range: TextRange,
    pub message: String,
    /// The lookup call in the script source.
    pub call: TextRange,
    /// `getSibling` or `getChild`.
    pub method: &'static str,
    /// The literal name argument.
    pub name: String,
}

/// Literal `getSibling`/`getChild` lookups of a view script whose base
/// component (or the view) is known and that name no component. Lookups
/// below components that cannot hold children in the view (embedded views,
/// components without a `children` array that are not containers) are
/// not reported.
pub fn unresolved_lookups(unit: &ScriptUnit<'_, '_>, view: &View<'_>) -> Vec<UnresolvedLookup> {
    let this = match unit.script.owner {
        Owner::View => Val::Object(AccessTarget::View),
        Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
        _ => return Vec::new(),
    };
    if !unit.parsed.is_valid() {
        return Vec::new();
    }
    let mut sink = Vec::new();
    let a = Analysis::new(None, Some(view), this, &unit.parsed.module.body, &mut sink);
    let mut out = Vec::new();
    crate::rules::script::walk_exprs(&unit.parsed.module.body, &mut |e| {
        let Expr::Call(c) = e else { return };
        let Expr::Attribute(attr) = &*c.func else {
            return;
        };
        let method = attr.attr.as_str();
        if method != "getSibling" && method != "getChild" {
            return;
        }
        let Some(Expr::Str(lit)) = c.args.first() else {
            return;
        };
        let Some(name) = lit.s.as_str() else { return };
        if c.args.len() != 1 || !c.keywords.is_empty() {
            return;
        }
        let Some(Val::Object(base)) = a.peek(&attr.value) else {
            return;
        };
        if let Some(message) = a.lookup_problem(base, method, name) {
            out.push(UnresolvedLookup {
                range: lit.range,
                message,
                call: c.range,
                method: if method == "getSibling" {
                    "getSibling"
                } else {
                    "getChild"
                },
                name: name.to_owned(),
            });
        }
    });
    out
}

/// What every `getSibling`/`getChild` call of a view script resolves to,
/// in source order, for the safety net's resolution map
/// (`crate::safety`): the receiver and the result as the access analysis
/// follows them (as `SCR305` does), `None` where it does not follow them.
/// Empty when the script does not parse.
pub fn component_lookups(
    owner: Owner,
    parsed: &iglint_python::Parsed,
    view: &View<'_>,
) -> Vec<(Option<AccessTarget>, Option<AccessTarget>)> {
    let this = match owner {
        Owner::View => Val::Object(AccessTarget::View),
        Owner::Component(c) => Val::Object(AccessTarget::Component(c)),
        _ => return Vec::new(),
    };
    if !parsed.is_valid() {
        return Vec::new();
    }
    let mut sink = Vec::new();
    let a = Analysis::new(None, Some(view), this, &parsed.module.body, &mut sink);
    let target = |v: Option<Val>| match v {
        Some(Val::Object(t)) => Some(t),
        _ => None,
    };
    let mut out = Vec::new();
    crate::rules::script::walk_exprs(&parsed.module.body, &mut |e| {
        let Expr::Call(c) = e else { return };
        let Expr::Attribute(attr) = &*c.func else {
            return;
        };
        if matches!(attr.attr.as_str(), "getSibling" | "getChild") {
            out.push((target(a.peek(&attr.value)), target(a.peek(e))));
        }
    });
    out
}

impl Analysis<'_, '_, '_> {
    /// Whether components may be looked up below `c` (it is a container).
    fn holds_children(&self, c: ComponentId) -> bool {
        let Some(view) = self.view else { return false };
        let component = view.component(c);
        component.value.get("children").is_some()
            || component
                .type_id
                .is_some_and(|t| t.starts_with("ia.container."))
    }

    /// Why `base.method(name)` finds nothing, if it certainly does not.
    fn lookup_problem(&self, base: AccessTarget, method: &str, name: &str) -> Option<String> {
        let view = self.view?;
        let (start, path, describe) = match (base, method) {
            (AccessTarget::Component(c), "getSibling") => {
                let parent = view.component(c).parent?;
                if name.contains('/') {
                    return None;
                }
                (Some(parent), name, "sibling")
            }
            (AccessTarget::Component(c), "getChild") => (Some(c), name, "child"),
            (AccessTarget::View, "getChild") => (None, name, "view path"),
            _ => return None,
        };
        let mut segments = path.split('/').filter(|s| !s.is_empty()).peekable();
        segments.peek()?;
        let mut at = match start {
            Some(c) => c,
            None => {
                let root = view.root_component()?;
                // Paths start with the root container's name; other forms
                // are not certain to fail.
                if segments.next()? != root.name {
                    return None;
                }
                root.id
            }
        };
        for segment in segments {
            if !self.holds_children(at) {
                return None;
            }
            match view.children(at).find(|c| c.name == segment) {
                Some(c) => at = c.id,
                None => {
                    let owner = view.component(at);
                    let hint = view
                        .children(at)
                        .find(|c| c.name.eq_ignore_ascii_case(segment))
                        .map(|c| format!(" (names are case-sensitive; did you mean `{}`?)", c.name))
                        .unwrap_or_default();
                    return Some(match describe {
                        "sibling" => format!(
                            "No sibling component named `{segment}` (in `{}`){hint}",
                            owner.path
                        ),
                        _ => format!(
                            "No child component named `{segment}` in `{}`{hint}",
                            owner.path
                        ),
                    });
                }
            }
        }
        None
    }
}

fn e_range(e: &Expr) -> TextRange {
    use iglint_text::Ranged;
    e.range()
}

/// Collects the direct child expressions of a node.
#[derive(Default)]
struct Children<'e>(Vec<&'e Expr>);

impl<'e> Visitor<'e> for Children<'e> {
    fn visit_expr(&mut self, expr: &'e Expr) {
        self.0.push(expr);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use iglint_project::view::build_view;

    const VIEW: &str = r#"{"custom": {"a": 1}, "params": {"p": 1},
      "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [
        {"meta": {"name": "A"}, "type": "ia.display.label", "custom": {"x": 1}},
        {"meta": {"name": "B"}, "type": "ia.input.button", "custom": {"y": 1},
         "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": ""}}}}}
      ]}}"#;

    /// Reviewer round 10, O2: the measured name members (`name`,
    /// `getName()`) of a component read its name, however the component
    /// is reached; the view's own name is not a component's.
    #[test]
    fn name_members_read_the_component_name() {
        for (body, expected) in [
            ("\tx = self.name", vec!["B.meta.name r"]),
            ("\tx = self.getName()", vec!["B.meta.name r"]),
            ("\tx = self.getSibling('A').name", vec!["A.meta.name r"]),
            (
                "\tx = self.parent.getChildren()[0].name",
                vec!["*.meta.name r"],
            ),
            (
                "\tfor c in self.parent.children:\n\t\tx = c.getName()",
                vec!["*.meta.name r"],
            ),
            ("\tx = self.view.name", vec![]),
        ] {
            assert_eq!(accesses_and_checks(body), expected, "{body}");
        }
    }

    /// Accesses of `body` run as the button's action script; callee
    /// checks are shown once as `check <callee>`.
    fn accesses_and_checks(body: &str) -> Vec<String> {
        let mut json: serde_json::Value = serde_json::from_str(VIEW).unwrap();
        json["root"]["children"][1]["events"]["component"]["onActionPerformed"]["config"]["script"] =
            serde_json::Value::String(body.to_owned());
        let text = serde_json::to_string(&json).unwrap();
        let root = iglint_json::parse(&text).unwrap();
        let view = build_view(&root);
        let script = &view.scripts[0];
        let parsed = crate::script_facts::parse_script(script);
        let unit = ScriptUnit::new(iglint_project::FileId(0), script, &parsed, 0);
        let mut out = Vec::new();
        AstScriptFacts.property_accesses(&unit, &view, &mut out);
        let mut shown = Vec::new();
        for a in &out {
            if let Some(call) = a.via.as_ref().filter(|c| c.is_callee_check()) {
                let s = format!("check {}", call.callee);
                if !shown.contains(&s) {
                    shown.push(s);
                }
                continue;
            }
            let t = match a.target {
                AccessTarget::View => "view".to_owned(),
                AccessTarget::Component(c) => view.component(c).name.to_owned(),
                AccessTarget::AnyComponent => "*".to_owned(),
            };
            shown.push(format!(
                "{t}.{}.{}{}{}",
                a.category.name(),
                a.name.as_deref().unwrap_or("*"),
                if a.read { " r" } else { "" },
                if a.write { " w" } else { "" },
            ));
        }
        shown
    }

    /// [`accesses_and_checks`] without the callee checks.
    fn accesses(body: &str) -> Vec<String> {
        accesses_and_checks(body)
            .into_iter()
            .filter(|s| !s.starts_with("check "))
            .collect()
    }

    const EVERYTHING: [&str; 3] = ["view.custom.* r w", "view.params.* r w", "*.custom.* r w"];

    /// Every member of Perspective's script objects measured in the image
    /// (`iglint_python_semantic::perspective`) is classified: on a page or
    /// session, members that hand out views, pages or components reach
    /// other views, the session is a host again, and everything else is
    /// opaque; on a component or view, every member is one of the
    /// understood forms and none reaches another view; the property tree
    /// names and the model delegates' methods reach nothing. A regenerated
    /// table with a member this does not cover fails here.
    #[test]
    fn every_measured_perspective_member_is_classified() {
        use iglint_python_semantic::perspective;
        // `object` members checked by hand: JSON data of the session and
        // project info, and the result of the callable `invokeOnQueue`
        // runs.
        const OBJECT_HOST: &[&str] = &[
            "getInfo",
            "getProjectInfo",
            "info",
            "invokeOnQueue",
            "projectInfo",
        ];
        let mut out = Vec::new();
        let a = Analysis::with_roots(None, None, Vec::new(), &[], &mut out);
        for (wrapper, name, kind) in perspective::members() {
            let step = match *wrapper {
                "page" | "session" => host_member(name),
                _ => {
                    let base = if *wrapper == "view" {
                        Val::Object(AccessTarget::View)
                    } else {
                        Val::Object(AccessTarget::AnyComponent)
                    };
                    let attribute = a.attribute(base, name);
                    let method = a.method(base, name, Some("x".to_owned()));
                    // The spelling that is understood.
                    if attribute == Step::Everything {
                        method
                    } else {
                        attribute
                    }
                }
            };
            let host = matches!(*wrapper, "page" | "session");
            let ok = match (host, *kind) {
                (true, "element" | "host") if HOST_SESSION_MEMBERS.contains(name) => {
                    step == Step::Follow(Val::Host)
                }
                (true, "element" | "host") => step == Step::Cross,
                (true, "object") => OBJECT_HOST.contains(name) && step == Step::Opaque,
                (true, _) => step == Step::Opaque,
                (false, "element") => matches!(step, Step::Follow(Val::Object(_))),
                (false, "host") => step == Step::Follow(Val::Host),
                // `getPropertyTreeOf`: this object's own trees (a
                // `PropertyTree` reaches no owner), read as everything.
                (false, "tree") => step == Step::Everything,
                // The measured name members read the component's name.
                (false, _) if *wrapper == "component" && is_component_name_member(name) => {
                    step == Step::Name(AccessTarget::AnyComponent)
                }
                (false, _) => step == Step::Opaque,
            };
            assert!(ok, "{wrapper}.{name} ({kind}): {step:?}");
        }
        assert_eq!(
            perspective::property_types(),
            ["custom", "meta", "params", "position", "props"]
        );
        for (name, kind) in perspective::delegate_members() {
            // Map data (`PyDictionary`) or values: no view.
            assert!(
                *kind == "value"
                    || (*kind == "object"
                        && [
                            "bounds",
                            "center",
                            "getBounds",
                            "getCenter",
                            "getSize",
                            "size"
                        ]
                        .contains(name)),
                "delegate member {name} ({kind})"
            );
        }
    }

    fn is_everything(found: &[String]) -> bool {
        EVERYTHING.iter().all(|e| found.iter().any(|f| f == e))
    }

    #[test]
    fn direct_accesses() {
        assert_eq!(
            accesses(
                "\tx = self.view.custom.a\n\tself.view.params.p = self.custom.y\n\tself.getSibling('A').custom['x'] += 1"
            ),
            vec![
                "view.custom.a r",
                "view.params.p w",
                "B.custom.y r",
                "A.custom.x r w"
            ]
        );
    }

    #[test]
    fn navigation() {
        assert_eq!(
            accesses(
                "\tself.parent.getChild('A').custom.x\n\tself.view.getChild('root/A').custom.q\n\tself.view.rootContainer.custom.r\n\tself.getSibling('Missing').custom.z\n\tself.view.getRootContainer().getChild('A').custom.s"
            ),
            vec![
                "A.custom.x r",
                "A.custom.q r",
                "root.custom.r r",
                "A.custom.s r"
            ]
        );
        // A loop over children binds any component.
        assert_eq!(
            accesses(
                "\tfor c in self.parent.children:\n\t\tc.custom.x = 1\n\tn = event.name\n\tself.getSibling(n).custom.w"
            ),
            vec!["*.custom.x w", "*.custom.w r"]
        );
        // A loop variable bound elsewhere too loses track of them.
        assert_eq!(
            accesses(
                "\tfor c in self.parent.children:\n\t\tc.custom.x = 1\n\tc = 1\n\tn = event.name\n\tself.getSibling(n).custom.w"
            ),
            vec![
                "view.custom.* r w",
                "view.params.* r w",
                "*.custom.* r w",
                "*.custom.w r"
            ]
        );
    }

    #[test]
    fn aliases() {
        assert_eq!(
            accesses("\tv = self.view\n\tc = v.custom\n\tc.a = v.params.p"),
            vec!["view.custom.a w", "view.params.p r"]
        );
        // An alias that is never used reads nothing; a property read into
        // a local is a read.
        assert!(accesses("\tv = self.view").is_empty());
        assert_eq!(accesses("\tx = self.custom.y"), vec!["B.custom.y r"]);
    }

    #[test]
    fn dynamic_uses_are_conservative() {
        assert_eq!(
            accesses("\tk = 'a'\n\tself.view.custom[k]"),
            vec!["view.custom.* r"]
        );
        assert_eq!(
            accesses("\tdata = dict(self.custom)"),
            vec!["B.custom.* r w"]
        );
        assert_eq!(
            accesses("\tfor k in self.view.custom.keys(): pass"),
            vec!["view.custom.keys r", "view.custom.* r"]
        );
        let escaped = accesses("\tproject.util.handle(self)");
        assert!(is_everything(&escaped), "{escaped:?}");
        let reassigned = accesses("\tv = self.view\n\tv = event.source\n\tv.custom.a");
        assert!(is_everything(&reassigned), "{reassigned:?}");
        assert!(is_everything(&accesses("\tx = locals()")));
    }

    /// Methods of the property wrapper: understood calls, and the methods
    /// used as values, which read and write the whole container.
    #[test]
    fn container_methods() {
        assert_eq!(
            accesses("\tself.custom.get('y')\n\tself.view.custom.get('a', 0)"),
            vec![
                "B.custom.get r",
                "B.custom.y r",
                "view.custom.get r",
                "view.custom.a r"
            ]
        );
        assert_eq!(
            accesses("\tif 'y' in self.custom: pass\n\tself.custom.has_key('z')"),
            vec!["B.custom.y r", "B.custom.has_key r", "B.custom.z r"]
        );
        assert_eq!(
            accesses("\tself.custom.pop('y')"),
            vec!["B.custom.pop r", "B.custom.y r w"]
        );
        for script in [
            "\tmap(self.custom.get, ['y'])",
            "\tg = self.custom.get\n\tg('y')",
            "\tsystem.util.invokeAsynchronous(self.custom.keys)",
            "\tself.custom.update({'y': 1})",
            "\tself.custom.get(name)",
            "\tc = self.custom\n\tm = c.get\n\tm('y')",
        ] {
            let found = accesses(script);
            assert!(
                found.iter().any(|f| f == "B.custom.* r w"),
                "{script}: {found:?}"
            );
        }
    }

    /// Attributes and methods of components and views that are not in the
    /// enumerated lists read and write everything; the listed harmless ones
    /// nothing.
    #[test]
    fn object_uses_are_enumerated() {
        assert!(
            accesses(
                "\tself.props.text = self.meta.name\n\tself.focus()\n\tself.view.refreshBinding('custom.a')\n\tself.session.props.id\n\tself.page.props.path\n\tself.session.custom.user"
            )
            .is_empty()
        );
        for script in [
            "\tself.getPropertyTreeOf(t)",
            "\tf = self.getSibling",
            "\tf = self.getSibling\n\tf('A').custom.x",
            "\tself.page.view.custom.a",
            "\tself.session.getPage('p')",
            "\tlib.util.log(self.session)",
            "\tx = self.zzz",
            "\tself['custom']",
            "\tself.view.getChild",
        ] {
            let found = accesses(script);
            assert!(is_everything(&found), "{script}: {found:?}");
        }
    }

    /// Frame builtins reached in any way read everything.
    #[test]
    fn frame_routes_read_everything() {
        for script in [
            "\tx = locals()",
            "\te = eval\n\te('self.custom.y')",
            "\timport __builtin__\n\t__builtin__.eval('self.custom.y')",
            "\tfrom __builtin__ import locals as capture\n\tcapture()",
            "\tx = vars()",
            "\timport sys\n\tsys._getframe(0).f_locals",
            "\timport inspect\n\tinspect.stack()",
            "\tx = sys.modules['__builtin__']",
            "\texec 'print self.custom.y'",
        ] {
            let found = accesses(script);
            assert!(is_everything(&found), "{script}: {found:?}");
        }
    }

    /// Names that may be library names are checked when they are called
    /// or used as values.
    #[test]
    fn callee_checks() {
        assert_eq!(
            accesses_and_checks(
                "\tlib.util.ev('x')\n\tx = lib.util.CONST\n\tfrom lib.other import f as g\n\tg()\n\timport json\n\tjson.dumps(1)\n\tlen([])\n\tsystem.perspective.print(1)\n\tevent.source"
            ),
            vec![
                "check lib.util.ev",
                "check lib.util.CONST",
                "check lib.other.f",
                "check json.dumps"
            ]
        );
    }

    /// Calling a value that does not come from a name (a call result, an
    /// item) may call a frame builtin; `getattr(o, 'm')()` is a method.
    #[test]
    fn calls_of_unnamed_values_read_everything() {
        for script in [
            "\tfactory()('x')",
            "\thandlers[k]('x')",
            "\tgetattr(o, n)('x')",
        ] {
            assert!(is_everything(&accesses(script)), "{script}");
        }
        assert!(accesses("\tgetattr(o, 'm')('x')").is_empty());
    }

    #[test]
    fn unrelated_code_has_no_accesses() {
        assert!(accesses("\tx = self.props.text\n\ty = 'self.custom.a'\n\t# self.view.custom.a\n\tsystem.perspective.print(self.meta.name)").is_empty());
    }

    /// Every Python 2.7 expression and statement context a followed value
    /// can appear in (one per kind of AST node that holds a name or an
    /// attribute). The use must be understood or access the whole
    /// container (every property of the view for a component).
    pub(crate) const CONTEXTS: &[&str] = &[
        // Expressions.
        "x = {v} or 1",
        "x = 1 and {v}",
        "x = {v} + 1",
        "x = '%s' % {v}",
        "x = 2 * {v}",
        "x = not {v}",
        "x = -{v}",
        "x = ~{v}",
        "x = +{v}",
        "f = lambda: {v}",
        "f = lambda a={v}: a",
        "x = {v} if c else 1",
        "x = 1 if {v} else 2",
        "x = 1 if c else {v}",
        "x = {{{v}: 1}}",
        "x = {{1: {v}}}",
        "x = {{{v}}}",
        "x = [{v} for i in y]",
        "x = [i for i in {v}]",
        "x = [i for i in y if {v}]",
        "x = {{{v} for i in y}}",
        "x = {{i: {v} for i in y}}",
        "x = ({v} for i in y)",
        "x = list(i for i in {v})",
        "yield {v}",
        "x = {v} == 1",
        "x = 1 < {v}",
        "x = {v} is None",
        "x = {v} is not y",
        "x = 1 in {v}",
        "x = {v} in y",
        "x = 'a' not in {v}",
        "x = {v} != y",
        "f({v})",
        "f(k={v})",
        "f(*{v})",
        "f(**{v})",
        "{v}()",
        "{v}.zzz()",
        "len({v})",
        "str({v})",
        "getattr({v}, 'a')",
        "system.perspective.print({v})",
        "x = `{v}`",
        "x = {v}.zzz",
        "x = {v}.__class__",
        "x = {v}.__dict__",
        "x = {v}[k]",
        "x = {v}[0:1]",
        "x = {v}[::2]",
        "x = {v}[1, 2]",
        "x = {v}[...]",
        "x = y[{v}]",
        "x = y[{v}:]",
        "{v}[k] = 1",
        "del {v}[k]",
        "x = [{v}]",
        "x = ({v}, 1)",
        "x = '{{}}'.format({v})",
        // Statements.
        "def g(a={v}):\n\t\tpass",
        "@{v}\n\tdef g():\n\t\tpass",
        "def g():\n\t\treturn {v}",
        "class K({v}):\n\t\tpass",
        "class K:\n\t\tq = {v}",
        "return {v}",
        "del {v}",
        "x = {v}",
        "x = y = {v}",
        "a, b = {v}",
        "[a, b] = {v}",
        "y.attr = {v}",
        "y[0] = {v}",
        "{v}.zzz = 1",
        "x += {v}",
        "{v}.zzz += 1",
        "{v} += 1",
        "print {v}",
        "print >>{v}, 1",
        "for i in {v}:\n\t\tpass",
        "for {v}.zzz in y:\n\t\tpass",
        "while {v}:\n\t\tbreak",
        "if {v}:\n\t\tpass",
        "with {v}:\n\t\tpass",
        "with y as {v}.zzz:\n\t\tpass",
        "raise {v}",
        "raise E, {v}",
        "raise E, 1, {v}",
        "try:\n\t\tpass\n\texcept {v}:\n\t\tpass",
        "try:\n\t\tpass\n\texcept E, {v}.zzz:\n\t\tpass",
        "try:\n\t\tpass\n\tfinally:\n\t\tx = {v}",
        "assert {v}",
        "assert c, {v}",
        "exec c in {v}",
        "global q\n\tq = {v}",
        "{v}",
    ];

    /// The understood uses among [`CONTEXTS`] for a container (`{v}` =
    /// `self.custom` of `B`) and what they access.
    pub(crate) fn understood_container(context: &str) -> Option<&'static [&'static str]> {
        Some(match context {
            "x = {v}" | "try:\n\t\tpass\n\tfinally:\n\t\tx = {v}" => &[],
            "x = {v}.zzz" => &["B.custom.zzz r"],
            "x = 'a' not in {v}" => &["B.custom.a r"],
            "{v}.zzz = 1" | "for {v}.zzz in y:\n\t\tpass" | "with y as {v}.zzz:\n\t\tpass" => {
                &["B.custom.zzz w"]
            }
            "try:\n\t\tpass\n\texcept E, {v}.zzz:\n\t\tpass" => &["B.custom.zzz w"],
            "{v}.zzz += 1" => &["B.custom.zzz r w"],
            "{v}.zzz()" => &["B.custom.zzz r"],
            "del {v}" => &["B.custom.* w"],
            "{v} += 1" => &["B.custom.* r w"],
            _ => return None,
        })
    }

    /// The understood uses among [`CONTEXTS`] for a component (`{v}` =
    /// `self.getSibling('A')`).
    pub(crate) fn understood_component(context: &str) -> Option<&'static [&'static str]> {
        Some(match context {
            "x = {v}" | "try:\n\t\tpass\n\tfinally:\n\t\tx = {v}" => &[],
            // Truth value and identity: no property is read.
            "x = not {v}"
            | "x = {v} is None"
            | "x = {v} is not y"
            | "while {v}:\n\t\tbreak"
            | "if {v}:\n\t\tpass" => &[],
            _ => return None,
        })
    }

    #[test]
    fn every_context_is_understood_or_reads_everything() {
        for context in CONTEXTS {
            let mut parsed_once = false;
            for (v, read_all, understood) in [
                (
                    "self.custom",
                    &["B.custom.*"][..],
                    understood_container as fn(&str) -> Option<&'static [&'static str]>,
                ),
                (
                    "self.getSibling('A')",
                    &EVERYTHING[..],
                    understood_component,
                ),
            ] {
                let body = format!(
                    "\t{}",
                    context
                        .replace("{v}", v)
                        .replace("{{", "{")
                        .replace("}}", "}")
                );
                if !iglint_python::parse_module(&format!("def f(self):\n{body}\n"))
                    .errors
                    .is_empty()
                {
                    continue;
                }
                parsed_once = true;
                let found = accesses(&body);
                match understood(context) {
                    Some(expected) => assert_eq!(found, expected, "{body}"),
                    None => assert!(
                        read_all
                            .iter()
                            .all(|r| found.iter().any(|f| f.starts_with(r)))
                            || is_everything(&found),
                        "{body}: {found:?}"
                    ),
                }
            }
            assert!(parsed_once, "{context}");
        }
    }
}
