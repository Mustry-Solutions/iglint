//! The project index: cross-resource facts for reference checks and
//! "unused" rules, built in `O(n)` from per-resource summaries.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::ids::{ProjectId, ResourceId};
use crate::kinds::{EventScriptKind, MODULE_IGNITION, MODULE_PERSPECTIVE, ResourceKind};
use crate::library::{LibraryLookup, LibraryTree, ModuleSymbols};
use crate::perspective::match_page_url;
use crate::summary::{
    Facts, MessageTarget, NamedQueryFacts, ResourceSummary, SessionFacts, StyleClassFacts,
    TagFacts, ViewFacts,
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
}

/// Lowercases ASCII and non-ASCII letters (tag paths are case-insensitive).
fn fold(s: &str) -> String {
    s.to_lowercase()
}

impl<'w> ProjectIndex<'w> {
    /// Builds the index. `summaries` may come in any order; resources
    /// without a summary are treated as existing with unknown content.
    pub fn build(ws: &'w Workspace, summaries: Vec<ResourceSummary>) -> ProjectIndex<'w> {
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
            databases: FxHashSet::default(),
            has_databases: false,
            usage: FxHashMap::default(),
            symbol_usage: FxHashMap::default(),
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
            if !matches!(r.container, Container::Collection(_)) {
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
        let mut sent: Vec<FxHashMap<String, u32>> = vec![FxHashMap::default(); ws.projects.len()];
        for p in &ws.projects {
            let pid = p.id;
            for rid in ws.effective_resources(Container::Project(pid)) {
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
                        }
                        LibraryLookup::Module { resource, .. } => {
                            *usage.entry(resource).or_default() += 1;
                        }
                        _ => {}
                    }
                }
                for m in &refs.messages {
                    let target_project = match &m.target {
                        MessageTarget::Perspective { .. } => Some(pid),
                        MessageTarget::Gateway {
                            project: Some(name),
                        } => ws.projects.iter().find(|q| &q.name == name).map(|q| q.id),
                        MessageTarget::Gateway { project: None } => None,
                    };
                    match target_project {
                        Some(t) => *sent[t.index()].entry(m.message_type.clone()).or_default() += 1,
                        None => {
                            for s in sent.iter_mut() {
                                *s.entry(m.message_type.clone()).or_default() += 1;
                            }
                        }
                    }
                }
            }
        }
        for (p, s) in self.projects.iter_mut().zip(sent) {
            p.sent = s;
        }
        self.usage = usage;
        self.symbol_usage = symbol_usage;
    }

    // ---- generic access ----

    /// The workspace the index was built from.
    pub fn workspace(&self) -> &'w Workspace {
        self.ws
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
        let data = &self.projects[project.index()];
        let result = data.library.resolve(dotted, |r| self.module_symbols(r));
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
                    || rest.starts_with(&format!("{k}/")) && {
                        let member = &rest[k.len() + 1..];
                        self.udt_has_member(provider, u, member, 0)
                    }
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
