//! Per-resource summaries: the owned facts the project index is built from.
//!
//! A summary is computed once per resource from its model (in parallel) and
//! holds no borrowed data, so it can be cached between runs. The script
//! layer adds what only Python analysis knows (library symbols, literal
//! `sendMessage` calls, ...) before the index is built.

use iglint_expression::{ExprKind, Reference, TagPath};
use iglint_text::TextRange;

use crate::ids::{FileId, ResourceId};
use crate::library::ModuleSymbols;
use crate::model::{ModelData, ResourceModel};
use crate::named_query::NamedQueryParamKind;
use crate::tags::{TagFolder, TagType};
use crate::view::{
    ActionKind, BindingConfig, NavTarget, ParamDirection, PropCategory, SessionProps, TagMode, View,
};

/// Owned facts about one resource.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSummary {
    pub resource: ResourceId,
    /// False when the main data file is missing or not valid JSON: the
    /// resource exists but its content is unknown.
    pub valid: bool,
    pub facts: Facts,
    /// Outgoing literal references.
    pub references: References,
}

/// Kind-specific facts.
#[derive(Clone, Debug, PartialEq)]
pub enum Facts {
    View(ViewFacts),
    StyleClass(StyleClassFacts),
    NamedQuery(NamedQueryFacts),
    PageConfig(PageConfigFacts),
    SessionProps(SessionFacts),
    Library(LibraryFacts),
    Tags(TagFacts),
    Gateway(GatewayFacts),
    /// Project stylesheet or gateway theme: CSS class names it defines.
    Css(CssFacts),
    None,
}

/// CSS classes defined by a stylesheet or theme.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CssFacts {
    pub classes: Vec<String>,
}

/// Facts about a gateway config resource.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GatewayFacts {
    /// Tag providers: `profile.type` (`STANDARD`, `MANAGED`, `REMOTE`, ...).
    pub provider_type: Option<String>,
}

/// Facts about a view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewFacts {
    pub params: Vec<ParamFact>,
    pub message_handlers: Vec<HandlerFact>,
}

/// A view parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamFact {
    pub name: String,
    pub direction: Option<ParamDirection>,
}

/// A component message handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerFact {
    pub message_type: String,
    pub page_scope: bool,
    pub session_scope: bool,
    pub view_scope: bool,
}

/// Facts about a style class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StyleClassFacts {
    pub empty: bool,
}

/// Facts about a named query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamedQueryFacts {
    pub query_type: Option<String>,
    pub database: Option<String>,
    pub parameters: Vec<(String, NamedQueryParamKind)>,
}

/// Facts about page configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageConfigFacts {
    pub pages: Vec<PageFact>,
    /// Dock ids (shared and per page).
    pub dock_ids: Vec<String>,
}

/// A page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageFact {
    pub url: String,
    pub view_path: Option<String>,
}

/// Facts about session properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionFacts {
    /// Top-level `custom` names (including `propConfig`-only ones).
    pub custom: Vec<String>,
    /// Top-level `props` keys written in the file.
    pub props: Vec<String>,
}

/// Facts about a library module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryFacts {
    /// Top-level symbols, filled in by the Python layer
    /// ([`ResourceSummary::set_module_symbols`]).
    pub symbols: Option<ModuleSymbols>,
}

/// Facts about a tag folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagFacts {
    pub provider: String,
    pub folder: String,
    pub definitions: bool,
    pub tags: Vec<TagFact>,
}

/// A tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagFact {
    /// Path inside the provider (or inside `_types_`).
    pub path: String,
    pub tag_type: TagType,
    pub type_id: Option<String>,
    /// UDT parameter names (definitions and instances).
    pub parameters: Vec<String>,
    /// False when the data file is not listed in the folder manifest (the
    /// gateway ignores it).
    pub listed: bool,
    /// For UDT definitions: member paths relative to the definition.
    pub members: Vec<String>,
    /// For UDT definitions: nested UDT instance members (relative path,
    /// type id).
    pub member_instances: Vec<(String, String)>,
}

/// How a message is sent.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MessageTarget {
    /// `system.perspective.sendMessage(type, payload, scope=...)`; `scope` is
    /// `session`, `page` or `view` (`None` when not literal).
    Perspective { scope: Option<String> },
    /// `system.util.sendMessage(project, handler, ...)` /
    /// `sendRequest`: gateway and session message handlers of `project`
    /// (`None` when not literal).
    Gateway { project: Option<String> },
}

/// A literal message send.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MessageSend {
    pub message_type: String,
    pub target: MessageTarget,
    /// The top-level keys of the payload when it is a dict literal with
    /// literal keys (`None` when the payload is anything else).
    pub payload_keys: Option<Vec<String>>,
}

/// References whose target is computed at run time. Each list holds
/// prefixes of the names the reference may produce (`"Popups/"` for
/// `'Popups/' + name`); an empty prefix stands for any name. "Unused"
/// rules stay silent for names a prefix covers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DynamicReferences {
    /// View paths.
    pub views: Vec<String>,
    /// Named query paths.
    pub named_queries: Vec<String>,
    /// Dotted library names reached in ways that cannot be followed: a
    /// module used as a value, `getattr`, `from m import *`, a non-literal
    /// `runScript` target.
    pub library: Vec<String>,
    /// Message types of `system.perspective.sendMessage`.
    pub perspective_messages: Vec<String>,
    /// Handler names of `system.util.sendMessage` / `sendRequest`, with
    /// the target project (`None`: any project).
    pub gateway_messages: Vec<(Option<String>, String)>,
}

impl DynamicReferences {
    /// Appends everything from `other`.
    pub fn extend(&mut self, other: &DynamicReferences) {
        self.views.extend(other.views.iter().cloned());
        self.named_queries
            .extend(other.named_queries.iter().cloned());
        self.library.extend(other.library.iter().cloned());
        self.perspective_messages
            .extend(other.perspective_messages.iter().cloned());
        self.gateway_messages
            .extend(other.gateway_messages.iter().cloned());
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
            && self.named_queries.is_empty()
            && self.library.is_empty()
            && self.perspective_messages.is_empty()
            && self.gateway_messages.is_empty()
    }
}

/// Whether `name` may be produced by a dynamic reference with one of
/// `prefixes`.
pub fn covered_by_prefix(prefixes: &[String], name: &str) -> bool {
    prefixes.iter().any(|p| name.starts_with(p.as_str()))
}

/// Outgoing literal references of a resource.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct References {
    /// View paths.
    pub views: Vec<String>,
    /// Style class paths.
    pub style_classes: Vec<String>,
    /// Named query paths.
    pub named_queries: Vec<String>,
    /// Page URLs.
    pub pages: Vec<String>,
    /// Tag paths (`[provider]path`).
    pub tag_paths: Vec<String>,
    /// Dotted library names (`core.format.withUnit`).
    pub library: Vec<String>,
    /// Dotted library names the script assigns, deletes or replaces the
    /// code of (`core.util.fmt = f`, `setattr(core.util, 'fmt', f)`,
    /// `core.util.__dict__[k] = v`, `core.util.fmt.func_code = c`): the
    /// name and everything under it may differ at run time from its
    /// definition.
    pub library_stores: Vec<String>,
    /// Dotted library names whose object reaches code that may change it
    /// or its module: a function or class used as a value (other than an
    /// argument of a `system.*` call), the base of `func_globals` or
    /// `__globals__`. Every name of the module that defines it may be
    /// rebound at run time.
    pub library_exposed: Vec<String>,
    /// Objects the script changes through a dotted name that may be a
    /// library name, by the dotted name of the object, with where: the
    /// object of `lib.math.pi = 1`, `del lib.math.pi`, `lib.math.pi += 1`,
    /// `setattr(lib.math, ...)` and `lib.math.__dict__` is `lib.math`.
    /// When the name reaches, through a library module's import, an object
    /// that is not a library module (`import math` in `lib`), the change
    /// is to a shared object whose measured facts no longer hold: the
    /// index counts it as unknown code
    /// ([`crate::ProjectIndex::shared_object_site`]).
    pub library_changes: Vec<(String, UnknownSite)>,
    /// Dotted names that may be library names used as values (passed,
    /// kept, returned), with where. A value that is, or holds through a
    /// library module's imports, an object that is not a library module
    /// (`m = lib.math`, `f(lib)` with `import math` in `lib`) hands a
    /// shared object to code that is not followed: unknown code.
    pub library_values: Vec<(String, UnknownSite)>,
    /// Dotted library modules the script star-imports (`from core.util
    /// import *`): every name the module exports this way is used.
    pub library_star_imports: Vec<String>,
    /// Message sends (filled in by the script layer).
    pub messages: Vec<MessageSend>,
    /// Historian provider names.
    pub history_providers: Vec<String>,
    /// Database connection names.
    pub databases: Vec<String>,
    /// References computed at run time.
    pub dynamic: DynamicReferences,
    /// A script uses something that is not vetted (see
    /// `iglint_python_semantic::vetted`): it may read the namespace of any
    /// module by name.
    pub unknown_code: bool,
    /// Where [`References::unknown_code`] comes from: the first such use
    /// of the resource (by file and offset).
    pub unknown_site: Option<UnknownSite>,
    /// Code that may read a file a fix edits (its bytes, or metadata that
    /// depends on them): a file read in vetted code whose path is not a
    /// literal outside the workspace (`iglint_python_semantic::file_reads`),
    /// or any code that is not vetted or reads frames, which may call
    /// `open` under another name. Every fix edits workspace files, so this
    /// keeps every fix of the workspace from being safe. Code that only
    /// reaches other views does not count.
    pub reads_files: bool,
    /// Where [`References::reads_files`] comes from: the first such site.
    pub file_read_site: Option<UnknownSite>,
    /// Imports of modules that are not vetted, with where they are; any
    /// that is not a project library module counts as
    /// [`References::unknown_code`].
    pub unvetted_imports: Vec<(String, UnknownSite)>,
    /// Names that are not in the measured 8.3 lists, with where they are
    /// used: `system.*` members the stubs do not list and expression
    /// functions the gateway does not register (a third-party module's).
    /// Each counts as [`References::unknown_code`] and as a file read,
    /// unless the project vets it ([`crate::index::Vetted`]).
    pub unvetted_names: Vec<(UnvettedName, UnknownSite)>,
    /// Calls that hand a Perspective object (a component, a view, a page
    /// or a session) to a dotted name that may be a library function. The
    /// linter resolves them once the index is built: a callee that may
    /// reach other views with it counts as [`References::unknown_code`]
    /// ([`crate::ProjectIndex::mark_unknown_code`]).
    pub perspective_calls: Vec<PerspectiveCall>,
}

/// A name that is not in the measured 8.3 lists
/// ([`References::unvetted_names`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnvettedName {
    /// A `system.*` chain the stubs do not list, as written
    /// (`system.perspective.runJavaScriptAsync`).
    System(String),
    /// An expression function the gateway does not register, as written.
    ExpressionFunction(String),
}

/// A call that hands a Perspective object to a name that may be a library
/// function ([`References::perspective_calls`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerspectiveCall {
    /// The dotted name called, as written in the script.
    pub callee: String,
    /// The parameter the object binds to.
    pub slot: crate::library::ArgSlot,
    /// A page or session (`true`), or a component or view.
    pub host: bool,
    /// Where the object is passed.
    pub site: UnknownSite,
}

/// Where a script uses code that is not vetted or reads a frame or the
/// interpreter, to explain why a fix is unsafe.
///
/// `range` is not part of the summary's identity (`Debug`, `PartialEq`):
/// an edit that only moves the site keeps the summaries, and so the cached
/// results of other units, as they were. Diagnostics that name a site of
/// another file are completed after the cache, from the current index
/// ([`crate::ProjectIndex::first_unknown_code`]).
#[derive(Clone)]
pub struct UnknownSite {
    pub file: FileId,
    /// The range in `file`. While the script layer collects references it
    /// is a range of the script's source, until the site is placed in the
    /// file ([`UnknownSite::DETACHED`]).
    pub range: TextRange,
    /// The construct, as written (one line, shortened).
    pub what: String,
}

impl UnknownSite {
    /// The file of a site not yet placed in a file.
    pub const DETACHED: FileId = FileId(u32::MAX);

    /// A site at `range` of a script's source, placed in a file later.
    pub fn detached(range: TextRange) -> Self {
        UnknownSite {
            file: Self::DETACHED,
            range,
            what: String::new(),
        }
    }
}

impl std::fmt::Debug for UnknownSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnknownSite")
            .field("file", &self.file)
            .field("what", &self.what)
            .finish_non_exhaustive()
    }
}

impl PartialEq for UnknownSite {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file && self.what == other.what
    }
}

impl Eq for UnknownSite {}

impl References {
    /// Appends everything from `other`.
    pub fn extend(&mut self, other: References) {
        self.views.extend(other.views);
        self.style_classes.extend(other.style_classes);
        self.named_queries.extend(other.named_queries);
        self.pages.extend(other.pages);
        self.tag_paths.extend(other.tag_paths);
        self.library.extend(other.library);
        self.library_stores.extend(other.library_stores);
        self.library_exposed.extend(other.library_exposed);
        self.library_star_imports.extend(other.library_star_imports);
        self.library_changes.extend(other.library_changes);
        self.library_values.extend(other.library_values);
        self.messages.extend(other.messages);
        self.history_providers.extend(other.history_providers);
        self.databases.extend(other.databases);
        self.dynamic.extend(&other.dynamic);
        self.unknown_code |= other.unknown_code;
        if self.unknown_site.is_none() {
            self.unknown_site = other.unknown_site;
        }
        self.reads_files |= other.reads_files;
        if self.file_read_site.is_none() {
            self.file_read_site = other.file_read_site;
        }
        self.unvetted_imports.extend(other.unvetted_imports);
        self.unvetted_names.extend(other.unvetted_names);
        self.perspective_calls.extend(other.perspective_calls);
    }
}

impl ResourceSummary {
    /// Attaches the symbols of a library module.
    pub fn set_module_symbols(&mut self, symbols: ModuleSymbols) {
        if let Facts::Library(l) = &mut self.facts {
            l.symbols = Some(symbols);
        }
    }
}

/// Computes the summary of a resource model.
pub fn summarize(model: &ResourceModel<'_>) -> ResourceSummary {
    let mut references = References::default();
    let mut valid = true;
    let facts = match &model.data {
        ModelData::View { view, file } => {
            hidden_code(&view.problems, *file, &mut references);
            client_code(view, *file, &mut references);
            configuration_reads(*file, &view.bindings, &mut references);
            Facts::View(view_facts(view, &mut references))
        }
        ModelData::StyleClass(s) => Facts::StyleClass(StyleClassFacts {
            empty: s.is_empty(),
        }),
        ModelData::NamedQuery(q) => {
            if let Some(db) = q.database.filter(|d| !d.is_empty()) {
                references.databases.push(db.to_owned());
            }
            Facts::NamedQuery(NamedQueryFacts {
                query_type: q.query_type.map(str::to_owned),
                database: q.database.map(str::to_owned),
                parameters: q
                    .parameters
                    .iter()
                    .filter_map(|p| Some((p.identifier?.value().to_owned(), p.kind)))
                    .collect(),
            })
        }
        ModelData::PageConfig(p) => {
            let mut facts = PageConfigFacts::default();
            for page in &p.pages {
                facts.pages.push(PageFact {
                    url: page.url.value().to_owned(),
                    view_path: page.view_path.map(|v| v.value().to_owned()),
                });
                references
                    .views
                    .extend(page.view_path.map(|v| v.value().to_owned()));
                for d in &page.docks {
                    facts.dock_ids.extend(d.id.map(|i| i.value().to_owned()));
                    references
                        .views
                        .extend(d.view_path.map(|v| v.value().to_owned()));
                }
            }
            for d in &p.shared_docks {
                facts.dock_ids.extend(d.id.map(|i| i.value().to_owned()));
                references
                    .views
                    .extend(d.view_path.map(|v| v.value().to_owned()));
            }
            Facts::PageConfig(facts)
        }
        ModelData::SessionProps { props, file } => {
            hidden_code(&props.problems, *file, &mut references);
            configuration_reads(*file, &props.bindings, &mut references);
            Facts::SessionProps(session_facts(props, &mut references))
        }
        ModelData::Library(_) => Facts::Library(LibraryFacts::default()),
        ModelData::Tags(t) => {
            if let Some((file, _)) = t.files.first() {
                hidden_code(&t.problems, *file, &mut references);
            }
            tag_configuration_reads(t, &mut references);
            Facts::Tags(tag_facts(t, &mut references))
        }
        ModelData::Stylesheet { text, .. } => Facts::Css(CssFacts {
            classes: crate::perspective::css_classes(text),
        }),
        ModelData::Theme { css } => {
            let mut classes: Vec<String> = css
                .iter()
                .flat_map(|(_, t)| crate::perspective::css_classes(t))
                .collect();
            classes.sort_unstable();
            classes.dedup();
            Facts::Css(CssFacts { classes })
        }
        ModelData::Gateway(g) => Facts::Gateway(GatewayFacts {
            provider_type: match &g.details {
                crate::gateway::GatewayDetails::TagProvider { provider_type } => {
                    provider_type.map(str::to_owned)
                }
                _ => None,
            },
        }),
        ModelData::EventScript(e) => {
            if let crate::event_resource::EventSettings::TagChange { paths, .. } = &e.settings {
                for (s, _) in paths {
                    // A tag change script on a configuration property gets
                    // the configuration as its new value.
                    if !tag_path_names_value(s.value()) {
                        configuration_read(
                            model.resource.manifest.file,
                            s.range(),
                            s.value(),
                            &mut references,
                        );
                    }
                }
                for (s, p) in paths {
                    if p.is_valid() && !p.is_relative() {
                        references.tag_paths.push(s.value().to_owned());
                    }
                }
            }
            Facts::None
        }
        ModelData::None => {
            valid = !matches!(
                model.resource.kind,
                crate::kinds::ResourceKind::View
                    | crate::kinds::ResourceKind::StyleClass
                    | crate::kinds::ResourceKind::PageConfig
                    | crate::kinds::ResourceKind::SessionProps
            );
            Facts::None
        }
        _ => Facts::None,
    };
    for (file, e) in model.expressions() {
        expression_configuration_reads(file, e, &mut references);
        unknown_expression_functions(file, e, &mut references);
    }
    if !model.errors.is_empty() {
        valid = false;
    }
    ResourceSummary {
        resource: model.resource.id,
        valid,
        facts,
        references,
    }
}

/// Whether the tag path text `path` can only name a tag's value: the text
/// after its last `/` (or `]`) has no `{...}` placeholder and names no
/// property, or `Value`, `Quality` or `Timestamp` (case-insensitively). A
/// property that is empty or only dots is none: Ignition 8.3.3's
/// `TagPathParser` reads `..`, `.`, `T.` and `a/..` without a property
/// (`..` and `.` as an empty path).
/// Any other property may be configuration (`Tag.eventScripts`,
/// `Tag.expression`, `Tag.alarms`): reading it hands out the content of
/// the files a fix edits. The same rule as the vetted `system.tag` reads
/// and writes of literal paths (`iglint_python_semantic::vetted`).
pub fn tag_path_names_value(path: &str) -> bool {
    let tail = path.rsplit(['/', ']']).next().unwrap_or(path);
    if tail.contains(['{', '}']) {
        return false;
    }
    match tail.split_once('.') {
        None => true,
        Some((_, property)) => {
            property.chars().all(|c| c == '.')
                || matches!(
                    property.to_ascii_lowercase().as_str(),
                    "value" | "quality" | "timestamp"
                )
        }
    }
}

/// Records a reference that may read tag configuration
/// ([`tag_path_names_value`] does not hold, or the path is computed): the
/// configuration of a tag is the content of a `tags.json` file a fix
/// edits, and an expression tag, reference or derived tag, binding or
/// tag change script that reads it hands it to every reader of the value
/// (verified on 8.3.3: `{[.]T.eventScripts}`, `tag('[.]T.eventScripts')`,
/// a reference tag on `[.]T.eventScripts` all have the event script text
/// as their value). Counted like a file read (`References::reads_files`).
fn configuration_read(file: FileId, range: TextRange, path: &str, refs: &mut References) {
    refs.reads_files = true;
    if refs.file_read_site.is_none() {
        let mut what: String = path.chars().take(60).collect();
        if what.len() < path.len() {
            what.push_str("...");
        }
        refs.file_read_site = Some(UnknownSite {
            file,
            range,
            what: format!("tag path `{what}`, which may read tag configuration"),
        });
    }
}

/// [`configuration_read`] for the tag references of an expression:
/// `{...}` tag paths and `tag(...)` (a computed path counts).
fn expression_configuration_reads(
    file: FileId,
    e: &crate::EmbeddedExpression<'_>,
    refs: &mut References,
) {
    for (r, resolved) in e.references() {
        if let Reference::Tag(t) = resolved
            && !tag_path_names_value(&t.raw)
        {
            configuration_read(file, e.file_range(r.path_range), &t.raw, refs);
        }
    }
    for call in e.parsed.expr.calls() {
        if !call.is("tag") {
            continue;
        }
        match call.args.first().map(|a| &a.unparenthesized().kind) {
            Some(ExprKind::String(s)) if tag_path_names_value(s) => {}
            Some(ExprKind::String(s)) => {
                let range = call.args[0].range;
                configuration_read(file, e.file_range(range), s, refs);
            }
            _ => {
                let range = call.args.first().map_or(call.name_range, |a| a.range);
                configuration_read(file, e.file_range(range), "tag(...)", refs);
            }
        }
    }
}

/// [`configuration_read`] for Perspective tag bindings: a direct or
/// indirect path whose tail may name a configuration property, or a path
/// computed by an expression.
fn configuration_reads(file: FileId, bindings: &[crate::view::Binding<'_>], refs: &mut References) {
    for b in bindings {
        let BindingConfig::Tag(t) = &b.typed else {
            continue;
        };
        match (t.mode, t.tag_path) {
            (TagMode::Direct | TagMode::Indirect, Some(p)) if tag_path_names_value(p.value()) => {}
            (_, Some(p)) => configuration_read(file, p.range(), p.value(), refs),
            (_, None) => configuration_read(file, b.value.range, &b.node_path, refs),
        }
    }
}

/// [`configuration_read`] for tags: reference and derived tags'
/// `sourceTagPath` (literal or bound), and alarm properties bound to a
/// tag.
fn tag_configuration_reads(folder: &TagFolder<'_>, refs: &mut References) {
    for t in &folder.tags {
        if let Some(p) = t.source_tag_path
            && !tag_path_names_value(p.value())
        {
            configuration_read(t.file, p.range(), p.value(), refs);
        }
        for b in &t.bound_properties {
            if b.property.value() == "sourceTagPath" {
                let text = b.binding.map_or("", |s| s.value());
                // A parameter binding substitutes text; an expression
                // computes the path.
                let parameter = b
                    .bind_type
                    .is_some_and(|k| k.eq_ignore_ascii_case("parameter"));
                if !parameter || !tag_path_names_value(text) {
                    configuration_read(t.file, b.value.range, text, refs);
                }
            }
        }
        for a in &t.alarms {
            for b in &a.bindings {
                if b.bind_type
                    .is_some_and(|k| k.value().eq_ignore_ascii_case("tag"))
                {
                    match b.bound_value.and_then(|v| v.as_json_string()) {
                        Some(p) if tag_path_names_value(p.value()) => {}
                        Some(p) => configuration_read(t.file, p.range(), p.value(), refs),
                        None => configuration_read(t.file, b.value.range, "", refs),
                    }
                }
            }
        }
    }
}

fn literal_tag_path(p: &TagPath) -> bool {
    p.is_valid() && !p.is_relative() && p.source.is_some()
}

/// References found in expressions: `{[provider]tag}` references, literal
/// `tag("...")` paths and `runScript("pkg.mod.func", ...)` targets.
fn expression_refs<'e>(
    exprs: impl Iterator<Item = &'e crate::EmbeddedExpression<'e>>,
    refs: &mut References,
) {
    for e in exprs {
        for (_, r) in e.references() {
            if let Reference::Tag(t) = r
                && literal_tag_path(&t)
            {
                refs.tag_paths.push(t.raw.clone());
            }
        }
        for call in e.parsed.expr.calls() {
            let first = call.args.first().map(|a| &a.unparenthesized().kind);
            let Some(ExprKind::String(s)) = first else {
                if call.is("runScript") {
                    // A computed function name may reach any library name.
                    refs.dynamic.library.push(String::new());
                }
                continue;
            };
            if call.is("runScript") {
                // A dotted name is a library reference; any other literal
                // is code, which the script layer analyses.
                let name = s.trim();
                let dotted = !name.is_empty()
                    && name.split('.').all(|p| {
                        p.chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                            && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    });
                if dotted {
                    refs.library.push(name.to_owned());
                }
            } else if call.is("tag") {
                let p = iglint_expression::parse_tag_path(s, iglint_text::TextSize::ZERO);
                if literal_tag_path(&p) {
                    refs.tag_paths.push(s.clone());
                }
            }
        }
    }
}

fn binding_refs(bindings: &[crate::view::Binding<'_>], refs: &mut References) {
    for b in bindings {
        match &b.typed {
            BindingConfig::Query(q) => refs
                .named_queries
                .extend(q.query_path.map(|p| p.value().to_owned())),
            BindingConfig::Tag(t) if t.mode == TagMode::Direct => {
                if let (Some(s), Some(p)) = (t.tag_path, &t.parsed_path)
                    && literal_tag_path(p)
                    && !s.value().contains('{')
                {
                    refs.tag_paths.push(s.value().to_owned());
                }
            }
            _ => {}
        }
    }
}

/// Structural problems that skip a place for scripts holding strings
/// ([`crate::view::ModelProblem::hides_code`]): the code there is not
/// read, so it is unknown code for the closed world.
fn hidden_code(problems: &[crate::view::ModelProblem], file: FileId, refs: &mut References) {
    for p in problems.iter().filter(|p| p.hides_code) {
        let site = UnknownSite {
            file: p.file.unwrap_or(file),
            range: p.range,
            what: p.message.clone(),
        };
        refs.unknown_code = true;
        refs.reads_files = true;
        if refs.unknown_site.is_none() {
            refs.unknown_site = Some(site.clone());
        }
        if refs.file_read_site.is_none() {
            refs.file_read_site = Some(site);
        }
    }
}

/// Calls of expression functions that Ignition 8.3 does not register for
/// the expression's context (a third-party module's: Vision's
/// `objectScript` or a simulation module's run Python), and Perspective's
/// `property`, which reads properties by a computed path: code iglint does
/// not read ([`References::unvetted_names`]). Expressions that do not
/// parse do not run.
fn unknown_expression_functions(
    file: FileId,
    e: &crate::EmbeddedExpression<'_>,
    refs: &mut References,
) {
    if !e.parsed.is_valid() {
        return;
    }
    for call in e.parsed.expr.calls() {
        let what =
            if iglint_stubs::expression_function_in(&call.name, e.function_context()).is_none() {
                "not an expression function of Ignition 8.3"
            } else if call.name.eq_ignore_ascii_case("property") {
                // Perspective's `property(path)` reads any property of the
                // view or session by a path computed at run time, which the
                // property access analysis does not follow.
                "reads a property by a path computed at run time"
            } else {
                continue;
            };
        refs.unvetted_names.push((
            UnvettedName::ExpressionFunction(call.name.clone()),
            UnknownSite {
                file,
                range: e.file_range(call.name_range),
                what: format!("{} ({what})", call.name),
            },
        ));
    }
}

/// What in a view can run JavaScript in the browser
/// ([`iglint_stubs::client_code`]): a component whose properties can run
/// code (a type the 8.3 component list does not have, an inline frame, an
/// SVG drawing, ...; a Markdown component that renders HTML, a link whose
/// URL may be a script), and a navigate action whose URL may be a script.
/// Code in a page reads and writes the properties of every view the page
/// shows, opens other pages of the session and calls the gateway with the
/// session's credentials, so it is unknown code for the whole workspace
/// and may read the files a fix edits.
///
/// A Markdown component or link counts unless its literal configuration
/// is plain and nothing can change it at run time: no `propConfig` entry
/// on `props` or the property, and no script in the view (only the view's
/// own scripts, and code handed one of its components by them, can write
/// its properties; other views reach it only through a page or session,
/// which is unknown code already).
fn client_code(view: &View<'_>, file: FileId, refs: &mut References) {
    use iglint_stubs::client_code::{
        ClientCode, MarkdownValue, component_client_code, markdown_is_plain, url_runs_no_script,
    };
    fn plain_svg(elements: &iglint_json::Value<'_>) -> bool {
        use iglint_stubs::client_code::{svg_attribute_is_plain, svg_element_type_is_plain};
        let plain_type = |v: Option<&iglint_json::Value<'_>>| {
            v.and_then(|t| t.as_str())
                .is_some_and(svg_element_type_is_plain)
        };
        elements.as_array().is_some_and(|a| {
            a.iter().all(|e| {
                e.as_object().is_some_and(|o| {
                    plain_type(o.get("type"))
                        && o.iter().all(|m| match m.key() {
                            "type" | "style" | "name" => true,
                            "elements" => plain_svg(&m.value),
                            "wrapper" => m.value.as_object().is_some_and(|w| {
                                w.iter().all(|wm| match wm.key() {
                                    "type" => {
                                        wm.value.as_str() == Some("") || plain_type(Some(&wm.value))
                                    }
                                    key => svg_attribute_is_plain(key, wm.value.as_str()),
                                })
                            }),
                            key => svg_attribute_is_plain(key, m.value.as_str()),
                        })
                })
            })
        })
    }
    let bound = |c: &crate::view::Component<'_>, property: &str| {
        view.prop_configs.iter().any(|p| {
            let key = p.key.value();
            p.owner == crate::script::Owner::Component(c.id)
                && (key == "props"
                    || key
                        .strip_prefix("props.")
                        .and_then(|k| k.strip_prefix(property))
                        .is_some_and(|rest| {
                            rest.is_empty() || rest.starts_with('.') || rest.starts_with('[')
                        }))
        })
    };
    let fixed = |c: &crate::view::Component<'_>, property: &str| {
        view.scripts.is_empty() && !bound(c, property)
    };
    let mut found: Option<(TextRange, String)> = None;
    for c in &view.components {
        let why = match component_client_code(c.type_id) {
            ClientCode::None => None,
            ClientCode::Always(why) => Some(why.to_owned()),
            ClientCode::Markdown => {
                let plain = c
                    .props
                    .and_then(|p| p.get("markdown"))
                    .and_then(|m| m.as_object())
                    .is_some_and(|m| {
                        markdown_is_plain(m.iter().map(|member| {
                            let value = if member.value.as_bool() == Some(true) {
                                MarkdownValue::True
                            } else if member.value.as_object().is_some_and(|o| o.is_empty()) {
                                MarkdownValue::EmptyObject
                            } else {
                                MarkdownValue::Other
                            };
                            (member.key(), value)
                        }))
                    });
                (!(plain && fixed(c, "markdown"))).then(|| {
                    "it renders raw HTML unless props.markdown is a literal with escapeHtml true \
                     and only the documented keys, which nothing in the view can change"
                        .to_owned()
                })
            }
            ClientCode::Svg => {
                let plain = c
                    .props
                    .and_then(|p| p.get("elements"))
                    .is_none_or(plain_svg);
                (!(plain && fixed(c, "elements"))).then(|| {
                    "it renders the element types and attributes that props.elements names (a \
                     script, a or foreignObject element, a link that is not a #reference), and \
                     they are not plain literals that nothing in the view can change"
                        .to_owned()
                })
            }
            ClientCode::Link => {
                let plain = c
                    .props
                    .and_then(|p| p.get("url"))
                    .is_none_or(|u| u.as_str().is_some_and(url_runs_no_script));
                (!(plain && fixed(c, "url"))).then(|| {
                    "props.url is the address of the link, and a javascript: URL runs code; it \
                     is not a literal URL that nothing in the view can change"
                        .to_owned()
                })
            }
        };
        if let Some(why) = why
            && found.is_none()
        {
            let range = c.type_value.map_or(c.value.range, |t| t.range);
            found = Some((
                range,
                format!(
                    "{} ({}): can run JavaScript in the browser, {why}",
                    c.path,
                    c.type_id.unwrap_or("no type")
                ),
            ));
        }
    }
    for e in &view.events {
        for a in &e.actions {
            if !matches!(a.kind, ActionKind::Navigate(_)) {
                continue;
            }
            let Some(url) = a.config.and_then(|c| c.get("url")) else {
                continue;
            };
            if url.as_str().is_some_and(url_runs_no_script) || found.is_some() {
                continue;
            }
            found = Some((
                url.range,
                format!(
                    "{}: a navigate action to a URL that may run JavaScript in the browser",
                    a.node_path
                ),
            ));
        }
    }
    if let Some((range, what)) = found {
        let site = UnknownSite { file, range, what };
        refs.unknown_code = true;
        refs.reads_files = true;
        if refs.unknown_site.is_none() {
            refs.unknown_site = Some(site.clone());
        }
        if refs.file_read_site.is_none() {
            refs.file_read_site = Some(site);
        }
    }
}

fn view_facts(view: &View<'_>, refs: &mut References) -> ViewFacts {
    let params = view
        .params()
        .map(|p| ParamFact {
            name: p.name.to_owned(),
            direction: p.direction,
        })
        .collect();
    let message_handlers = view
        .message_handlers
        .iter()
        .filter_map(|h| {
            Some(HandlerFact {
                message_type: h.message_type?.value().to_owned(),
                page_scope: h.page_scope,
                session_scope: h.session_scope,
                view_scope: h.view_scope,
            })
        })
        .collect();
    for r in &view.view_refs {
        if !r.bound {
            refs.views.push(r.path.value().to_owned());
        } else {
            match bound_view_path(view, r) {
                BoundPath::Literal(path) => refs.views.push(path),
                BoundPath::Prefix(prefix) => refs.dynamic.views.push(prefix),
            }
        }
    }
    for s in &view.style_refs {
        if !s.bound {
            refs.style_classes
                .extend(s.classes.iter().map(|(c, _)| (*c).to_owned()));
        }
    }
    for e in &view.events {
        for a in &e.actions {
            if let ActionKind::Navigate(NavTarget::Page(p)) = &a.kind {
                refs.pages.push(p.value().to_owned());
            }
        }
    }
    binding_refs(&view.bindings, refs);
    expression_refs(view.expressions.iter(), refs);
    ViewFacts {
        params,
        message_handlers,
    }
}

/// What a bound view path may evaluate to.
enum BoundPath {
    Literal(String),
    Prefix(String),
}

/// A bound view path: an expression binding without transforms whose
/// expression is a string literal, or a concatenation starting with one,
/// gives the path or its prefix; anything else may be any view.
fn bound_view_path(view: &View<'_>, r: &crate::view::ViewRef<'_>) -> BoundPath {
    let any = BoundPath::Prefix(String::new());
    let Some(b) = view
        .bindings
        .iter()
        .find(|b| b.owner == r.owner && b.property == r.property)
    else {
        return any;
    };
    let BindingConfig::Expr {
        expression: Some(id),
    } = &b.typed
    else {
        return any;
    };
    if !b.transforms.is_empty() {
        return any;
    }
    let e = view.expression(*id);
    if !e.parsed.errors.is_empty() {
        return any;
    }
    fn prefix(e: &iglint_expression::Expr) -> Option<&str> {
        match &e.kind {
            ExprKind::String(s) => Some(s),
            ExprKind::Paren(inner) => prefix(inner),
            ExprKind::Binary(b) if b.op == iglint_expression::BinaryOp::Add => prefix(&b.left),
            _ => None,
        }
    }
    match &e.parsed.expr.kind {
        ExprKind::String(s) => BoundPath::Literal(s.clone()),
        _ => BoundPath::Prefix(prefix(&e.parsed.expr).unwrap_or("").to_owned()),
    }
}

fn session_facts(props: &SessionProps<'_>, refs: &mut References) -> SessionFacts {
    binding_refs(&props.bindings, refs);
    expression_refs(props.expressions.iter(), refs);
    SessionFacts {
        custom: props
            .properties
            .iter()
            .filter(|p| p.category == PropCategory::Custom)
            .map(|p| p.name.to_owned())
            .collect(),
        props: props
            .props
            .and_then(|p| p.as_object())
            .map(|o| o.keys().map(str::to_owned).collect())
            .unwrap_or_default(),
    }
}

fn tag_facts(folder: &TagFolder<'_>, refs: &mut References) -> TagFacts {
    let mut tags = Vec::with_capacity(folder.tags.len());
    for t in &folder.tags {
        let listed = folder
            .files
            .iter()
            .find(|(f, _)| *f == t.file)
            .is_none_or(|(_, l)| *l);
        let mut members = Vec::new();
        let mut member_instances = Vec::new();
        if t.tag_type == TagType::UdtType {
            let mut stack: Vec<_> = t.children.iter().rev().copied().collect();
            while let Some(c) = stack.pop() {
                let child = folder.tag(c);
                let rel = child
                    .path
                    .get(t.path.len()..)
                    .unwrap_or_default()
                    .trim_start_matches('/')
                    .to_owned();
                if child.tag_type == TagType::UdtInstance
                    && let Some(type_id) = child.type_id
                {
                    member_instances.push((rel.clone(), type_id.value().to_owned()));
                }
                members.push(rel);
                stack.extend(child.children.iter().rev().copied());
            }
        }
        if let Some(h) = t.history_provider
            && !h.value().is_empty()
            && !h.value().contains('{')
        {
            refs.history_providers.push(h.value().to_owned());
        }
        if let Some(d) = t.datasource
            && !d.value().is_empty()
            && !d.value().contains('{')
        {
            refs.databases.push(d.value().to_owned());
        }
        tags.push(TagFact {
            path: t.path.clone(),
            tag_type: t.tag_type,
            type_id: t.type_id.map(|s| s.value().to_owned()),
            parameters: t
                .parameters
                .iter()
                .map(|p| p.name.value().to_owned())
                .collect(),
            listed,
            members,
            member_instances,
        });
    }
    expression_refs(folder.expressions.iter(), refs);
    TagFacts {
        provider: folder.provider.to_owned(),
        folder: folder.folder.to_owned(),
        definitions: folder.definitions,
        tags,
    }
}

#[cfg(test)]
mod tests {
    use super::tag_path_names_value;

    /// Review round 12: tag paths that may name configuration properties
    /// (verified on 8.3.3: `{[.]T.eventScripts}`, `tag('[.]T.eventScripts')`
    /// and a reference tag on `[.]T.eventScripts` hand out the script text).
    #[test]
    fn tag_paths_that_name_values() {
        for p in [
            "[default]T",
            "[default]Folder/T.Value",
            "[.]T.quality",
            "[~]A/B.Timestamp",
            "T",
            "[default]{1}/Speed",
            "[default]T[0]",
            "[default]",
            // Review round 22 (Opus) N1: no property for `TagPathParser`.
            "..",
            ".",
            "[default]..",
            "T.",
            "a/..",
        ] {
            assert!(tag_path_names_value(p), "{p}");
        }
        for p in [
            "[default]T.eventScripts",
            "[.]T.expression",
            "[default]T.Documentation",
            "[default]T.alarms",
            "[default]Folder/{1}",
            "[default]T.{prop}",
            "[default]T[0].eventScripts",
            "[default]T.Value.x",
            "T..x",
        ] {
            assert!(!tag_path_names_value(p), "{p}");
        }
    }
}
