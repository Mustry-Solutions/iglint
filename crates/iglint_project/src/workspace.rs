//! Discovery of projects, gateway config collections, resources and files,
//! and resource inheritance.
//!
//! Discovery only walks directories and parses manifests (`project.json`,
//! `config-mode.json`, `resource.json`, `unary-resource.json`). Data files
//! are read later, per resource, by [`Workspace::load_resource`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::document::JsonDocument;
use crate::ids::{CollectionId, FileId, ProjectId, ResourceId};
use crate::kinds::{FileKind, ResourceKind};
use crate::manifest::{ContainerManifest, Manifest};

const PROJECT_MANIFEST: &str = "project.json";
const COLLECTION_MANIFEST: &str = "config-mode.json";
const RESOURCE_MANIFEST: &str = "resource.json";
const UNARY_MANIFEST: &str = "unary-resource.json";

/// A project or a gateway config collection: something that owns resources
/// and can inherit them from a parent of the same kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Container {
    Project(ProjectId),
    Collection(CollectionId),
}

/// Link from a project (or collection) to its parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentLink<I> {
    /// No parent.
    None,
    /// The parent was found.
    Found(I),
    /// The manifest names a parent that is not in the workspace. Lookups that
    /// fail in such a project are [`Existence::Unknown`], never missing.
    Missing(String),
    /// Following parents leads back to this container.
    Cycle(String),
}

impl<I: Copy> ParentLink<I> {
    /// The parent id, when found.
    pub fn found(&self) -> Option<I> {
        match self {
            ParentLink::Found(id) => Some(*id),
            _ => None,
        }
    }
}

/// Whether a referenced resource exists. Lookups never guess: when the
/// answer depends on something outside the workspace the result is
/// `Unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Existence {
    Exists,
    Missing,
    Unknown,
}

impl Existence {
    /// True for [`Existence::Missing`].
    pub fn is_missing(self) -> bool {
        self == Existence::Missing
    }
}

/// A child resource that replaces a parent resource of the same path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Override {
    /// The overriding resource (in the child).
    pub resource: ResourceId,
    /// The replaced resource (in an ancestor).
    pub replaced: ResourceId,
    /// True when the replaced resource is marked `overridable: false`.
    pub forbidden: bool,
}

/// Resources a container sees after inheritance.
#[derive(Debug, Default)]
pub struct Inheritance {
    effective: FxHashMap<ResourceKey, ResourceId>,
    /// Overrides of ancestor resources by this container's own resources.
    pub overrides: Vec<Override>,
    /// True when every ancestor is in the workspace.
    pub complete: bool,
}

/// `module \0 type \0 path`: one allocation per key.
type ResourceKey = Box<str>;

fn resource_key(module_id: &str, type_id: &str, path: &str) -> ResourceKey {
    let mut s = String::with_capacity(module_id.len() + type_id.len() + path.len() + 2);
    s.push_str(module_id);
    s.push('\0');
    s.push_str(type_id);
    s.push('\0');
    s.push_str(path);
    s.into_boxed_str()
}

/// A project (`project.json`).
#[derive(Debug)]
pub struct Project {
    pub id: ProjectId,
    /// The directory name, which is the project name.
    pub name: String,
    pub dir: PathBuf,
    pub manifest: ContainerManifest,
    pub parent: ParentLink<ProjectId>,
    pub children: Vec<ProjectId>,
    /// True when the project was loaded only because a discovered project
    /// inherits from it.
    pub context_only: bool,
    /// The project's own resources.
    pub resources: Vec<ResourceId>,
    pub inheritance: Inheritance,
}

impl Project {
    /// `inheritable` in `project.json`.
    pub fn inheritable(&self) -> bool {
        self.manifest.inheritable
    }
}

/// A gateway config collection (`config-mode.json`), for example `core`.
#[derive(Debug)]
pub struct Collection {
    pub id: CollectionId,
    /// The directory name, which is the collection name.
    pub name: String,
    pub dir: PathBuf,
    pub manifest: ContainerManifest,
    pub parent: ParentLink<CollectionId>,
    pub children: Vec<CollectionId>,
    pub resources: Vec<ResourceId>,
    pub inheritance: Inheritance,
}

/// A resource: a directory with a manifest below `<moduleId>/<typeId>/`.
#[derive(Debug)]
pub struct Resource {
    pub id: ResourceId,
    pub container: Container,
    pub module_id: Box<str>,
    pub type_id: Box<str>,
    /// Path below the type directory with `/` separators; empty for
    /// singletons.
    pub path: Box<str>,
    pub kind: ResourceKind,
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// Files in the resource directory (including the manifest), sorted by
    /// name.
    pub files: Vec<FileId>,
    /// Names listed in the manifest's `files` that do not exist.
    pub missing_files: Vec<String>,
}

impl Resource {
    /// The last path component (the resource name), empty for singletons.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or("")
    }

    /// `<moduleId>/<typeId>/<path>`.
    pub fn qualified_path(&self) -> String {
        if self.path.is_empty() {
            format!("{}/{}", self.module_id, self.type_id)
        } else {
            format!("{}/{}/{}", self.module_id, self.type_id, self.path)
        }
    }
}

/// A file found during discovery.
#[derive(Debug)]
pub struct FileEntry {
    pub id: FileId,
    pub path: PathBuf,
    /// Path relative to the container directory, `/`-separated.
    pub rel_path: String,
    pub kind: FileKind,
    pub container: Option<Container>,
    pub resource: Option<ResourceId>,
    /// `Some(false)` when the resource manifest has a `files` list that does
    /// not name this file: the gateway ignores it. `None` for manifests and
    /// files outside resources.
    pub listed: Option<bool>,
}

impl FileEntry {
    /// The file name.
    pub fn name(&self) -> &str {
        self.rel_path.rsplit('/').next().unwrap_or("")
    }
}

/// Something discovery could not do (unreadable directory or manifest).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceProblem {
    pub path: PathBuf,
    pub message: String,
}

/// Discovery settings.
#[derive(Clone, Copy, Debug)]
pub struct DiscoveryOptions {
    /// Also load parent projects that are not below a root, when they are
    /// siblings of a discovered project (`<projects>/<parent>/project.json`).
    pub load_parents: bool,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        DiscoveryOptions { load_parents: true }
    }
}

/// Everything discovered below a set of roots.
#[derive(Debug, Default)]
pub struct Workspace {
    pub projects: Vec<Project>,
    pub collections: Vec<Collection>,
    pub resources: Vec<Resource>,
    pub files: Vec<FileEntry>,
    pub problems: Vec<WorkspaceProblem>,
}

/// One resource directory found during the walk, before its manifest is
/// read.
struct ScannedResource {
    module_id: Box<str>,
    type_id: Box<str>,
    path: Box<str>,
    dir: PathBuf,
    manifest_name: &'static str,
    files: Vec<(String, PathBuf)>,
}

struct ScannedContainer {
    dir: PathBuf,
    is_project: bool,
    context_only: bool,
    resources: Vec<ScannedResource>,
    /// Files not inside a resource: (rel path, path, kind).
    other_files: Vec<(String, PathBuf, FileKind)>,
    problems: Vec<WorkspaceProblem>,
}

impl Workspace {
    /// Discovers projects and gateway config collections below `roots`, with
    /// default options. A root may be a container directory, a directory
    /// that contains containers, or any path inside a container (the
    /// enclosing container is used).
    pub fn discover<P: AsRef<Path>>(roots: &[P]) -> Workspace {
        Workspace::discover_with(roots, DiscoveryOptions::default())
    }

    /// Like [`Workspace::discover`] with explicit options.
    pub fn discover_with<P: AsRef<Path>>(roots: &[P], options: DiscoveryOptions) -> Workspace {
        let mut problems = Vec::new();
        let mut containers: Vec<(PathBuf, bool, bool)> = Vec::new();
        for root in roots {
            let root = root.as_ref();
            let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
            if let Some((dir, is_project)) = enclosing_container(&root) {
                containers.push((dir, is_project, false));
            } else {
                find_containers(&root, &mut containers, &mut problems);
            }
        }
        containers.sort();
        containers.dedup_by(|a, b| a.0 == b.0);

        if options.load_parents {
            load_parent_dirs(&mut containers);
        }

        let scanned: Vec<ScannedContainer> = containers
            .into_par_iter()
            .map(|(dir, is_project, context_only)| scan_container(dir, is_project, context_only))
            .collect();

        let mut ws = Workspace {
            problems,
            ..Workspace::default()
        };
        ws.assemble(scanned);
        ws.resolve_parents();
        ws.compute_inheritance();
        ws
    }

    /// Turns scanned containers into arenas and reads manifests in parallel.
    fn assemble(&mut self, scanned: Vec<ScannedContainer>) {
        // Read every manifest in parallel first.
        struct Pending {
            container: Container,
            manifest: Option<(PathBuf, Result<JsonDocument, String>)>,
            scanned: ScannedContainer,
        }
        let mut pending: Vec<Pending> = Vec::with_capacity(scanned.len());
        let (mut projects, mut collections) = (0, 0);
        for sc in scanned {
            let name = if sc.is_project {
                PROJECT_MANIFEST
            } else {
                COLLECTION_MANIFEST
            };
            let path = sc.dir.join(name);
            let container = if sc.is_project {
                projects += 1;
                Container::Project(ProjectId::from_index(projects - 1))
            } else {
                collections += 1;
                Container::Collection(CollectionId::from_index(collections - 1))
            };
            let doc = read_json(&path);
            pending.push(Pending {
                container,
                manifest: Some((path, doc)),
                scanned: sc,
            });
        }

        // Resource manifests, all containers at once.
        let resource_docs: Vec<Vec<Result<JsonDocument, String>>> = pending
            .par_iter()
            .map(|p| {
                p.scanned
                    .resources
                    .par_iter()
                    .map(|r| read_json(&r.dir.join(r.manifest_name)))
                    .collect()
            })
            .collect();

        for (p, docs) in pending.into_iter().zip(resource_docs) {
            let Pending {
                container,
                manifest,
                scanned,
            } = p;
            self.problems.extend(scanned.problems);
            let (manifest_path, manifest_doc) = manifest.expect("container manifest");
            let manifest_file = self.push_file(
                manifest_path,
                if scanned.is_project {
                    PROJECT_MANIFEST.to_owned()
                } else {
                    COLLECTION_MANIFEST.to_owned()
                },
                if scanned.is_project {
                    FileKind::ProjectManifest
                } else {
                    FileKind::CollectionManifest
                },
                Some(container),
                None,
                None,
            );
            let manifest_doc = match manifest_doc {
                Ok(doc) => doc,
                Err(message) => {
                    self.problems.push(WorkspaceProblem {
                        path: self.files[manifest_file.index()].path.clone(),
                        message,
                    });
                    JsonDocument::parse(Arc::from(""))
                }
            };
            let manifest = ContainerManifest::new(manifest_file, manifest_doc);
            let name = scanned
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let mut own = Vec::with_capacity(scanned.resources.len());
            for (res, doc) in scanned.resources.into_iter().zip(docs) {
                let id = ResourceId::from_index(self.resources.len());
                own.push(id);
                let kind = ResourceKind::from_type(&res.module_id, &res.type_id);
                let rel_dir = rel_dir(&res.module_id, &res.type_id, &res.path);
                // The manifest file first, so its id is known.
                let mut file_ids = Vec::with_capacity(res.files.len());
                let mut manifest_id = None;
                let unary = res.manifest_name == UNARY_MANIFEST;
                let doc = match doc {
                    Ok(doc) => doc,
                    Err(message) => {
                        self.problems.push(WorkspaceProblem {
                            path: res.dir.join(res.manifest_name),
                            message,
                        });
                        JsonDocument::parse(Arc::from(""))
                    }
                };
                // Peek at the `files` list to set `listed`.
                let manifest_probe = Manifest::new(FileId(u32::MAX), unary, doc);
                for (file_name, path) in &res.files {
                    let file_kind = FileKind::of_resource_file(kind, file_name);
                    let is_manifest = file_name == RESOURCE_MANIFEST || file_name == UNARY_MANIFEST;
                    let listed = if is_manifest || manifest_probe.info.files.is_none() {
                        None
                    } else {
                        Some(manifest_probe.lists(file_name))
                    };
                    let fid = self.push_file(
                        path.clone(),
                        format!("{rel_dir}{file_name}"),
                        file_kind,
                        Some(container),
                        Some(id),
                        listed,
                    );
                    if file_name == res.manifest_name {
                        manifest_id = Some(fid);
                    }
                    file_ids.push(fid);
                }
                let mut manifest = manifest_probe;
                manifest.file = manifest_id.expect("manifest file is part of the resource");
                let missing_files = manifest
                    .info
                    .files
                    .as_ref()
                    .map(|listed| {
                        listed
                            .iter()
                            .filter(|f| !res.files.iter().any(|(n, _)| n == *f))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                self.resources.push(Resource {
                    id,
                    container,
                    module_id: res.module_id,
                    type_id: res.type_id,
                    path: res.path,
                    kind,
                    dir: res.dir,
                    manifest,
                    files: file_ids,
                    missing_files,
                });
            }
            for (rel, path, kind) in scanned.other_files {
                self.push_file(path, rel, kind, Some(container), None, None);
            }

            match container {
                Container::Project(id) => {
                    debug_assert_eq!(id.index(), self.projects.len());
                    self.projects.push(Project {
                        id,
                        name,
                        dir: scanned.dir,
                        manifest,
                        parent: ParentLink::None,
                        children: Vec::new(),
                        context_only: scanned.context_only,
                        resources: own,
                        inheritance: Inheritance::default(),
                    });
                }
                Container::Collection(id) => {
                    debug_assert_eq!(id.index(), self.collections.len());
                    self.collections.push(Collection {
                        id,
                        name,
                        dir: scanned.dir,
                        manifest,
                        parent: ParentLink::None,
                        children: Vec::new(),
                        resources: own,
                        inheritance: Inheritance::default(),
                    });
                }
            }
        }
    }

    fn push_file(
        &mut self,
        path: PathBuf,
        rel_path: String,
        kind: FileKind,
        container: Option<Container>,
        resource: Option<ResourceId>,
        listed: Option<bool>,
    ) -> FileId {
        let id = FileId::from_index(self.files.len());
        self.files.push(FileEntry {
            id,
            path,
            rel_path,
            kind,
            container,
            resource,
            listed,
        });
        id
    }

    /// Links parents by name. A parent is looked up among containers of the
    /// same kind in the same directory first, then anywhere.
    fn resolve_parents(&mut self) {
        let project_links: Vec<ParentLink<ProjectId>> = self
            .projects
            .iter()
            .map(|p| match &p.manifest.parent {
                None => ParentLink::None,
                Some(name) => {
                    let siblings = |q: &&Project| q.dir.parent() == p.dir.parent();
                    let found = self
                        .projects
                        .iter()
                        .filter(siblings)
                        .find(|q| &q.name == name)
                        .or_else(|| self.projects.iter().find(|q| &q.name == name));
                    match found {
                        Some(q) if q.id != p.id => ParentLink::Found(q.id),
                        Some(_) => ParentLink::Cycle(name.clone()),
                        None => ParentLink::Missing(name.clone()),
                    }
                }
            })
            .collect();
        for (i, link) in project_links.into_iter().enumerate() {
            if let Some(parent) = link.found() {
                self.projects[parent.index()]
                    .children
                    .push(ProjectId::from_index(i));
            }
            self.projects[i].parent = link;
        }
        let collection_links: Vec<ParentLink<CollectionId>> = self
            .collections
            .iter()
            .map(|c| match &c.manifest.parent {
                None => ParentLink::None,
                Some(name) => {
                    let found = self
                        .collections
                        .iter()
                        .find(|q| q.dir.parent() == c.dir.parent() && &q.name == name);
                    match found {
                        Some(q) if q.id != c.id => ParentLink::Found(q.id),
                        Some(_) => ParentLink::Cycle(name.clone()),
                        None => ParentLink::Missing(name.clone()),
                    }
                }
            })
            .collect();
        for (i, link) in collection_links.into_iter().enumerate() {
            if let Some(parent) = link.found() {
                self.collections[parent.index()]
                    .children
                    .push(CollectionId::from_index(i));
            }
            self.collections[i].parent = link;
        }
        // Break cycles: a container whose ancestor chain returns to itself.
        for i in 0..self.projects.len() {
            let mut seen = vec![i];
            let mut at = self.projects[i].parent.found();
            while let Some(p) = at {
                if seen.contains(&p.index()) {
                    let name = self.projects[i].manifest.parent.clone().unwrap_or_default();
                    self.projects[i].parent = ParentLink::Cycle(name);
                    break;
                }
                seen.push(p.index());
                at = self.projects[p.index()].parent.found();
            }
        }
        for i in 0..self.collections.len() {
            let mut seen = vec![i];
            let mut at = self.collections[i].parent.found();
            while let Some(p) = at {
                if seen.contains(&p.index()) {
                    let name = self.collections[i]
                        .manifest
                        .parent
                        .clone()
                        .unwrap_or_default();
                    self.collections[i].parent = ParentLink::Cycle(name);
                    break;
                }
                seen.push(p.index());
                at = self.collections[p.index()].parent.found();
            }
        }
    }

    fn compute_inheritance(&mut self) {
        let project_parents: Vec<_> = self.projects.iter().map(|p| p.parent.clone()).collect();
        let project_own: Vec<_> = self.projects.iter().map(|p| p.resources.clone()).collect();
        let results = compute_chain(&project_parents, &project_own, &self.resources, |l| {
            l.found().map(ProjectId::index)
        });
        for (p, inh) in self.projects.iter_mut().zip(results) {
            p.inheritance = inh;
        }
        let collection_parents: Vec<_> =
            self.collections.iter().map(|c| c.parent.clone()).collect();
        let collection_own: Vec<_> = self
            .collections
            .iter()
            .map(|c| c.resources.clone())
            .collect();
        let results = compute_chain(&collection_parents, &collection_own, &self.resources, |l| {
            l.found().map(CollectionId::index)
        });
        for (c, inh) in self.collections.iter_mut().zip(results) {
            c.inheritance = inh;
        }
    }

    // ---- accessors ----

    /// A file by id.
    pub fn file(&self, id: FileId) -> &FileEntry {
        &self.files[id.index()]
    }

    /// A resource by id.
    pub fn resource(&self, id: ResourceId) -> &Resource {
        &self.resources[id.index()]
    }

    /// A project by id.
    pub fn project(&self, id: ProjectId) -> &Project {
        &self.projects[id.index()]
    }

    /// A collection by id.
    pub fn collection(&self, id: CollectionId) -> &Collection {
        &self.collections[id.index()]
    }

    /// The first project with this name.
    pub fn project_by_name(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// The file entry for a path, if discovered.
    pub fn file_by_path(&self, path: &Path) -> Option<&FileEntry> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.files.iter().find(|f| f.path == abs)
    }

    /// The inheritance data of a container.
    pub fn inheritance(&self, container: Container) -> &Inheritance {
        match container {
            Container::Project(p) => &self.projects[p.index()].inheritance,
            Container::Collection(c) => &self.collections[c.index()].inheritance,
        }
    }

    /// The name of a container.
    pub fn container_name(&self, container: Container) -> &str {
        match container {
            Container::Project(p) => &self.projects[p.index()].name,
            Container::Collection(c) => &self.collections[c.index()].name,
        }
    }

    /// The resource a container sees at `<module>/<type>/<path>` after
    /// inheritance.
    pub fn effective_resource(
        &self,
        container: Container,
        module_id: &str,
        type_id: &str,
        path: &str,
    ) -> Option<ResourceId> {
        let key = resource_key(module_id, type_id, path);
        self.inheritance(container).effective.get(&key).copied()
    }

    /// Every resource a container sees after inheritance, in resource id
    /// order.
    pub fn effective_resources(&self, container: Container) -> Vec<ResourceId> {
        let mut ids: Vec<ResourceId> = self
            .inheritance(container)
            .effective
            .values()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Ancestors of a project, nearest first.
    pub fn project_ancestors(&self, id: ProjectId) -> Vec<ProjectId> {
        let mut out = Vec::new();
        let mut at = self.projects[id.index()].parent.found();
        while let Some(p) = at {
            if out.contains(&p) || p == id {
                break;
            }
            out.push(p);
            at = self.projects[p.index()].parent.found();
        }
        out
    }

    /// All descendants of a project.
    pub fn project_descendants(&self, id: ProjectId) -> Vec<ProjectId> {
        let mut out = Vec::new();
        let mut stack = self.projects[id.index()].children.clone();
        while let Some(p) = stack.pop() {
            if out.contains(&p) || p == id {
                continue;
            }
            out.push(p);
            stack.extend(self.projects[p.index()].children.iter().copied());
        }
        out
    }

    /// The data files of a resource that the gateway loads (listed in the
    /// manifest), excluding the manifest itself.
    pub fn data_files(&self, id: ResourceId) -> impl Iterator<Item = &FileEntry> {
        let res = &self.resources[id.index()];
        res.files
            .iter()
            .map(|f| &self.files[f.index()])
            .filter(move |f| f.id != res.manifest.file)
    }

    /// A file of a resource by name.
    pub fn resource_file(&self, id: ResourceId, name: &str) -> Option<&FileEntry> {
        self.resources[id.index()]
            .files
            .iter()
            .map(|f| &self.files[f.index()])
            .find(|f| f.name() == name)
    }
}

fn compute_chain<I: Copy>(
    parents: &[ParentLink<I>],
    own: &[Vec<ResourceId>],
    resources: &[Resource],
    parent_index: impl Fn(&ParentLink<I>) -> Option<usize>,
) -> Vec<Inheritance> {
    let n = parents.len();
    let mut done: Vec<Option<Inheritance>> = (0..n).map(|_| None).collect();
    // Resolve in an order where parents come first.
    fn visit<I: Copy>(
        i: usize,
        parents: &[ParentLink<I>],
        own: &[Vec<ResourceId>],
        resources: &[Resource],
        parent_index: &dyn Fn(&ParentLink<I>) -> Option<usize>,
        done: &mut Vec<Option<Inheritance>>,
        depth: usize,
    ) {
        if done[i].is_some() {
            return;
        }
        let parent = if depth < parents.len() {
            parent_index(&parents[i])
        } else {
            None
        };
        let mut inh = Inheritance {
            complete: true,
            ..Inheritance::default()
        };
        if let Some(p) = parent {
            visit(p, parents, own, resources, parent_index, done, depth + 1);
            let parent_inh = done[p].as_ref().expect("parent computed");
            inh.effective = parent_inh.effective.clone();
            inh.complete = parent_inh.complete;
        } else if matches!(parents[i], ParentLink::Missing(_) | ParentLink::Cycle(_)) {
            inh.complete = false;
        }
        for &rid in &own[i] {
            let r = &resources[rid.index()];
            let key = resource_key(&r.module_id, &r.type_id, &r.path);
            if let Some(replaced) = inh.effective.insert(key, rid) {
                inh.overrides.push(Override {
                    resource: rid,
                    replaced,
                    forbidden: !resources[replaced.index()].manifest.overridable(),
                });
            }
        }
        done[i] = Some(inh);
    }
    for i in 0..n {
        visit(i, parents, own, resources, &parent_index, &mut done, 0);
    }
    done.into_iter().map(|d| d.unwrap_or_default()).collect()
}

fn rel_dir(module_id: &str, type_id: &str, path: &str) -> String {
    if path.is_empty() {
        format!("{module_id}/{type_id}/")
    } else {
        format!("{module_id}/{type_id}/{path}/")
    }
}

fn read_json(path: &Path) -> Result<JsonDocument, String> {
    let text = read_text(path)?;
    Ok(JsonDocument::parse(text))
}

/// Reads a file as UTF-8 text (a leading byte order mark is kept; the JSON
/// parser skips it).
pub(crate) fn read_text(path: &Path) -> Result<Arc<str>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read file: {e}"))?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Arc::from(s)),
        Err(e) => Err(format!("file is not valid UTF-8: {}", e.utf8_error())),
    }
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

/// The container that `path` is in (or is), with `true` for projects.
fn enclosing_container(path: &Path) -> Option<(PathBuf, bool)> {
    let mut at = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    while let Some(dir) = at {
        if dir.join(PROJECT_MANIFEST).is_file() {
            return Some((dir.to_path_buf(), true));
        }
        if dir.join(COLLECTION_MANIFEST).is_file() {
            return Some((dir.to_path_buf(), false));
        }
        at = dir.parent();
    }
    None
}

/// Walks down from `dir` collecting container directories.
fn find_containers(
    dir: &Path,
    out: &mut Vec<(PathBuf, bool, bool)>,
    problems: &mut Vec<WorkspaceProblem>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            problems.push(WorkspaceProblem {
                path: dir.to_path_buf(),
                message: format!("cannot read directory: {e}"),
            });
            return;
        }
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if is_hidden(&name) || name == "node_modules" || name == "target" {
            continue;
        }
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            subdirs.push(entry.path());
        }
    }
    subdirs.sort();
    for sub in subdirs {
        if sub.join(PROJECT_MANIFEST).is_file() {
            out.push((sub, true, false));
        } else if sub.join(COLLECTION_MANIFEST).is_file() {
            out.push((sub, false, false));
        } else {
            find_containers(&sub, out, problems);
        }
    }
}

/// Adds parent projects that live next to discovered projects.
fn load_parent_dirs(containers: &mut Vec<(PathBuf, bool, bool)>) {
    let mut i = 0;
    while i < containers.len() {
        let (dir, is_project, _) = containers[i].clone();
        i += 1;
        if !is_project {
            continue;
        }
        let Ok(text) = read_text(&dir.join(PROJECT_MANIFEST)) else {
            continue;
        };
        let doc = JsonDocument::parse(text);
        let Some(parent) = doc
            .root()
            .and_then(|r| r.get("parent"))
            .and_then(|p| p.as_str())
            .filter(|p| !p.is_empty() && !p.contains(['/', '\\']) && *p != "." && *p != "..")
        else {
            continue;
        };
        let Some(base) = dir.parent() else { continue };
        let parent_dir = base.join(parent);
        if containers.iter().any(|c| c.0 == parent_dir) {
            continue;
        }
        if parent_dir.join(PROJECT_MANIFEST).is_file() {
            containers.push((parent_dir, true, true));
        }
    }
}

fn scan_container(dir: PathBuf, is_project: bool, context_only: bool) -> ScannedContainer {
    let mut sc = ScannedContainer {
        dir: dir.clone(),
        is_project,
        context_only,
        resources: Vec::new(),
        other_files: Vec::new(),
        problems: Vec::new(),
    };
    let manifest_name = if is_project {
        PROJECT_MANIFEST
    } else {
        COLLECTION_MANIFEST
    };
    let (files, modules) = list_dir(&dir, &mut sc.problems);
    for (name, path) in files {
        if name != manifest_name {
            sc.other_files.push((name, path, FileKind::Stray));
        }
    }
    for (module, module_dir) in modules {
        let (files, types) = list_dir(&module_dir, &mut sc.problems);
        for (name, path) in files {
            sc.other_files
                .push((format!("{module}/{name}"), path, FileKind::Stray));
        }
        for (type_id, type_dir) in types {
            scan_resource_tree(&mut sc, &module, &type_id, &type_dir, String::new());
        }
    }
    sc
}

fn scan_resource_tree(
    sc: &mut ScannedContainer,
    module_id: &str,
    type_id: &str,
    dir: &Path,
    path: String,
) {
    let (files, subdirs) = list_dir(dir, &mut sc.problems);
    let manifest = if files.iter().any(|(n, _)| n == RESOURCE_MANIFEST) {
        Some(RESOURCE_MANIFEST)
    } else if files.iter().any(|(n, _)| n == UNARY_MANIFEST) {
        Some(UNARY_MANIFEST)
    } else {
        None
    };
    match manifest {
        Some(manifest_name) => sc.resources.push(ScannedResource {
            module_id: module_id.into(),
            type_id: type_id.into(),
            path: path.as_str().into(),
            dir: dir.to_path_buf(),
            manifest_name,
            files,
        }),
        None => {
            let prefix = rel_dir(module_id, type_id, &path);
            for (name, file) in files {
                sc.other_files
                    .push((format!("{prefix}{name}"), file, FileKind::Stray));
            }
        }
    }
    for (name, sub) in subdirs {
        let child = if path.is_empty() {
            name
        } else {
            format!("{path}/{name}")
        };
        scan_resource_tree(sc, module_id, type_id, &sub, child);
    }
}

type DirListing = (Vec<(String, PathBuf)>, Vec<(String, PathBuf)>);

/// Files and subdirectories of `dir` (hidden entries skipped), sorted by name.
fn list_dir(dir: &Path, problems: &mut Vec<WorkspaceProblem>) -> DirListing {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name();
                if is_hidden(&name) {
                    continue;
                }
                let name = name.to_string_lossy().into_owned();
                let path = entry.path();
                let is_dir = match entry.file_type() {
                    Ok(t) if t.is_symlink() => path.is_dir(),
                    Ok(t) => t.is_dir(),
                    Err(_) => false,
                };
                if is_dir {
                    dirs.push((name, path));
                } else {
                    files.push((name, path));
                }
            }
        }
        Err(e) => problems.push(WorkspaceProblem {
            path: dir.to_path_buf(),
            message: format!("cannot read directory: {e}"),
        }),
    }
    files.sort();
    dirs.sort();
    (files, dirs)
}
