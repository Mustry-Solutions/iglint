//! Discovery of projects, gateway config collections, resources and files,
//! and resource inheritance.
//!
//! Discovery only walks directories and parses manifests (`project.json`,
//! `config-mode.json`, `resource.json`, `unary-resource.json`). Data files
//! are read later, per resource, by [`Workspace::load_resource`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_text::TextRange;
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
    /// Directories that could hold Ignition resources but were not read: a
    /// directory that cannot be listed (permissions), a broken symbolic
    /// link, a symbolic link back to a directory it is in (a cycle, cut
    /// there), or a walk over [`WALK_LIMIT`] directories. What they hold
    /// is unknown, never absent: the closed world is unknown while any
    /// exists. Sorted by path.
    pub unread: Vec<WorkspaceProblem>,
}

/// The most directories a walk for projects and collections visits
/// outside them (see [`Workspace::unread`] when it is reached).
pub const WALK_LIMIT: usize = 200_000;

/// One resource directory found during the walk, before its manifest is
/// read.
struct ScannedResource {
    module_id: Box<str>,
    type_id: Box<str>,
    path: Box<str>,
    dir: PathBuf,
    manifest_name: &'static str,
    /// The directory has no manifest, but a file whose name and place make
    /// it a data file of this resource type (a `view.json` below
    /// `views/`, a `code.py` below `script-python/`, ...). The gateway
    /// does not load it; iglint lints it as that resource and reports the
    /// missing manifest.
    manifest_missing: bool,
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
    unread: Vec<WorkspaceProblem>,
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
        let mut unread = Vec::new();
        let mut containers: Vec<(PathBuf, bool, bool)> = Vec::new();
        for root in roots {
            let root = root.as_ref();
            let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
            if let Some((dir, is_project)) = enclosing_container(&root) {
                containers.push((dir, is_project, false));
            } else {
                let mut walk = ContainerWalk {
                    out: &mut containers,
                    unread: &mut unread,
                    budget: WALK_LIMIT,
                    first_only: false,
                };
                walk.root(&root);
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
            unread,
            ..Workspace::default()
        };
        ws.assemble(scanned);
        ws.unread.sort_by(|a, b| a.path.cmp(&b.path));
        ws.unread.dedup();
        ws.resolve_parents();
        ws.compute_inheritance();
        ws
    }

    /// Turns scanned containers into arenas and reads manifests in parallel.
    fn assemble(&mut self, scanned: Vec<ScannedContainer>) {
        // Read every manifest in parallel first.
        struct Pending {
            container: Container,
            manifest: Option<(PathBuf, Result<ReadJson, String>)>,
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
        let resource_docs: Vec<Vec<Result<ReadJson, String>>> = pending
            .par_iter()
            .map(|p| {
                p.scanned
                    .resources
                    .par_iter()
                    .map(|r| {
                        if r.manifest_missing {
                            Ok((JsonDocument::parse(Arc::from("{}")), Vec::new()))
                        } else {
                            read_json(&r.dir.join(r.manifest_name))
                        }
                    })
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
            self.unread.extend(scanned.unread);
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
            let (manifest_doc, invalid_utf8) = match manifest_doc {
                Ok(doc) => doc,
                Err(message) => {
                    self.problems.push(WorkspaceProblem {
                        path: self.files[manifest_file.index()].path.clone(),
                        message,
                    });
                    (JsonDocument::parse(Arc::from("")), Vec::new())
                }
            };
            let mut manifest = ContainerManifest::new(manifest_file, manifest_doc);
            manifest.invalid_utf8 = invalid_utf8;
            let name = scanned
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let mut own = Vec::with_capacity(scanned.resources.len());
            for (res, doc) in scanned.resources.into_iter().zip(docs) {
                let id = ResourceId::from_index(self.resources.len());
                // A resource without its manifest does not exist for the
                // gateway: it is linted, but neither inherited nor found by
                // references.
                if !res.manifest_missing {
                    own.push(id);
                }
                let kind = ResourceKind::from_type(&res.module_id, &res.type_id);
                let rel_dir = rel_dir(&res.module_id, &res.type_id, &res.path);
                // The manifest file first, so its id is known.
                let mut file_ids = Vec::with_capacity(res.files.len());
                let mut manifest_id = None;
                let unary = res.manifest_name == UNARY_MANIFEST;
                let (doc, invalid_utf8) = match doc {
                    Ok(doc) => doc,
                    Err(message) => {
                        self.problems.push(WorkspaceProblem {
                            path: res.dir.join(res.manifest_name),
                            message,
                        });
                        (JsonDocument::parse(Arc::from("")), Vec::new())
                    }
                };
                // Peek at the `files` list to set `listed`.
                let mut manifest_probe = Manifest::new(FileId(u32::MAX), unary, doc);
                manifest_probe.invalid_utf8 = invalid_utf8;
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
                if res.manifest_missing {
                    // A file entry for the manifest that does not exist, so
                    // that every resource has one. It is not part of
                    // `files` (which lists what is on disk) and has no text.
                    manifest_id = Some(self.push_file(
                        res.dir.join(res.manifest_name),
                        format!("{rel_dir}{}", res.manifest_name),
                        FileKind::of_resource_file(kind, res.manifest_name),
                        Some(container),
                        Some(id),
                        None,
                    ));
                    manifest.missing = true;
                }
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

/// A parsed JSON file and its invalid UTF-8 sequences.
type ReadJson = (JsonDocument, Vec<TextRange>);

fn read_json(path: &Path) -> Result<ReadJson, String> {
    let (text, invalid_utf8) = read_text(path)?;
    Ok((JsonDocument::parse(text), invalid_utf8))
}

/// Reads a file as UTF-8 text (a leading byte order mark is kept; the JSON
/// parser skips it). Invalid UTF-8 does not fail: like Ignition, which
/// decodes resources as UTF-8 and replaces invalid bytes with U+FFFD, the
/// file is read anyway (see [`iglint_text::decode_utf8`]); the ranges of
/// the invalid sequences are returned with the text.
pub fn read_text(path: &Path) -> Result<(Arc<str>, Vec<TextRange>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read file: {e}"))?;
    let (text, invalid_utf8) = iglint_text::decode_utf8(bytes);
    Ok((Arc::from(text), invalid_utf8))
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

/// Whether `dir`, or a directory below it, holds a project or a gateway
/// configuration collection that [`Workspace::discover`] would find:
/// `Some(true)` or `Some(false)`, or `None` when the walk stopped at
/// [`WALK_LIMIT`] directories without finding one.
pub fn holds_container(dir: &Path) -> Option<bool> {
    let mut out = Vec::new();
    let mut unread = Vec::new();
    let mut walk = ContainerWalk {
        out: &mut out,
        unread: &mut unread,
        budget: WALK_LIMIT,
        first_only: true,
    };
    walk.root(dir);
    let exhausted = walk.budget == 0;
    if !out.is_empty() {
        Some(true)
    } else if exhausted {
        None
    } else {
        Some(false)
    }
}

/// Directories skipped by name while looking for projects and
/// collections, unless they hold a `project.json` or `config-mode.json`
/// themselves (a project named `target` loads on the gateway): build
/// outputs, dependencies and caches, and hidden directories (`.git`).
fn skipped_by_name(name: &std::ffi::OsStr) -> bool {
    is_hidden(name) || name == "node_modules" || name == "target" || name == "__pycache__"
}

/// The walk down from a root collecting container directories. Symbolic
/// links to directories are followed (also outside the root), with the
/// path through the link, so that a linked project is found where the
/// gateway finds it; a link that leads back to a directory the walk is in
/// is a cycle and cut there. Every directory that could hold a container
/// but is not read is recorded in `unread`.
struct ContainerWalk<'o> {
    out: &'o mut Vec<(PathBuf, bool, bool)>,
    unread: &'o mut Vec<WorkspaceProblem>,
    /// Directories the walk may still visit.
    budget: usize,
    /// Stop at the first container found.
    first_only: bool,
}

impl ContainerWalk<'_> {
    fn root(&mut self, root: &Path) {
        match std::fs::canonicalize(root) {
            Ok(real) => {
                let mut ancestors = vec![real];
                self.dir(root, &mut ancestors);
            }
            Err(e) => self.unread.push(WorkspaceProblem {
                path: root.to_path_buf(),
                message: format!("cannot read directory: {e}"),
            }),
        }
    }

    fn done(&self) -> bool {
        self.budget == 0 || (self.first_only && !self.out.is_empty())
    }

    /// Walks `dir`, whose real path is the last of `ancestors` (the real
    /// paths of the directories the walk is in).
    fn dir(&mut self, dir: &Path, ancestors: &mut Vec<PathBuf>) {
        if self.done() {
            return;
        }
        if self.budget == 1 {
            self.budget = 0;
            self.unread.push(WorkspaceProblem {
                path: dir.to_path_buf(),
                message: format!(
                    "the walk for projects and collections stopped after {WALK_LIMIT} directories"
                ),
            });
            return;
        }
        self.budget -= 1;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                self.unread.push(WorkspaceProblem {
                    path: dir.to_path_buf(),
                    message: format!("cannot read directory: {e}"),
                });
                return;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.unread.push(WorkspaceProblem {
                        path: dir.to_path_buf(),
                        message: format!("cannot read directory: {e}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            let path = entry.path();
            match subdirectory(&entry, &path, ancestors) {
                Subdirectory::No => {}
                Subdirectory::Yes(real) => subdirs.push((path, name, real)),
                Subdirectory::Unread(message) => {
                    if !is_hidden(&name) {
                        self.unread.push(WorkspaceProblem { path, message });
                    }
                }
            }
        }
        subdirs.sort();
        for (sub, name, real) in subdirs {
            if self.done() {
                return;
            }
            if sub.join(PROJECT_MANIFEST).is_file() {
                self.out.push((sub, true, false));
            } else if sub.join(COLLECTION_MANIFEST).is_file() {
                self.out.push((sub, false, false));
            } else if !skipped_by_name(&name) {
                ancestors.push(real);
                self.dir(&sub, ancestors);
                ancestors.pop();
            }
        }
    }
}

/// What a directory entry is for a walk: not a directory, a directory with
/// its real path, or a directory (or a link that may be one) that cannot
/// be walked, and why.
enum Subdirectory {
    No,
    Yes(PathBuf),
    Unread(String),
}

/// Classifies `entry` (at `path`) of the directory whose real path is the
/// last of `ancestors`. A symbolic link is followed: to a file it is not a
/// directory; broken, or to a directory the walk is in (or one that holds
/// it), it cannot be walked.
fn subdirectory(entry: &std::fs::DirEntry, path: &Path, ancestors: &[PathBuf]) -> Subdirectory {
    let Ok(file_type) = entry.file_type() else {
        return Subdirectory::Unread("cannot read the directory entry".to_owned());
    };
    if !file_type.is_symlink() {
        if !file_type.is_dir() {
            return Subdirectory::No;
        }
        let parent = ancestors.last().map_or_else(PathBuf::new, PathBuf::clone);
        return Subdirectory::Yes(parent.join(entry.file_name()));
    }
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => Subdirectory::No,
        Ok(_) => match std::fs::canonicalize(path) {
            Ok(real) if ancestors.iter().any(|a| a.starts_with(&real)) => {
                Subdirectory::Unread(format!(
                    "symbolic link cycle: the link leads back to `{}`, which holds it",
                    real.display()
                ))
            }
            Ok(real) => Subdirectory::Yes(real),
            Err(e) => Subdirectory::Unread(format!("cannot resolve symbolic link: {e}")),
        },
        Err(e) => Subdirectory::Unread(format!("broken symbolic link: {e}")),
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
        let Ok((text, _)) = read_text(&dir.join(PROJECT_MANIFEST)) else {
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
        unread: Vec::new(),
    };
    let manifest_name = if is_project {
        PROJECT_MANIFEST
    } else {
        COLLECTION_MANIFEST
    };
    let mut ancestors = match std::fs::canonicalize(&dir) {
        Ok(real) => vec![real],
        Err(e) => {
            sc.unread.push(WorkspaceProblem {
                path: dir,
                message: format!("cannot read directory: {e}"),
            });
            return sc;
        }
    };
    let (files, modules) = list_dir(&dir, &ancestors, &mut sc.unread);
    for (name, path) in files {
        if name != manifest_name {
            sc.other_files.push((name, path, FileKind::Stray));
        }
    }
    for (module, module_dir, real) in modules {
        ancestors.push(real);
        let (files, types) = list_dir(&module_dir, &ancestors, &mut sc.unread);
        for (name, path) in files {
            sc.other_files
                .push((format!("{module}/{name}"), path, FileKind::Stray));
        }
        for (type_id, type_dir, real) in types {
            ancestors.push(real);
            scan_resource_tree(
                &mut sc,
                &module,
                &type_id,
                &type_dir,
                String::new(),
                &mut ancestors,
            );
            ancestors.pop();
        }
        ancestors.pop();
    }
    sc
}

fn scan_resource_tree(
    sc: &mut ScannedContainer,
    module_id: &str,
    type_id: &str,
    dir: &Path,
    path: String,
    ancestors: &mut Vec<PathBuf>,
) {
    let (files, subdirs) = list_dir(dir, ancestors, &mut sc.unread);
    let manifest = if files.iter().any(|(n, _)| n == RESOURCE_MANIFEST) {
        Some(RESOURCE_MANIFEST)
    } else if files.iter().any(|(n, _)| n == UNARY_MANIFEST) {
        Some(UNARY_MANIFEST)
    } else {
        None
    };
    let kind = ResourceKind::from_type(module_id, type_id);
    let implied = manifest.is_none()
        && files
            .iter()
            .any(|(name, _)| FileKind::of_resource_file(kind, name).implies_resource());
    let manifest = manifest.or_else(|| {
        implied.then_some(if kind.has_unary_manifest() {
            UNARY_MANIFEST
        } else {
            RESOURCE_MANIFEST
        })
    });
    match manifest {
        Some(manifest_name) => sc.resources.push(ScannedResource {
            module_id: module_id.into(),
            type_id: type_id.into(),
            path: path.as_str().into(),
            dir: dir.to_path_buf(),
            manifest_name,
            manifest_missing: implied,
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
    for (name, sub, real) in subdirs {
        let child = if path.is_empty() {
            name
        } else {
            format!("{path}/{name}")
        };
        ancestors.push(real);
        scan_resource_tree(sc, module_id, type_id, &sub, child, ancestors);
        ancestors.pop();
    }
}

type DirListing = (Vec<(String, PathBuf)>, Vec<(String, PathBuf, PathBuf)>);

/// Files and subdirectories (with their real paths) of `dir`, whose real
/// path is the last of `ancestors`, sorted by name. Hidden entries are
/// skipped. Symbolic links are followed; a directory that cannot be read,
/// a broken link and a link back to a directory the walk is in are
/// recorded in `unread` (see [`subdirectory`]).
fn list_dir(dir: &Path, ancestors: &[PathBuf], unread: &mut Vec<WorkspaceProblem>) -> DirListing {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        unread.push(WorkspaceProblem {
                            path: dir.to_path_buf(),
                            message: format!("cannot read directory: {e}"),
                        });
                        continue;
                    }
                };
                let name = entry.file_name();
                if is_hidden(&name) {
                    continue;
                }
                let name = name.to_string_lossy().into_owned();
                let path = entry.path();
                match subdirectory(&entry, &path, ancestors) {
                    Subdirectory::No => files.push((name, path)),
                    Subdirectory::Yes(real) => dirs.push((name, path, real)),
                    Subdirectory::Unread(message) => {
                        unread.push(WorkspaceProblem { path, message })
                    }
                }
            }
        }
        Err(e) => unread.push(WorkspaceProblem {
            path: dir.to_path_buf(),
            message: format!("cannot read directory: {e}"),
        }),
    }
    files.sort();
    dirs.sort();
    (files, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn project(root: &Path, rel: &str) {
        write(root, &format!("{rel}/project.json"), r#"{"title": "p"}"#);
        write(
            root,
            &format!("{rel}/ignition/script-python/m/resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        );
        write(
            root,
            &format!("{rel}/ignition/script-python/m/code.py"),
            "x = 1\n",
        );
    }

    fn names(ws: &Workspace) -> Vec<String> {
        let mut out: Vec<String> = ws
            .projects
            .iter()
            .map(|p| format!("project {}", p.name))
            .chain(
                ws.collections
                    .iter()
                    .map(|c| format!("collection {}", c.name)),
            )
            .collect();
        out.sort();
        out
    }

    fn unread(ws: &Workspace) -> Vec<(String, String)> {
        ws.unread
            .iter()
            .map(|u| {
                (
                    u.path.file_name().unwrap().to_string_lossy().into_owned(),
                    u.message.clone(),
                )
            })
            .collect()
    }

    /// Reviewer round 15 B2: a project or collection named like a build
    /// or dependency directory loads on the gateway, so it is found; the
    /// names are skipped only for directories that are neither.
    #[test]
    fn containers_are_found_whatever_their_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        project(root, "projects/target");
        project(root, "projects/node_modules");
        project(root, "projects/__pycache__");
        write(
            root,
            "services/gw/config/resources/target/config-mode.json",
            "{}",
        );
        // Below a skipped directory that is not a container: not loaded.
        project(root, "node_modules/pkg/projects/X");
        project(root, "target/debug/projects/Y");
        project(root, ".git/projects/Z");
        let ws = Workspace::discover(&[root]);
        assert_eq!(
            names(&ws),
            [
                "collection target",
                "project __pycache__",
                "project node_modules",
                "project target",
            ]
        );
        assert!(ws.unread.is_empty(), "{:?}", ws.unread);
        assert_eq!(holds_container(root), Some(true));
        let empty = tempfile::tempdir().unwrap();
        project(empty.path(), "node_modules/pkg/projects/X");
        assert_eq!(holds_container(empty.path()), Some(false));
    }

    /// Reviewer round 15 B2: symbolic links to projects and collections,
    /// also outside the walked tree, are followed, and the project is
    /// found at the path through the link, with its resources.
    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_followed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let elsewhere = dir.path().join("elsewhere");
        project(&elsewhere, "A");
        write(&elsewhere, "core/config-mode.json", "{}");
        write(
            &elsewhere,
            "core/ignition/tag-definition/default/tags.json",
            "[]",
        );
        project(&elsewhere, "lib/ignition/script-python/shared");
        std::fs::create_dir_all(root.join("projects")).unwrap();
        std::fs::create_dir_all(root.join("config/resources")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("A"), root.join("projects/A")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("core"), root.join("config/resources/core"))
            .unwrap();
        let ws = Workspace::discover(&[&root]);
        assert_eq!(names(&ws), ["collection core", "project A"]);
        assert!(ws.unread.is_empty(), "{:?}", ws.unread);
        let a = ws.project_by_name("A").unwrap();
        assert_eq!(a.dir, root.join("projects/A"));
        assert!(
            ws.files
                .iter()
                .any(|f| f.path == root.join("projects/A/ignition/script-python/m/code.py")),
        );
        assert!(
            ws.resources.iter().any(
                |r| r.dir == root.join("config/resources/core/ignition/tag-definition/default")
            )
        );
        // A linked resource directory inside a project.
        std::os::unix::fs::symlink(
            elsewhere.join("lib/ignition/script-python/shared/ignition/script-python/m"),
            elsewhere.join("A/ignition/script-python/linked"),
        )
        .unwrap();
        let ws = Workspace::discover(&[&root]);
        assert!(
            ws.resources
                .iter()
                .any(|r| &*r.path == "linked" && r.dir.starts_with(root.join("projects/A"))),
            "{:?}",
            ws.resources.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
    }

    /// Reviewer round 15 B2: what cannot be walked is recorded as not
    /// read (and so makes the closed world unknown), never as absent: a
    /// broken link, a link cycle (cut, the walk ends), a directory that
    /// cannot be listed.
    #[cfg(unix)]
    #[test]
    fn what_cannot_be_walked_is_unread() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        project(root, "projects/A");
        std::os::unix::fs::symlink(root.join("gone"), root.join("projects/B")).unwrap();
        let ws = Workspace::discover(&[root]);
        assert_eq!(names(&ws), ["project A"]);
        let found = unread(&ws);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "B");
        assert!(found[0].1.starts_with("broken symbolic link"), "{found:?}");
        std::fs::remove_file(root.join("projects/B")).unwrap();

        // Cycles, outside and inside a project.
        std::os::unix::fs::symlink(root, root.join("projects/loop")).unwrap();
        std::os::unix::fs::symlink(
            root.join("projects/A/ignition"),
            root.join("projects/A/ignition/script-python/m/back"),
        )
        .unwrap();
        let ws = Workspace::discover(&[root]);
        assert_eq!(names(&ws), ["project A"]);
        let found = unread(&ws);
        assert_eq!(
            found.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
            ["back", "loop"],
            "{found:?}"
        );
        assert!(found.iter().all(|f| f.1.starts_with("symbolic link cycle")));
        std::fs::remove_file(root.join("projects/loop")).unwrap();
        std::fs::remove_file(root.join("projects/A/ignition/script-python/m/back")).unwrap();

        // A directory that cannot be listed (skipped when running as
        // root, which reads it anyway).
        std::fs::create_dir_all(root.join("locked/projects")).unwrap();
        let locked = root.join("locked");
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        let readable = std::fs::read_dir(&locked).is_ok();
        let ws = Workspace::discover(&[root]);
        let found = unread(&ws);
        std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        if !readable {
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].0, "locked");
            assert!(found[0].1.starts_with("cannot read directory"), "{found:?}");
        }
        // Inside a project too.
        let module = root.join("projects/A/ignition/script-python/m");
        std::fs::set_permissions(&module, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        let readable = std::fs::read_dir(&module).is_ok();
        let ws = Workspace::discover(&[root]);
        let found = unread(&ws);
        std::fs::set_permissions(&module, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        if !readable {
            assert_eq!(found.len(), 1, "{found:?}");
            assert_eq!(found[0].0, "m");
        }
    }
}
