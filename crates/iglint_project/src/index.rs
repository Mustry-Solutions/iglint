//! The project index: cross-resource facts for reference checks and
//! "unused" rules, built in `O(n)` from per-resource summaries.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ids::{ProjectId, ResourceId};
use crate::kinds::{EventScriptKind, MODULE_IGNITION, MODULE_PERSPECTIVE, ResourceKind};
use crate::library::{LibraryLookup, LibraryTree, ModuleSymbols, SymbolKind};
use crate::perspective::match_page_url;
use crate::summary::{
    DynamicReferences, Facts, MessageTarget, NamedQueryFacts, ResourceSummary, SessionFacts,
    StyleClassFacts, TagFacts, UnknownSite, ViewFacts,
};
use crate::tags::TagType;
use crate::view::SESSION_PROPS;
use crate::workspace::{Container, Existence, Workspace};

/// Result of looking up a resource by path in a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<'i, T> {
    /// The resource exists. `facts` is `None` when its content could not be
    /// read (invalid JSON, missing data file).
    Found {
        resource: ResourceId,
        facts: Option<&'i T>,
    },
    Missing,
    /// Not in the workspace, but it may exist (missing parent project, or
    /// provided by a descendant of an inheritable project).
    Unknown,
}

impl<T> Lookup<'_, T> {
    /// Whether the resource exists.
    pub fn existence(&self) -> Existence {
        match self {
            Lookup::Found { .. } => Existence::Exists,
            Lookup::Missing => Existence::Missing,
            Lookup::Unknown => Existence::Unknown,
        }
    }

    /// The resource, when found.
    pub fn resource(&self) -> Option<ResourceId> {
        match self {
            Lookup::Found { resource, .. } => Some(*resource),
            _ => None,
        }
    }
}

/// Where a message type is handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandlerSite {
    /// A component message handler in a view.
    Component {
        view: ResourceId,
        page_scope: bool,
        session_scope: bool,
        view_scope: bool,
    },
    /// A Perspective session message handler resource.
    Session(ResourceId),
    /// A gateway message handler resource.
    Gateway(ResourceId),
    /// A Vision client message handler resource.
    Client(ResourceId),
}

/// One literal message send, as the handlers see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMessage {
    /// `system.perspective.sendMessage` (component handlers) rather than
    /// `system.util.sendMessage`/`sendRequest` (gateway, session and client
    /// handlers).
    pub perspective: bool,
    /// Top-level payload keys when the payload is a dict literal.
    pub payload_keys: Option<Vec<String>>,
}

/// A page matched by URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageMatch {
    pub page_config: ResourceId,
    /// The page URL pattern.
    pub url: String,
    pub view_path: Option<String>,
    /// `:name` captures.
    pub params: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct ProjectData {
    library: LibraryTree,
    handlers: FxHashMap<String, Vec<HandlerSite>>,
    page_config: Option<ResourceId>,
    session_props: Option<ResourceId>,
    /// Classes defined in the project stylesheet.
    css_classes: FxHashSet<String>,
    /// True when the stylesheet exists but could not be read.
    css_unknown: bool,
    /// Message types sent with a target in this project.
    sent: FxHashMap<String, u32>,
    /// The literal sends of each message type that target this project.
    sends: FxHashMap<String, Vec<SentMessage>>,
    /// References computed at run time by resources this project sees
    /// (and by gateway resources, which may run with any project).
    dynamic: DynamicReferences,
    /// Library modules whose every top-level name may be used (a module
    /// used as a value, `from m import *`).
    escaped_modules: FxHashSet<ResourceId>,
    /// Escaped modules that some resource uses as a value (not only
    /// star-imports).
    value_modules: FxHashSet<ResourceId>,
    /// Library modules that a resource this project sees star-imports.
    star_imported_modules: FxHashSet<ResourceId>,
    /// Library names that resources this project sees assign or delete
    /// (`References::library_stores`).
    library_stores: FxHashSet<String>,
    /// Library modules whose names code may rebind through an object it
    /// receives (`References::library_exposed`).
    exposed_modules: FxHashSet<ResourceId>,
    /// A resource this project sees uses code that is not vetted
    /// (`References::unknown_code`, or an unvetted import that is not a
    /// library module of the project).
    unknown_code: bool,
    complete: bool,
    inheritable: bool,
    descendants: Vec<ProjectId>,
}

/// One UDT definition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UdtInfo {
    /// Definition path, e.g. `Motors/Motor`.
    pub path: String,
    /// `typeId` of the parent definition.
    pub parent: Option<String>,
    pub parameters: Vec<String>,
    /// Member paths relative to the definition (lowercase not applied).
    pub members: Vec<String>,
    pub member_instances: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct ProviderData {
    /// Lowercase paths of tags and folders.
    paths: FxHashSet<String>,
    /// Lowercase instance path -> type id.
    instances: FxHashMap<String, String>,
    /// Lowercase definition path -> definition.
    udts: FxHashMap<String, UdtInfo>,
    has_definitions: bool,
}

/// Cross-resource facts of a workspace.
#[derive(Debug)]
pub struct ProjectIndex<'w> {
    ws: &'w Workspace,
    summaries: Vec<Option<ResourceSummary>>,
    projects: Vec<ProjectData>,
    providers: FxHashMap<String, ProviderData>,
    /// Lowercase provider names that are not standard (remote, managed).
    opaque_providers: FxHashSet<String>,
    history_providers: FxHashSet<String>,
    has_history_providers: bool,
    theme_classes: FxHashSet<String>,
    has_themes: bool,
    databases: FxHashSet<String>,
    has_databases: bool,
    usage: FxHashMap<ResourceId, u32>,
    symbol_usage: FxHashMap<(ResourceId, String), u32>,
    /// Like `symbol_usage`, counting only references from other resources.
    external_symbol_usage: FxHashMap<(ResourceId, String), u32>,
    /// The first use (by path and offset) of code that is not vetted, or
    /// that reads a frame or the interpreter, in the workspace.
    first_unknown: Option<UnknownSite>,
    /// Some resource of the workspace may read a file a fix edits
    /// (`References::reads_files`, or an import that is not vetted and not
    /// a library module), and the first such site by path and offset.
    reads_files: bool,
    first_file_read: Option<UnknownSite>,
    /// Code of a gateway collection is not vetted or reads frames, in a
    /// workspace without projects (with projects it counts for each).
    gateway_unknown_code: bool,
    /// No workspace root was found around a linted path, so the closed
    /// world is not known ([`ProjectIndex::mark_closed_world_unknown`]).
    closed_world_unknown: bool,
    /// The names the project declares vetted.
    vetted: Vetted,
}

/// Names that are not in the measured 8.3 lists but that the project
/// declares vetted (`[fix] vetted-system-functions` and
/// `vetted-expression-functions`): checking that they cannot evaluate
/// code, read a frame or read workspace files is the user's
/// responsibility. Default empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Vetted {
    /// `system.*` paths; an entry also vets the members read from it.
    pub system_functions: Vec<String>,
    /// Expression function names, matched case-insensitively as Ignition
    /// resolves them.
    pub expression_functions: Vec<String>,
}

impl Vetted {
    /// Whether the project vets `name`.
    pub fn vets(&self, name: &crate::summary::UnvettedName) -> bool {
        match name {
            crate::summary::UnvettedName::System(path) => {
                iglint_stubs::system_path_listed(path, &self.system_functions)
            }
            crate::summary::UnvettedName::ExpressionFunction(f) => self
                .expression_functions
                .iter()
                .any(|v| v.eq_ignore_ascii_case(f)),
        }
    }

    /// The first site of `refs` that uses a name the project does not vet
    /// ([`crate::summary::References::unvetted_names`]).
    pub fn first_unvetted<'r>(
        &self,
        refs: &'r crate::summary::References,
    ) -> Option<&'r UnknownSite> {
        refs.unvetted_names
            .iter()
            .find(|(name, _)| !self.vets(name))
            .map(|(_, site)| site)
    }
}

/// The answers of [`ProjectIndex::module_may_hold_shared`] and
/// [`ProjectIndex::module_imports_shared`] while the index is built, and
/// the modules whose answer is being computed.
struct SharedMemo {
    holds: FxHashMap<(ProjectId, ResourceId), bool>,
    imports: FxHashMap<(ProjectId, ResourceId), bool>,
    /// The modules being computed, with their depth.
    computing: FxHashMap<ResourceId, usize>,
    /// The smallest depth of a module being computed that the current
    /// computation relied on (`usize::MAX`: none).
    low: usize,
}

impl Default for SharedMemo {
    fn default() -> Self {
        SharedMemo {
            holds: FxHashMap::default(),
            imports: FxHashMap::default(),
            computing: FxHashMap::default(),
            low: usize::MAX,
        }
    }
}

/// Lowercases ASCII and non-ASCII letters (tag paths are case-insensitive).
fn fold(s: &str) -> String {
    s.to_lowercase()
}

impl<'w> ProjectIndex<'w> {
    /// Builds the index. `summaries` may come in any order; resources
    /// without a summary are treated as existing with unknown content.
    pub fn build(ws: &'w Workspace, summaries: Vec<ResourceSummary>) -> ProjectIndex<'w> {
        Self::build_with(ws, summaries, Vetted::default())
    }

    /// [`ProjectIndex::build`] for a project that declares the names of
    /// `vetted` vetted.
    pub fn build_with(
        ws: &'w Workspace,
        summaries: Vec<ResourceSummary>,
        vetted: Vetted,
    ) -> ProjectIndex<'w> {
        let mut by_id: Vec<Option<ResourceSummary>> =
            (0..ws.resources.len()).map(|_| None).collect();
        for s in summaries {
            let i = s.resource.index();
            if i < by_id.len() {
                by_id[i] = Some(s);
            }
        }
        let mut index = ProjectIndex {
            ws,
            summaries: by_id,
            projects: Vec::with_capacity(ws.projects.len()),
            providers: FxHashMap::default(),
            opaque_providers: FxHashSet::default(),
            history_providers: FxHashSet::default(),
            has_history_providers: false,
            theme_classes: FxHashSet::default(),
            has_themes: false,
            first_unknown: None,
            reads_files: false,
            first_file_read: None,
            gateway_unknown_code: false,
            closed_world_unknown: false,
            vetted,
            databases: FxHashSet::default(),
            has_databases: false,
            usage: FxHashMap::default(),
            symbol_usage: FxHashMap::default(),
            external_symbol_usage: FxHashMap::default(),
        };
        index.build_projects();
        index.build_gateway();
        index.count_usage();
        index
    }

    fn build_projects(&mut self) {
        for p in &self.ws.projects {
            let container = Container::Project(p.id);
            let mut data = ProjectData {
                complete: p.inheritance.complete,
                inheritable: p.inheritable(),
                descendants: self.ws.project_descendants(p.id),
                ..ProjectData::default()
            };
            let effective = self.ws.effective_resources(container);
            let mut modules = Vec::new();
            for rid in effective {
                let r = self.ws.resource(rid);
                match r.kind {
                    ResourceKind::ScriptLibrary => {
                        if self.ws.resource_file(rid, "code.py").is_some() {
                            modules.push((&*r.path, rid));
                        }
                    }
                    ResourceKind::View => {
                        if let Some(Facts::View(v)) = self.facts(rid) {
                            for h in &v.message_handlers {
                                data.handlers
                                    .entry(h.message_type.clone())
                                    .or_default()
                                    .push(HandlerSite::Component {
                                        view: rid,
                                        page_scope: h.page_scope,
                                        session_scope: h.session_scope,
                                        view_scope: h.view_scope,
                                    });
                            }
                        }
                    }
                    ResourceKind::EventScript(EventScriptKind::SessionMessage) => {
                        data.handlers
                            .entry(r.path.to_string())
                            .or_default()
                            .push(HandlerSite::Session(rid));
                    }
                    ResourceKind::EventScript(EventScriptKind::GatewayMessage) => {
                        data.handlers
                            .entry(r.path.to_string())
                            .or_default()
                            .push(HandlerSite::Gateway(rid));
                    }
                    ResourceKind::EventScript(EventScriptKind::ClientMessage) => {
                        data.handlers
                            .entry(r.path.to_string())
                            .or_default()
                            .push(HandlerSite::Client(rid));
                    }
                    ResourceKind::PageConfig => data.page_config = Some(rid),
                    ResourceKind::Stylesheet => match self.facts(rid) {
                        Some(Facts::Css(c)) => data.css_classes.extend(c.classes.iter().cloned()),
                        _ => data.css_unknown = true,
                    },
                    ResourceKind::SessionProps => data.session_props = Some(rid),
                    _ => {}
                }
            }
            data.library = LibraryTree::new(modules);
            self.projects.push(data);
        }
    }

    fn build_gateway(&mut self) {
        let ws = self.ws;
        for r in &ws.resources {
            if !matches!(r.container, Container::Collection(_)) || r.manifest.missing {
                continue;
            }
            match r.kind {
                ResourceKind::TagProvider => {
                    let provider = fold(&r.path);
                    if let Some(Facts::Gateway(g)) = self.facts_raw(r.id)
                        && g.provider_type.as_deref().is_some_and(|t| t != "STANDARD")
                    {
                        self.opaque_providers.insert(provider.clone());
                    }
                    self.providers.entry(provider).or_default();
                }
                ResourceKind::PerspectiveAsset if &*r.type_id == "themes" => {
                    self.has_themes = true;
                    if let Some(Some(s)) = self.summaries.get(r.id.index())
                        && let Facts::Css(c) = &s.facts
                    {
                        self.theme_classes.extend(c.classes.iter().cloned());
                    }
                }
                ResourceKind::HistorianProvider => {
                    self.has_history_providers = true;
                    self.history_providers.insert(fold(r.name()));
                }
                ResourceKind::DatabaseConnection => {
                    self.has_databases = true;
                    self.databases.insert(fold(r.name()));
                }
                ResourceKind::TagDefinition | ResourceKind::TagTypeDefinition => {
                    let Some(Facts::Tags(t)) =
                        self.summaries[r.id.index()].as_ref().map(|s| &s.facts)
                    else {
                        continue;
                    };
                    let t: TagFacts = t.clone();
                    let provider = self.providers.entry(fold(&t.provider)).or_default();
                    provider.has_definitions |= !t.definitions;
                    if !t.definitions {
                        // The folder itself and its ancestors.
                        let mut prefix = String::new();
                        for seg in t.folder.split('/').filter(|s| !s.is_empty()) {
                            if !prefix.is_empty() {
                                prefix.push('/');
                            }
                            prefix.push_str(seg);
                            provider.paths.insert(fold(&prefix));
                        }
                    }
                    for tag in &t.tags {
                        if !tag.listed {
                            continue;
                        }
                        if t.definitions {
                            if tag.tag_type == TagType::UdtType {
                                provider.udts.insert(
                                    fold(&tag.path),
                                    UdtInfo {
                                        path: tag.path.clone(),
                                        parent: tag.type_id.clone(),
                                        parameters: tag.parameters.clone(),
                                        members: tag.members.clone(),
                                        member_instances: tag.member_instances.clone(),
                                    },
                                );
                            }
                        } else {
                            let key = fold(&tag.path);
                            if tag.tag_type == TagType::UdtInstance
                                && let Some(type_id) = &tag.type_id
                            {
                                provider.instances.insert(key.clone(), type_id.clone());
                            }
                            provider.paths.insert(key);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn facts_raw(&self, id: ResourceId) -> Option<&Facts> {
        self.summaries.get(id.index())?.as_ref().map(|s| &s.facts)
    }

    fn count_usage(&mut self) {
        let ws = self.ws;
        let mut usage: FxHashMap<ResourceId, u32> = FxHashMap::default();
        let mut symbol_usage: FxHashMap<(ResourceId, String), u32> = FxHashMap::default();
        let mut external_symbol_usage: FxHashMap<(ResourceId, String), u32> = FxHashMap::default();
        let n = ws.projects.len();
        let mut sent: Vec<FxHashMap<String, u32>> = vec![FxHashMap::default(); n];
        let mut sends: Vec<FxHashMap<String, Vec<SentMessage>>> = vec![FxHashMap::default(); n];
        let mut dynamic: Vec<DynamicReferences> = vec![DynamicReferences::default(); n];
        let mut escaped: Vec<FxHashSet<ResourceId>> = vec![FxHashSet::default(); n];
        let mut value_modules: Vec<FxHashSet<ResourceId>> = vec![FxHashSet::default(); n];
        let mut stores: Vec<FxHashSet<String>> = vec![FxHashSet::default(); n];
        let mut exposed: Vec<FxHashSet<ResourceId>> = vec![FxHashSet::default(); n];
        let mut star_imported: Vec<FxHashSet<ResourceId>> = vec![FxHashSet::default(); n];
        let mut unknown_code: Vec<bool> = vec![false; n];
        let mut first_unknown: Option<UnknownSite> = None;
        let mut memo = SharedMemo::default();
        // Resources of gateway collections (tag event scripts) run with the
        // gateway scripting project, which the workspace does not name:
        // their references count for every project.
        let gateway: Vec<ResourceId> = ws
            .resources
            .iter()
            .filter(|r| matches!(r.container, Container::Collection(_)) && !r.manifest.missing)
            .map(|r| r.id)
            .collect();
        for p in &ws.projects {
            let pid = p.id;
            let mut seen = ws.effective_resources(Container::Project(pid));
            seen.extend(gateway.iter().copied());
            for rid in seen {
                let Some(summary) = self.summaries[rid.index()].as_ref() else {
                    continue;
                };
                let refs = &summary.references;
                for v in &refs.views {
                    if let Some(t) = self.view(pid, v).resource() {
                        *usage.entry(t).or_default() += 1;
                    }
                }
                for s in &refs.style_classes {
                    if let Some(t) = self.style_class(pid, s).resource() {
                        *usage.entry(t).or_default() += 1;
                    }
                }
                for q in &refs.named_queries {
                    if let Some(t) = self.named_query(pid, q).resource() {
                        *usage.entry(t).or_default() += 1;
                    }
                }
                for name in &refs.library {
                    match self.resolve_library(pid, name) {
                        LibraryLookup::Symbol {
                            resource, symbol, ..
                        } => {
                            *usage.entry(resource).or_default() += 1;
                            *symbol_usage
                                .entry((resource, symbol.name.clone()))
                                .or_default() += 1;
                            if resource != rid {
                                *external_symbol_usage
                                    .entry((resource, symbol.name.clone()))
                                    .or_default() += 1;
                            }
                        }
                        LibraryLookup::Module { resource, .. } => {
                            *usage.entry(resource).or_default() += 1;
                        }
                        _ => {}
                    }
                }
                let unvetted = refs.unvetted_imports.iter().find(|(name, _)| {
                    !matches!(
                        self.resolve_library(pid, name),
                        LibraryLookup::Module { .. }
                            | LibraryLookup::Package { .. }
                            | LibraryLookup::Symbol { .. }
                    )
                });
                let unvetted_name = self.vetted.first_unvetted(refs);
                let shared = self.shared_object_site_with(pid, refs, &mut memo);
                if refs.unknown_code
                    || unvetted.is_some()
                    || unvetted_name.is_some()
                    || shared.is_some()
                {
                    unknown_code[pid.index()] = true;
                    let sites = refs
                        .unknown_site
                        .iter()
                        .filter(|_| refs.unknown_code)
                        .chain(unvetted.map(|(_, site)| site))
                        .chain(unvetted_name)
                        .chain(shared);
                    for site in sites {
                        let key = |s: &UnknownSite| (ws.file(s.file).path.clone(), s.range.start());
                        if site.file != UnknownSite::DETACHED
                            && first_unknown
                                .as_ref()
                                .is_none_or(|f: &UnknownSite| key(site) < key(f))
                        {
                            first_unknown = Some(site.clone());
                        }
                    }
                }
                for name in &refs.dynamic.library {
                    let library = &self.projects[pid.index()].library;
                    let prefix = match self.resolve_library(pid, name) {
                        LibraryLookup::Module { resource, .. } => {
                            escaped[pid.index()].insert(resource);
                            // A star import records the module once; any
                            // other dynamic reference is a use as a value.
                            let star = refs
                                .library_star_imports
                                .iter()
                                .filter(|m| *m == name)
                                .count();
                            let dynamic =
                                refs.dynamic.library.iter().filter(|m| *m == name).count();
                            if dynamic > star {
                                value_modules[pid.index()].insert(resource);
                            }
                            continue;
                        }
                        LibraryLookup::Package { dotted } => format!("{dotted}."),
                        _ if name.is_empty() => String::new(),
                        _ => continue,
                    };
                    for (dotted, resource) in library.modules() {
                        if dotted.starts_with(&prefix) {
                            escaped[pid.index()].insert(resource);
                        }
                    }
                }
                dynamic[pid.index()].extend(&refs.dynamic);
                stores[pid.index()].extend(refs.library_stores.iter().cloned());
                for name in &refs.library_exposed {
                    // Through imports to the module that defines the object.
                    let mut dotted = name.clone();
                    for _ in 0..8 {
                        match self.resolve_library(pid, &dotted) {
                            LibraryLookup::Symbol {
                                resource, symbol, ..
                            } => match &symbol.kind {
                                // A function or class (or a method of it)
                                // reaches the module through `func_globals`.
                                // A variable's value, if a function, was
                                // itself named where it was stored.
                                SymbolKind::Function(_) | SymbolKind::Class => {
                                    exposed[pid.index()].insert(resource);
                                    break;
                                }
                                SymbolKind::Import {
                                    target: Some(target),
                                } => dotted = target.clone(),
                                _ => break,
                            },
                            LibraryLookup::Module { resource, .. } => {
                                exposed[pid.index()].insert(resource);
                                break;
                            }
                            LibraryLookup::Package { dotted: package } => {
                                let prefix = format!("{package}.");
                                let library = &self.projects[pid.index()].library;
                                for (m, resource) in library.modules() {
                                    if m.starts_with(&prefix) {
                                        exposed[pid.index()].insert(resource);
                                    }
                                }
                                break;
                            }
                            _ => break,
                        }
                    }
                }
                for name in &refs.library_star_imports {
                    if let LibraryLookup::Module { resource, .. } = self.resolve_library(pid, name)
                    {
                        star_imported[pid.index()].insert(resource);
                    }
                }
                for m in &refs.messages {
                    let (target_project, perspective) = match &m.target {
                        MessageTarget::Perspective { .. } => (Some(pid), true),
                        MessageTarget::Gateway {
                            project: Some(name),
                        } => (
                            ws.projects.iter().find(|q| &q.name == name).map(|q| q.id),
                            false,
                        ),
                        MessageTarget::Gateway { project: None } => (None, false),
                    };
                    let sent_message = SentMessage {
                        perspective,
                        payload_keys: m.payload_keys.clone(),
                    };
                    let targets: Vec<usize> = match target_project {
                        Some(t) => vec![t.index()],
                        None => (0..n).collect(),
                    };
                    for t in targets {
                        *sent[t].entry(m.message_type.clone()).or_default() += 1;
                        let list = sends[t].entry(m.message_type.clone()).or_default();
                        if !list.contains(&sent_message) {
                            list.push(sent_message.clone());
                        }
                    }
                }
            }
        }
        // Gateway message sends with a non-literal or unknown target project
        // reach every project.
        let shared: Vec<(Option<String>, String)> = dynamic
            .iter()
            .flat_map(|d| d.gateway_messages.iter().cloned())
            .collect();
        for (i, (p, s)) in self.projects.iter_mut().zip(sent).enumerate() {
            p.sent = s;
            p.sends = std::mem::take(&mut sends[i]);
            let mut d = std::mem::take(&mut dynamic[i]);
            for (target, prefix) in &shared {
                let applies = target.as_ref().is_none_or(|t| *t == ws.projects[i].name);
                let entry = (target.clone(), prefix.clone());
                if applies && !d.gateway_messages.contains(&entry) {
                    d.gateway_messages.push(entry);
                }
            }
            p.dynamic = d;
            p.escaped_modules = std::mem::take(&mut escaped[i]);
            p.value_modules = std::mem::take(&mut value_modules[i]);
            p.library_stores = std::mem::take(&mut stores[i]);
            p.exposed_modules = std::mem::take(&mut exposed[i]);
            p.star_imported_modules = std::mem::take(&mut star_imported[i]);
            p.unknown_code = unknown_code[i];
        }
        if ws.projects.is_empty() {
            for rid in &gateway {
                let Some(summary) = self.summaries[rid.index()].as_ref() else {
                    continue;
                };
                let refs = &summary.references;
                let unvetted = refs.unvetted_imports.first();
                let unvetted_name = self.vetted.first_unvetted(refs);
                if !refs.unknown_code && unvetted.is_none() && unvetted_name.is_none() {
                    continue;
                }
                self.gateway_unknown_code = true;
                let sites = refs
                    .unknown_site
                    .iter()
                    .filter(|_| refs.unknown_code)
                    .chain(unvetted.map(|(_, site)| site))
                    .chain(unvetted_name);
                for site in sites {
                    let key = |s: &UnknownSite| (ws.file(s.file).path.clone(), s.range.start());
                    if site.file != UnknownSite::DETACHED
                        && first_unknown
                            .as_ref()
                            .is_none_or(|f: &UnknownSite| key(site) < key(f))
                    {
                        first_unknown = Some(site.clone());
                    }
                }
            }
        }
        self.first_unknown = first_unknown;
        self.count_file_reads();
        self.usage = usage;
        self.symbol_usage = symbol_usage;
        self.external_symbol_usage = external_symbol_usage;
    }

    // ---- generic access ----

    /// The workspace the index was built from.
    pub fn workspace(&self) -> &'w Workspace {
        self.ws
    }

    /// Takes the summaries back, in resource id order, so that a caller that
    /// keeps them between runs (the language server) can update one and
    /// rebuild the index without copying the others.
    pub fn into_summaries(self) -> Vec<ResourceSummary> {
        self.summaries.into_iter().flatten().collect()
    }

    /// The summary of a resource.
    pub fn summary(&self, id: ResourceId) -> Option<&ResourceSummary> {
        self.summaries.get(id.index())?.as_ref()
    }

    fn facts(&self, id: ResourceId) -> Option<&Facts> {
        let s = self.summary(id)?;
        if s.valid { Some(&s.facts) } else { None }
    }

    /// How many literal references (from every project that sees the
    /// referencing resource) point at a resource.
    pub fn usage_count(&self, id: ResourceId) -> u32 {
        self.usage.get(&id).copied().unwrap_or(0)
    }

    /// How many literal references point at a library symbol.
    /// How many references from other resources resolve to the top-level
    /// `name` of library `module`.
    pub fn external_symbol_usage_count(&self, module: ResourceId, name: &str) -> u32 {
        self.external_symbol_usage
            .get(&(module, name.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    pub fn symbol_usage_count(&self, module: ResourceId, name: &str) -> u32 {
        self.symbol_usage
            .get(&(module, name.to_owned()))
            .copied()
            .unwrap_or(0)
    }

    /// Looks up an effective resource; `Missing` becomes `Unknown` when the
    /// project's ancestry is incomplete or a descendant of an inheritable
    /// project provides it.
    fn lookup<T>(
        &self,
        project: ProjectId,
        module: &str,
        type_id: &str,
        path: &str,
        get: impl Fn(&Facts) -> Option<&T>,
    ) -> Lookup<'_, T> {
        let container = Container::Project(project);
        if let Some(rid) = self.ws.effective_resource(container, module, type_id, path) {
            return Lookup::Found {
                resource: rid,
                facts: self.facts(rid).and_then(get),
            };
        }
        let data = &self.projects[project.index()];
        if !data.complete {
            return Lookup::Unknown;
        }
        if data.inheritable
            && data.descendants.iter().any(|d| {
                self.ws
                    .effective_resource(Container::Project(*d), module, type_id, path)
                    .is_some()
            })
        {
            return Lookup::Unknown;
        }
        Lookup::Missing
    }

    // ---- Perspective ----

    /// A view by path (`Popups/MotorDetail`), with its parameters.
    pub fn view(&self, project: ProjectId, path: &str) -> Lookup<'_, ViewFacts> {
        let path = path.trim_start_matches('/');
        self.lookup(project, MODULE_PERSPECTIVE, "views", path, |f| match f {
            Facts::View(v) => Some(v),
            _ => None,
        })
    }

    /// A style class by path (`Brand/Primary`).
    pub fn style_class(&self, project: ProjectId, path: &str) -> Lookup<'_, StyleClassFacts> {
        self.lookup(
            project,
            MODULE_PERSPECTIVE,
            "style-classes",
            path,
            |f| match f {
                Facts::StyleClass(s) => Some(s),
                _ => None,
            },
        )
    }

    /// Whether a name in a `classes` list resolves. Perspective renders
    /// each name as the CSS class `psc-<name>`, so besides a style class
    /// resource (`Brand/Primary`) the name may be styled by a `.psc-<name>`
    /// rule in the project stylesheet or a gateway theme. When the gateway
    /// themes are not in the workspace, names without a resource are
    /// `Unknown`.
    pub fn style_class_reference(&self, project: ProjectId, name: &str) -> Existence {
        let lookup = self.style_class(project, name).existence();
        if lookup != Existence::Missing {
            return lookup;
        }
        let css = format!("psc-{name}");
        let data = &self.projects[project.index()];
        if data.css_classes.contains(&css) || self.theme_classes.contains(&css) {
            return Existence::Exists;
        }
        if data.css_unknown || !self.has_themes {
            return Existence::Unknown;
        }
        Existence::Missing
    }

    /// The session properties of a project.
    pub fn session_props(&self, project: ProjectId) -> Option<&SessionFacts> {
        let rid = self.projects[project.index()].session_props?;
        match self.facts(rid)? {
            Facts::SessionProps(s) => Some(s),
            _ => None,
        }
    }

    /// Whether `session.<category>.<name>` exists. `custom` is checked
    /// against session props; `props` against the built-in keys and the
    /// keys written in `props.json` (other keys are `Unknown`).
    pub fn session_property(&self, project: ProjectId, category: &str, name: &str) -> Existence {
        match category {
            "custom" => {
                let data = &self.projects[project.index()];
                match self.session_props(project) {
                    Some(s) if s.custom.iter().any(|c| c == name) => Existence::Exists,
                    Some(_) if data.complete && !data.inheritable => Existence::Missing,
                    Some(_) => Existence::Unknown,
                    None if data.session_props.is_none() && data.complete && !data.inheritable => {
                        Existence::Missing
                    }
                    None => Existence::Unknown,
                }
            }
            "props" => {
                if SESSION_PROPS.contains(&name)
                    || self
                        .session_props(project)
                        .is_some_and(|s| s.props.iter().any(|p| p == name))
                {
                    Existence::Exists
                } else {
                    Existence::Unknown
                }
            }
            _ => Existence::Unknown,
        }
    }

    /// The page that serves `url` (page config with inheritance).
    pub fn page_for_url(&self, project: ProjectId, url: &str) -> Lookup<'_, PageMatch> {
        let data = &self.projects[project.index()];
        let Some(rid) = data.page_config else {
            return if data.complete && !data.inheritable {
                Lookup::Missing
            } else {
                Lookup::Unknown
            };
        };
        if !matches!(self.facts(rid), Some(Facts::PageConfig(_))) {
            return Lookup::Unknown;
        }
        // Matches are computed; `match_page` returns them. This reports
        // existence only.
        if self.match_page(project, url).is_some() {
            Lookup::Found {
                resource: rid,
                facts: None,
            }
        } else if data.inheritable {
            Lookup::Unknown
        } else {
            Lookup::Missing
        }
    }

    /// The page config entry matching `url`, with `:param` captures.
    pub fn match_page(&self, project: ProjectId, url: &str) -> Option<PageMatch> {
        let rid = self.projects[project.index()].page_config?;
        let Some(Facts::PageConfig(pc)) = self.facts(rid) else {
            return None;
        };
        // Exact patterns before parameterised ones.
        let mut best: Option<PageMatch> = None;
        for page in &pc.pages {
            if let Some(captures) = match_page_url(&page.url, url) {
                let better = match &best {
                    None => true,
                    Some(b) => captures.len() < b.params.len(),
                };
                if better {
                    best = Some(PageMatch {
                        page_config: rid,
                        url: page.url.clone(),
                        view_path: page.view_path.clone(),
                        params: captures
                            .into_iter()
                            .map(|(k, v)| (k, v.to_owned()))
                            .collect(),
                    });
                }
            }
        }
        best
    }

    /// Where a message type is handled in a project (component handlers in
    /// views, session and gateway message handler resources).
    pub fn message_handlers(&self, project: ProjectId, message_type: &str) -> &[HandlerSite] {
        self.projects[project.index()]
            .handlers
            .get(message_type)
            .map_or(&[], Vec::as_slice)
    }

    /// Every handled message type of a project.
    pub fn message_types(
        &self,
        project: ProjectId,
    ) -> impl Iterator<Item = (&str, &[HandlerSite])> {
        self.projects[project.index()]
            .handlers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_slice()))
    }

    /// The literal sends of a message type that target a project.
    pub fn message_sends(&self, project: ProjectId, message_type: &str) -> &[SentMessage] {
        self.projects[project.index()]
            .sends
            .get(message_type)
            .map_or(&[], Vec::as_slice)
    }

    /// References computed at run time by the resources a project sees
    /// (including gateway resources).
    pub fn dynamic_references(&self, project: ProjectId) -> &DynamicReferences {
        &self.projects[project.index()].dynamic
    }

    /// Whether a resource `project` sees uses code that is not vetted
    /// (see `iglint_python_semantic::vetted`): such code may read the
    /// namespace of any library module by name.
    pub fn has_unknown_code(&self, project: ProjectId) -> bool {
        self.projects[project.index()].unknown_code
    }

    /// Whether any project in the workspace sees code that is not vetted or
    /// that reads a frame or the interpreter. The Ignition runtime is
    /// shared across projects and gateway-scoped scripts, so such code
    /// anywhere (`sys._current_frames`, reflection, `cPickle` of an
    /// `eval` opcode) can observe any module's or any frame's names. Safe
    /// fixes that assume non-observability (`SCR102`, `VIEW015`,
    /// `NAME001`) require this to be false: the whole workspace is the
    /// closed world. Projects outside the workspace are covered by the
    /// `inheritable` / incomplete-inheritance checks, which already fail
    /// closed.
    pub fn has_any_unknown_code(&self) -> bool {
        self.closed_world_unknown
            || self.unread_directory().is_some()
            || self.gateway_unknown_code
            || self.projects.iter().any(|p| p.unknown_code)
    }

    /// The names the project declares vetted.
    pub fn vetted(&self) -> &Vetted {
        &self.vetted
    }

    /// Records that the closed world is not known: no workspace root was
    /// found around a linted path (a loose file, a project with nothing
    /// around it that says where the rest of its gateway is). Every fix
    /// that needs the closed world is then unsafe
    /// ([`ProjectIndex::has_any_unknown_code`]).
    pub fn mark_closed_world_unknown(&mut self) {
        self.closed_world_unknown = true;
    }

    /// Whether [`ProjectIndex::mark_closed_world_unknown`] was called.
    pub fn closed_world_unknown(&self) -> bool {
        self.closed_world_unknown
    }

    /// Whether any code of the workspace may read a file that a fix edits
    /// (`References::reads_files`, or an import that is neither vetted nor
    /// a project library module): every fix edits workspace files, so none
    /// is safe while this holds, unless the project declares that scripts
    /// do not read workspace files.
    pub fn may_read_workspace_files(&self) -> bool {
        self.reads_files || self.unread_directory().is_some()
    }

    /// The first directory of the workspace (by path) that could hold
    /// Ignition resources but was not read
    /// ([`crate::workspace::Workspace::unread`]). Its scripts are unknown,
    /// so while it exists the closed world is unknown
    /// ([`ProjectIndex::has_any_unknown_code`]) and code in it may read
    /// the files a fix edits ([`ProjectIndex::may_read_workspace_files`]).
    pub fn unread_directory(&self) -> Option<&crate::workspace::WorkspaceProblem> {
        self.ws.unread.first()
    }

    /// The first site (by path, then offset) of
    /// [`ProjectIndex::may_read_workspace_files`].
    pub fn first_file_read(&self) -> Option<&UnknownSite> {
        self.first_file_read.as_ref()
    }

    fn count_file_reads(&mut self) {
        let ws = self.ws;
        let key = |s: &UnknownSite| (ws.file(s.file).path.clone(), s.range.start());
        let mut reads = false;
        let mut first: Option<UnknownSite> = None;
        let mut memo = SharedMemo::default();
        for r in &ws.resources {
            let Some(summary) = self.summaries[r.id.index()].as_ref() else {
                continue;
            };
            let refs = &summary.references;
            // An import of a module that is neither vetted nor a library
            // module of every project that may run the resource.
            let projects: Vec<ProjectId> = match r.container {
                Container::Project(p) => {
                    let mut out = vec![p];
                    out.extend(self.projects[p.index()].descendants.iter().copied());
                    out
                }
                Container::Collection(_) => ws.projects.iter().map(|p| p.id).collect(),
            };
            let unvetted = refs.unvetted_imports.iter().find(|(name, _)| {
                projects.is_empty()
                    || projects.iter().any(|p| {
                        !matches!(
                            self.resolve_library(*p, name),
                            LibraryLookup::Module { .. }
                                | LibraryLookup::Package { .. }
                                | LibraryLookup::Symbol { .. }
                        )
                    })
            });
            // A name that is not vetted may read files too, and so may
            // code that changes a shared object (like vetted code's).
            let unvetted_name = self.vetted.first_unvetted(refs);
            let shared = projects
                .iter()
                .find_map(|p| self.shared_object_site_with(*p, refs, &mut memo));
            if !refs.reads_files
                && unvetted.is_none()
                && unvetted_name.is_none()
                && shared.is_none()
            {
                continue;
            }
            reads = true;
            let sites = refs
                .file_read_site
                .iter()
                .filter(|_| refs.reads_files)
                .chain(unvetted.map(|(_, site)| site))
                .chain(unvetted_name)
                .chain(shared);
            for site in sites {
                if site.file != UnknownSite::DETACHED
                    && first.as_ref().is_none_or(|f| key(site) < key(f))
                {
                    first = Some(site.clone());
                }
            }
        }
        self.reads_files = reads;
        self.first_file_read = first;
    }

    /// The first change of a shared object, or the first use of one as a
    /// value, that `refs` makes through a library name of `project`
    /// ([`crate::summary::References::library_changes`] and
    /// [`crate::summary::References::library_values`]): the name reaches,
    /// through an import of a library module, something that is not a
    /// library module (a vetted module, a Java class, `system`), or a
    /// library module whose names cannot be known. Such code is unknown
    /// code: the measured facts about the object hold only while no code
    /// changes it ("Shared objects" in `docs/internal/fix-safety.md`).
    /// Changes of library modules themselves are the library analysis'
    /// business ([`ProjectIndex::library_name_stored`]).
    pub fn shared_object_site<'r>(
        &self,
        project: ProjectId,
        refs: &'r crate::summary::References,
    ) -> Option<&'r UnknownSite> {
        self.shared_object_site_with(project, refs, &mut SharedMemo::default())
    }

    /// [`ProjectIndex::shared_object_site`] with the answers of earlier
    /// calls (the index builds it for every resource with one memo).
    fn shared_object_site_with<'r>(
        &self,
        project: ProjectId,
        refs: &'r crate::summary::References,
        memo: &mut SharedMemo,
    ) -> Option<&'r UnknownSite> {
        let changed = refs
            .library_changes
            .iter()
            .find(|(object, _)| self.reaches_shared_from(project, object, false, memo));
        let value = refs
            .library_values
            .iter()
            .find(|(object, _)| self.reaches_shared_from(project, object, true, memo));
        changed
            .into_iter()
            .chain(value)
            .map(|(_, site)| site)
            .min_by_key(|s| s.range.start())
    }

    /// Whether the library name `dotted` may stand for an object that is
    /// not a library module or a name a library module defines: a name a
    /// library module imports from elsewhere, or anything under it, or a
    /// name in a module whose names are not known. With `whole`, a library
    /// module or package (used as a value) counts when any of its modules
    /// imports such a name: whoever holds the module reaches it. A
    /// function, class or variable a library module defines counts when
    /// the module may store such an object where it is reachable
    /// ([`ProjectIndex::module_may_hold_shared`]): `del l.K.math.pi` after
    /// `class K: import math` in `l`.
    ///
    /// `memo` keeps the answers of [`ProjectIndex::module_may_hold_shared`]
    /// and [`ProjectIndex::module_imports_shared`] (review round 25, M3:
    /// recomputed for every library value, the index took time cubic in
    /// the number of top-level names of a module).
    fn reaches_shared_from(
        &self,
        project: ProjectId,
        dotted: &str,
        whole: bool,
        memo: &mut SharedMemo,
    ) -> bool {
        let mut dotted = dotted.to_owned();
        for _ in 0..8 {
            match self.resolve_library(project, &dotted) {
                LibraryLookup::NotLibrary
                | LibraryLookup::Missing { .. }
                | LibraryLookup::HiddenByModule { .. }
                | LibraryLookup::MissingAttribute { .. } => return false,
                LibraryLookup::Unknown => return true,
                LibraryLookup::Module { resource, .. } => {
                    return whole && self.module_imports_shared(project, resource, memo);
                }
                LibraryLookup::Package { dotted: package } => {
                    if !whole {
                        return false;
                    }
                    let prefix = format!("{package}.");
                    let library = &self.projects[project.index()].library;
                    return library.modules().any(|(m, resource)| {
                        m.starts_with(&prefix)
                            && self.module_imports_shared(project, resource, memo)
                    });
                }
                LibraryLookup::Symbol {
                    resource,
                    symbol,
                    rest,
                } => match &symbol.kind {
                    SymbolKind::Import {
                        target: Some(target),
                    } => {
                        // Follow the import, with the rest of the name.
                        let tail: Vec<&str> = dotted.split('.').collect();
                        let rest = &tail[tail.len() - rest..];
                        let next = std::iter::once(target.as_str())
                            .chain(rest.iter().copied())
                            .collect::<Vec<_>>()
                            .join(".");
                        if matches!(
                            self.resolve_library(project, target),
                            LibraryLookup::NotLibrary
                                | LibraryLookup::Missing { .. }
                                | LibraryLookup::HiddenByModule { .. }
                                | LibraryLookup::MissingAttribute { .. }
                        ) {
                            // `import math` in the library module: a change
                            // through it changes a shared object; a value
                            // is one when it is the module, a Java class or
                            // anything else than a vetted value
                            // (`lib.os.sep` is a string).
                            return !whole
                                || iglint_python_semantic::vetted::holds_shared_object(&next);
                        }
                        dotted = next;
                    }
                    SymbolKind::Import { target: None } => return true,
                    // A function, class or variable of the module: what
                    // its attributes, instances or items hold is the
                    // module's own code.
                    _ => return self.module_may_hold_shared(project, resource, memo),
                },
            }
        }
        true
    }

    /// Whether the code of the library module `module` may store an object
    /// that is not a library module (a module, a Java class, `system`,
    /// anything not vetted) where another module reaches it through one of
    /// its names: in a class body (`class K: import math`), an attribute
    /// (`K.m = math`), an instance, a container or a default argument.
    /// The vetting walker makes each such store unknown code
    /// (`vetted::unknown_code`, "Shared objects" in
    /// `docs/internal/fix-safety.md`); a library module or library name
    /// stored that way is one of the module's library values, which hold
    /// such an object when [`ProjectIndex::reaches_shared`] says so. A
    /// module whose code or names are not known holds one.
    ///
    /// A module whose answer is being computed counts as `false` (a cycle
    /// of library values adds no object the other modules of the cycle do
    /// not already reach). An answer is kept in `memo` once it is final:
    /// `true`, or `false` without relying on a module further up that is
    /// still being computed (whose answer could still turn it `true`).
    fn module_may_hold_shared(
        &self,
        project: ProjectId,
        module: ResourceId,
        memo: &mut SharedMemo,
    ) -> bool {
        if let Some(known) = memo.holds.get(&(project, module)) {
            return *known;
        }
        if let Some(depth) = memo.computing.get(&module) {
            memo.low = memo.low.min(*depth);
            return false;
        }
        let (Some(symbols), Some(summary)) = (self.module_symbols(module), self.summary(module))
        else {
            memo.holds.insert((project, module), true);
            return true;
        };
        let depth = memo.computing.len();
        memo.computing.insert(module, depth);
        let outer = std::mem::replace(&mut memo.low, usize::MAX);
        let refs = &summary.references;
        let holds = symbols.unknown_code
            || symbols.dynamic
            || refs.unknown_code
            || refs
                .library_values
                .iter()
                .any(|(value, _)| self.reaches_shared_from(project, value, true, memo));
        memo.computing.remove(&module);
        let provisional = !holds && memo.low < depth;
        if !provisional {
            memo.holds.insert((project, module), holds);
        }
        memo.low = if provisional {
            outer.min(memo.low)
        } else {
            outer
        };
        holds
    }

    /// Whether the library module `module` imports a name that is not a
    /// library name, or its names are not known.
    fn module_imports_shared(
        &self,
        project: ProjectId,
        module: ResourceId,
        memo: &mut SharedMemo,
    ) -> bool {
        if let Some(known) = memo.imports.get(&(project, module)) {
            return *known;
        }
        let Some(symbols) = self.module_symbols(module) else {
            return true;
        };
        let shared = symbols.dynamic
            || symbols.symbols.iter().any(|s| match &s.kind {
                SymbolKind::Import { target: Some(t) } => !matches!(
                    self.resolve_library(project, t),
                    LibraryLookup::Module { .. }
                        | LibraryLookup::Package { .. }
                        | LibraryLookup::Symbol { .. }
                ),
                SymbolKind::Import { target: None } => true,
                _ => false,
            });
        memo.imports.insert((project, module), shared);
        shared
    }

    /// The first use (by path, then offset) of code that is not vetted, or
    /// that reads a frame or the interpreter, anywhere in the workspace
    /// ([`ProjectIndex::has_any_unknown_code`]): what explains an unsafe
    /// fix under the closed-world rule.
    pub fn first_unknown_code(&self) -> Option<&UnknownSite> {
        self.first_unknown.as_ref()
    }

    /// Every [`crate::summary::PerspectiveCall`] of the workspace, with each
    /// project whose scripts may make it (the project a resource belongs
    /// to, the projects inheriting it, and every project for gateway
    /// resources), in a stable order.
    pub fn perspective_calls(&self) -> Vec<(ProjectId, crate::summary::PerspectiveCall)> {
        let ws = self.ws;
        let gateway: Vec<ResourceId> = ws
            .resources
            .iter()
            .filter(|r| matches!(r.container, Container::Collection(_)) && !r.manifest.missing)
            .map(|r| r.id)
            .collect();
        let mut out = Vec::new();
        for p in &ws.projects {
            let mut seen = ws.effective_resources(Container::Project(p.id));
            seen.extend(gateway.iter().copied());
            for rid in seen {
                let Some(summary) = self.summaries[rid.index()].as_ref() else {
                    continue;
                };
                for call in &summary.references.perspective_calls {
                    out.push((p.id, call.clone()));
                }
            }
        }
        out
    }

    /// Counts `site` as code that is not vetted in `project` (and so in the
    /// workspace, [`ProjectIndex::has_any_unknown_code`]): a fact the
    /// linter derives after the index is built (a library function that
    /// reaches other views with a Perspective object it is handed).
    pub fn mark_unknown_code(&mut self, project: ProjectId, site: UnknownSite) {
        self.projects[project.index()].unknown_code = true;
        let ws = self.ws;
        let key = |s: &UnknownSite| (ws.file(s.file).path.clone(), s.range.start());
        if site.file != UnknownSite::DETACHED
            && self
                .first_unknown
                .as_ref()
                .is_none_or(|f| key(&site) < key(f))
        {
            self.first_unknown = Some(site);
        }
    }

    /// Whether every top-level name of a library module may be used by
    /// `project` (the module is used as a value or star-imported).
    pub fn module_escapes(&self, project: ProjectId, module: ResourceId) -> bool {
        self.projects[project.index()]
            .escaped_modules
            .contains(&module)
    }

    /// Whether a resource of `project` uses the library module as a value
    /// (`m = core.util`, a default argument, a container, `getGlobals()`,
    /// `sys.modules['core.util']`, a computed `getattr` on it), so that it
    /// may read any of the module's names. Star imports alone do not count
    /// ([`ProjectIndex::module_star_imported`]).
    pub fn module_used_as_value(&self, project: ProjectId, module: ResourceId) -> bool {
        self.projects[project.index()]
            .value_modules
            .contains(&module)
    }

    /// Whether a resource `project` sees star-imports the library module
    /// (`from core.util import *`), so that every name the module exports
    /// to star imports is used.
    pub fn module_star_imported(&self, project: ProjectId, module: ResourceId) -> bool {
        self.projects[project.index()]
            .star_imported_modules
            .contains(&module)
    }

    /// Whether a resource of `project` assigns or deletes the library name
    /// `dotted` (as written, before following imports) or a module or
    /// package on its path (`core.util.fmt = f`, `setattr(core.util,
    /// 'fmt', f)`, `core.util.__dict__[k] = v`).
    pub fn library_name_stored(&self, project: ProjectId, dotted: &str) -> bool {
        let stores = &self.projects[project.index()].library_stores;
        let mut prefix = dotted;
        loop {
            if stores.contains(prefix) {
                return true;
            }
            match prefix.rsplit_once('.') {
                Some((head, _)) => prefix = head,
                None => return false,
            }
        }
    }

    /// Whether the library name `dotted` (as written, before following
    /// imports) may be bound at run time to something other than its
    /// definition by code the module itself does not show: it is stored
    /// ([`ProjectIndex::library_name_stored`]) or its module escapes
    /// ([`ProjectIndex::module_escapes`]: used as a value, reached through
    /// `sys.modules` or code run from strings, so any code may set its
    /// attributes) or is exposed (a function or class of the module is
    /// used as a value, or its `func_globals` is reached: whoever holds it
    /// may replace its code or the module's names). Rebinding by the
    /// module itself is part of its symbols
    /// (a rebound name is a
    /// [`SymbolKind::Variable`](crate::library::SymbolKind::Variable)).
    pub fn library_name_rebound(&self, project: ProjectId, dotted: &str) -> bool {
        if self.library_name_stored(project, dotted) {
            return true;
        }
        match self.resolve_library(project, dotted) {
            LibraryLookup::Symbol { resource, .. }
            | LibraryLookup::MissingAttribute { resource, .. }
            | LibraryLookup::Module { resource, .. } => {
                self.module_escapes(project, resource)
                    || self.projects[project.index()]
                        .exposed_modules
                        .contains(&resource)
            }
            _ => false,
        }
    }

    /// How many literal sends of a message type target a project.
    pub fn messages_sent(&self, project: ProjectId, message_type: &str) -> u32 {
        self.projects[project.index()]
            .sent
            .get(message_type)
            .copied()
            .unwrap_or(0)
    }

    // ---- named queries ----

    /// A named query by path (`Machines/ByLine`).
    pub fn named_query(&self, project: ProjectId, path: &str) -> Lookup<'_, NamedQueryFacts> {
        self.lookup(
            project,
            MODULE_IGNITION,
            "named-query",
            path.trim_start_matches('/'),
            |f| match f {
                Facts::NamedQuery(q) => Some(q),
                _ => None,
            },
        )
    }

    // ---- library ----

    /// The module tree of a project.
    pub fn library(&self, project: ProjectId) -> &LibraryTree {
        &self.projects[project.index()].library
    }

    /// Top-level library names (globals of every script of the project).
    pub fn library_globals(&self, project: ProjectId) -> Vec<&str> {
        let mut names: Vec<&str> = self.projects[project.index()].library.globals().collect();
        names.sort_unstable();
        names
    }

    /// The library names a script of `project` may see as globals: the
    /// project's own top-level library names and, for an inheritable
    /// project, those of its descendants (its scripts may run there). The
    /// flag is false when the list may be incomplete because the project's
    /// ancestry is incomplete (a parent outside the workspace).
    pub fn script_library_globals(&self, project: ProjectId) -> (Vec<&str>, bool) {
        let data = &self.projects[project.index()];
        let mut names: Vec<&str> = data.library.globals().collect();
        let mut complete = data.complete;
        if data.inheritable {
            for d in &data.descendants {
                let child = &self.projects[d.index()];
                names.extend(child.library.globals());
                complete &= child.complete;
            }
        }
        names.sort_unstable();
        names.dedup();
        (names, complete)
    }

    /// The symbols of a library module, when the script layer provided them.
    pub fn module_symbols(&self, module: ResourceId) -> Option<&ModuleSymbols> {
        match self.facts(module)? {
            Facts::Library(l) => l.symbols.as_ref(),
            _ => None,
        }
    }

    /// Resolves `pkg.mod.func` in a project. Missing results are `Unknown`
    /// when the project's ancestry is incomplete.
    pub fn resolve_library(&self, project: ProjectId, dotted: &str) -> LibraryLookup<'_> {
        self.resolve_library_with(project, dotted, false)
    }

    /// [`ProjectIndex::resolve_library`] with the symbols for findings
    /// ([`ModuleSymbols::for_findings`]): a module whose only escape
    /// routes do not observe names is not dynamic there. For rules that
    /// report findings; fixes resolve with [`ProjectIndex::resolve_library`].
    pub fn resolve_library_for_findings(
        &self,
        project: ProjectId,
        dotted: &str,
    ) -> LibraryLookup<'_> {
        self.resolve_library_with(project, dotted, true)
    }

    fn resolve_library_with(
        &self,
        project: ProjectId,
        dotted: &str,
        findings: bool,
    ) -> LibraryLookup<'_> {
        let data = &self.projects[project.index()];
        let result = data.library.resolve(dotted, |r| {
            self.module_symbols(r)
                .map(|s| if findings { s.for_findings() } else { s })
        });
        match result {
            LibraryLookup::Missing { .. }
            | LibraryLookup::MissingAttribute { .. }
            | LibraryLookup::NotLibrary
                if !data.complete =>
            {
                LibraryLookup::Unknown
            }
            r => r,
        }
    }

    // ---- gateway ----

    /// Tag provider names known to the workspace (lowercase).
    pub fn tag_providers(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }

    /// Whether a tag path exists. Only absolute paths (`[provider]a/b`) to
    /// providers whose tags are in the workspace can be `Missing`; paths
    /// with placeholders, relative paths and unknown providers are
    /// `Unknown`. UDT instance members are expanded through their
    /// definitions (with inheritance). Tag properties (`.Quality`) are
    /// ignored.
    pub fn tag(&self, path: &str) -> Existence {
        if path.contains('{') {
            return Existence::Unknown;
        }
        let parsed = iglint_expression::parse_tag_path(path, iglint_text::TextSize::ZERO);
        if !parsed.is_valid() || parsed.is_relative() {
            return Existence::Unknown;
        }
        let Some(source) = parsed.source_name() else {
            return Existence::Unknown;
        };
        let provider_key = fold(source);
        if self.opaque_providers.contains(&provider_key) {
            return Existence::Unknown;
        }
        let Some(provider) = self.providers.get(&provider_key) else {
            return Existence::Unknown;
        };
        let components: Vec<String> = parsed
            .components
            .iter()
            .map(|c| {
                // Drop array indexes (`Tag[0]`).
                let t = c.text.split('[').next().unwrap_or(&c.text);
                fold(t.trim())
            })
            .collect();
        if components.is_empty() {
            return Existence::Exists;
        }
        if components[0] == "_types_" {
            if provider.udts.is_empty() {
                return Existence::Unknown;
            }
            let rest = components[1..].join("/");
            if rest.is_empty() {
                return Existence::Exists;
            }
            for (k, u) in &provider.udts {
                if *k == rest
                    || rest
                        .strip_prefix(k.as_str())
                        .and_then(|r| r.strip_prefix('/'))
                        .is_some_and(|member| self.udt_has_member(provider, u, member, 0))
                {
                    return Existence::Exists;
                }
                if k.starts_with(&format!("{rest}/")) {
                    return Existence::Exists;
                }
            }
            return Existence::Missing;
        }
        if !provider.has_definitions {
            return Existence::Unknown;
        }
        let full = components.join("/");
        if provider.paths.contains(&full) {
            return Existence::Exists;
        }
        // UDT instance member: find the deepest instance prefix.
        for i in (1..components.len()).rev() {
            let prefix = components[..i].join("/");
            if let Some(type_id) = provider.instances.get(&prefix) {
                let member = components[i..].join("/");
                return match provider.udts.get(&fold(type_id)) {
                    Some(u) => {
                        if self.udt_has_member(provider, u, &member, 0) {
                            Existence::Exists
                        } else if self.udt_chain_complete(provider, u, 0) {
                            Existence::Missing
                        } else {
                            Existence::Unknown
                        }
                    }
                    None => Existence::Unknown,
                };
            }
        }
        Existence::Missing
    }

    fn udt_has_member(
        &self,
        provider: &ProviderData,
        udt: &UdtInfo,
        member: &str,
        depth: usize,
    ) -> bool {
        if depth > 32 {
            return false;
        }
        if udt.members.iter().any(|m| fold(m) == member) {
            return true;
        }
        for (rel, type_id) in &udt.member_instances {
            let rel = fold(rel);
            if let Some(rest) = member.strip_prefix(&format!("{rel}/"))
                && let Some(nested) = provider.udts.get(&fold(type_id))
                && self.udt_has_member(provider, nested, rest, depth + 1)
            {
                return true;
            }
        }
        match &udt.parent {
            Some(parent) => match provider.udts.get(&fold(parent)) {
                Some(p) => self.udt_has_member(provider, p, member, depth + 1),
                None => false,
            },
            None => false,
        }
    }

    fn udt_chain_complete(&self, provider: &ProviderData, udt: &UdtInfo, depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        let nested_ok = udt
            .member_instances
            .iter()
            .all(|(_, t)| provider.udts.contains_key(&fold(t)));
        nested_ok
            && match &udt.parent {
                Some(parent) => match provider.udts.get(&fold(parent)) {
                    Some(p) => self.udt_chain_complete(provider, p, depth + 1),
                    None => false,
                },
                None => true,
            }
    }

    /// A UDT definition by provider and path (`Motors/Motor`).
    pub fn udt(&self, provider: &str, path: &str) -> Option<&UdtInfo> {
        self.providers.get(&fold(provider))?.udts.get(&fold(path))
    }

    /// Whether a UDT definition exists. `Unknown` when the provider has no
    /// definitions in the workspace.
    pub fn udt_existence(&self, provider: &str, path: &str) -> Existence {
        match self.providers.get(&fold(provider)) {
            Some(p) if !p.udts.is_empty() => {
                if p.udts.contains_key(&fold(path)) {
                    Existence::Exists
                } else {
                    Existence::Missing
                }
            }
            _ => Existence::Unknown,
        }
    }

    /// All UDT parameters of a definition including inherited ones.
    pub fn udt_parameters(&self, provider: &str, path: &str) -> Option<Vec<&str>> {
        let p = self.providers.get(&fold(provider))?;
        let mut out = Vec::new();
        let mut at = p.udts.get(&fold(path))?;
        for _ in 0..32 {
            out.extend(at.parameters.iter().map(String::as_str));
            match at
                .parent
                .as_ref()
                .and_then(|parent| p.udts.get(&fold(parent)))
            {
                Some(next) => at = next,
                None => break,
            }
        }
        Some(out)
    }

    /// Whether a historian provider exists. `Unknown` unless the workspace
    /// configures historian providers (built-in and module-provided
    /// historians are not on disk).
    pub fn history_provider(&self, name: &str) -> Existence {
        let key = fold(name);
        if self.history_providers.contains(&key) || self.databases.contains(&key) {
            Existence::Exists
        } else if self.has_history_providers {
            Existence::Missing
        } else {
            Existence::Unknown
        }
    }

    /// Whether a database connection exists in any collection.
    pub fn database(&self, name: &str) -> Existence {
        if self.databases.contains(&fold(name)) {
            Existence::Exists
        } else if self.has_databases {
            Existence::Missing
        } else {
            Existence::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_models_are_send_and_sync() {
        fn check<T: Send + Sync>() {}
        check::<ProjectIndex<'static>>();
        check::<Workspace>();
        check::<crate::LoadedResource>();
        check::<crate::ResourceModel<'static>>();
        check::<crate::ResourceSummary>();
    }
}
