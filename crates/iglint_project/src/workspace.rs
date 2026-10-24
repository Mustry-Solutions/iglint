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
    /// The path is a symbolic link to a file (the gateway reads it through
    /// the link; see [`Workspace::shared_file`]).
    pub symlink: bool,
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
    /// link, a symbolic link to a directory (see
    /// [`Workspace::directory_links`]), or a walk over [`WALK_LIMIT`]
    /// directories. What they hold is unknown, never absent: the closed
    /// world is unknown while any exists. Sorted by path.
    pub unread: Vec<WorkspaceProblem>,
    /// Symbolic links to directories inside projects and collections, and
    /// links that are (or lead to directories that hold) projects or
    /// collections. The gateway does not follow them (verified on 8.3.3),
    /// so what they hold is not loaded at that path; iglint does not read
    /// it as part of the workspace either. Each is also in
    /// [`Workspace::unread`]: a deployment that resolves links (a copy,
    /// an archive) would load it. Sorted.
    pub directory_links: Vec<PathBuf>,
    /// Files of the workspace by identity, computed on first use
    /// ([`Workspace::shared_file`]).
    identities: std::sync::OnceLock<FxHashMap<FileIdentity, Vec<FileId>>>,
}

/// What identifies a file on disk: device and inode on Unix, the
/// canonical path elsewhere.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FileIdentity {
    #[cfg_attr(not(unix), allow(dead_code))]
    Inode(u64, u64),
    #[cfg_attr(unix, allow(dead_code))]
    Path(PathBuf),
}

/// The identity of the file at `path` (through links) and its number of
/// hard links (1 where that is not known).
fn file_identity(path: &Path) -> Option<(FileIdentity, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some((FileIdentity::Inode(meta.dev(), meta.ino()), meta.nlink()))
    }
    #[cfg(not(unix))]
    {
        Some((FileIdentity::Path(std::fs::canonicalize(path).ok()?), 1))
    }
}

/// How the file at a path is shared with other paths
/// ([`Workspace::shared_file`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedFile {
    /// The path is a symbolic link to this file.
    pub link_target: Option<PathBuf>,
    /// Other paths of the workspace that reach the same file: symbolic
    /// links to it, the file a link points to, hard links.
    pub same_file: Vec<PathBuf>,
    /// Hard links to the file that are not paths of the workspace (they
    /// may be anywhere on the file system).
    pub other_hard_links: u64,
}

impl SharedFile {
    /// Whether an edit of the path changes the file for anything else.
    pub fn is_shared(&self) -> bool {
        self.link_target.is_some() || !self.same_file.is_empty() || self.other_hard_links > 0
    }
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
    files: Vec<ListedFile>,
}

struct ScannedContainer {
    dir: PathBuf,
    is_project: bool,
    context_only: bool,
    resources: Vec<ScannedResource>,
    /// Files not inside a resource: (rel path, path, kind, symbolic link).
    other_files: Vec<(String, PathBuf, FileKind, bool)>,
    problems: Vec<WorkspaceProblem>,
    not_read: NotRead,
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
        let mut not_read = NotRead::default();
        let mut containers: Vec<(PathBuf, bool, bool)> = Vec::new();
        for root in roots {
            let root = root.as_ref();
            let root = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
            if let Some((dir, is_project)) = enclosing_container(&root) {
                // A project or collection directory that is a symbolic
                // link is not loaded by the gateway.
                if is_symlink(&dir) {
                    not_read.link(dir);
                } else {
                    containers.push((dir, is_project, false));
                }
            } else {
                let mut walk = ContainerWalk {
                    out: &mut containers,
                    not_read: &mut not_read,
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
            unread: not_read.problems,
            directory_links: not_read.links,
            ..Workspace::default()
        };
        ws.assemble(scanned);
        ws.unread.sort_by(|a, b| a.path.cmp(&b.path));
        ws.unread.dedup();
        ws.directory_links.sort();
        ws.directory_links.dedup();
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
            self.unread.extend(scanned.not_read.problems);
            self.directory_links.extend(scanned.not_read.links);
            let (manifest_path, manifest_doc) = manifest.expect("container manifest");
            let manifest_link = is_symlink(&manifest_path);
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
            self.files[manifest_file.index()].symlink = manifest_link;
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
                for (file_name, path, link) in &res.files {
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
                    self.files[fid.index()].symlink = *link;
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
                            .filter(|f| !res.files.iter().any(|(n, _, _)| n == *f))
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
            for (rel, path, kind, link) in scanned.other_files {
                let id = self.push_file(path, rel, kind, Some(container), None, None);
                self.files[id.index()].symlink = link;
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
            symlink: false,
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

    /// The symbolic link to a directory ([`Workspace::directory_links`])
    /// that `path` lies behind, if any: the gateway does not load what is
    /// there.
    pub fn directory_link_above(&self, path: &Path) -> Option<&Path> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.directory_links
            .iter()
            .find(|link| abs.starts_with(link))
            .map(PathBuf::as_path)
    }

    /// The project or collection directory on disk that holds `path` when
    /// the walk did not reach it: no project or collection of the
    /// workspace holds the path and no symbolic link to a directory is
    /// above it ([`Workspace::directory_link_above`]). `None` for paths
    /// outside every project and collection.
    pub fn unreached_container(&self, path: &Path) -> Option<PathBuf> {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let (dir, _) = enclosing_container(&abs)?;
        if self.directory_link_above(&abs).is_some() {
            return None;
        }
        let held = self
            .projects
            .iter()
            .map(|p| &p.dir)
            .chain(self.collections.iter().map(|c| &c.dir))
            .any(|d| abs.starts_with(d));
        (!held).then_some(dir)
    }

    /// How the file at `path` is shared with other paths: it is a
    /// symbolic link, other files of the workspace are links to it or the
    /// same file through a link or a hard link, or it has hard links
    /// elsewhere. An edit of such a file changes what every one of them
    /// reads (verified on 8.3.3: a project whose `code.py` is a link to
    /// another project's file runs that file). Files are compared by
    /// identity (device and inode on Unix), computed for the whole
    /// workspace on the first call.
    pub fn shared_file(&self, path: &Path) -> SharedFile {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let mut shared = SharedFile::default();
        if is_symlink(&abs) {
            shared.link_target = Some(std::fs::canonicalize(&abs).unwrap_or_else(|_| abs.clone()));
        }
        let Some((identity, links)) = file_identity(&abs) else {
            return shared;
        };
        // Sequential on purpose: callers run on rayon's threads, and a
        // parallel loop inside the initialisation would wait for threads
        // that are themselves waiting for it.
        let identities = self.identities.get_or_init(|| {
            let mut map: FxHashMap<FileIdentity, Vec<FileId>> = FxHashMap::default();
            for f in &self.files {
                if let Some((identity, _)) = file_identity(&f.path) {
                    map.entry(identity).or_default().push(f.id);
                }
            }
            map
        });
        let mut hard_links_seen = 0;
        let own_name = (!is_symlink(&abs))
            .then(|| std::fs::canonicalize(&abs).ok())
            .flatten();
        for id in identities.get(&identity).into_iter().flatten() {
            let entry = &self.files[id.index()];
            // The path itself, also when it is spelled differently (through
            // a link above the workspace root).
            if entry.path == abs
                || (!entry.symlink
                    && own_name.is_some()
                    && std::fs::canonicalize(&entry.path).ok() == own_name)
            {
                continue;
            }
            if !entry.symlink {
                hard_links_seen += 1;
            }
            shared.same_file.push(entry.path.clone());
        }
        // The names of the file (hard links) that are not named already:
        // the path itself when it is not a link, the workspace paths that
        // are not links, and the target of a link when no workspace path
        // is the file itself.
        let link = shared.link_target.is_some();
        let named = hard_links_seen + u64::from(!link) + u64::from(link && hard_links_seen == 0);
        shared.other_hard_links = links.saturating_sub(named);
        shared.same_file.sort();
        shared.same_file.dedup();
        shared
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
    let mut not_read = NotRead::default();
    let mut walk = ContainerWalk {
        out: &mut out,
        not_read: &mut not_read,
        budget: WALK_LIMIT,
        first_only: true,
    };
    walk.root(dir);
    let exhausted = walk.budget == 0;
    // A symbolic link to a project or collection counts: the workspace
    // walk records it as not read.
    if !out.is_empty() || !not_read.links.is_empty() {
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

/// What the `.git` entry of a directory makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitTop {
    /// No `.git` entry.
    None,
    /// The top of a repository's work tree: a `.git` directory, or a
    /// `.git` file that points at a whole Git directory (a submodule, a
    /// repository made with `--separate-git-dir`) or that cannot be read.
    Repository,
    /// A linked work tree (`git worktree add`, also of a bare
    /// repository): a `.git` file that points at
    /// `<git dir>/worktrees/<name>`, which holds a `commondir` file. It is
    /// a checkout of its own, wherever it lies.
    LinkedWorktree,
}

/// What the `.git` entry of `dir` makes it. Only the files on disk count,
/// not `GIT_DIR` or `GIT_WORK_TREE`, so the answer does not depend on how
/// iglint is started (a Git hook sets them, a shell does not).
pub fn git_top(dir: &Path) -> GitTop {
    let dot_git = dir.join(".git");
    let Ok(meta) = std::fs::metadata(&dot_git) else {
        return GitTop::None;
    };
    if !meta.is_file() || meta.len() > 64 * 1024 {
        return GitTop::Repository;
    }
    let Ok(text) = std::fs::read_to_string(&dot_git) else {
        return GitTop::Repository;
    };
    let Some(target) = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)
        .filter(|t| !t.is_empty())
    else {
        return GitTop::Repository;
    };
    // Relative paths are relative to the directory of the `.git` file
    // (`worktree.useRelativePaths`, submodules).
    let git_dir = dir.join(target);
    if git_dir.join("commondir").is_file() {
        GitTop::LinkedWorktree
    } else {
        GitTop::Repository
    }
}

/// Whether the walk of [`Workspace::discover`] from `root` reaches `dir`,
/// a directory at or below `root`: it does unless a directory on the way
/// is a linked Git work tree ([`GitTop::LinkedWorktree`], a checkout of
/// its own) or is skipped by name (a hidden directory, `node_modules`,
/// ...) before the walk finds a project or collection. A symbolic link to
/// a directory on the way counts as reached: the walk records it
/// ([`Workspace::directory_links`]), and what lies behind it gets the
/// treatment of files behind a link.
pub fn walk_reaches(root: &Path, dir: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(root) else {
        return false;
    };
    let mut at = root.to_path_buf();
    for component in rel.components() {
        at.push(component);
        if git_top(&at) == GitTop::LinkedWorktree {
            return false;
        }
        if at.join(PROJECT_MANIFEST).is_file()
            || at.join(COLLECTION_MANIFEST).is_file()
            || is_symlink(&at)
        {
            return true;
        }
        if skipped_by_name(component.as_os_str()) {
            return false;
        }
    }
    true
}

/// Why a symbolic link to a directory is not walked.
pub const DIRECTORY_LINK: &str = "symbolic link to a directory: Ignition 8.3.3 does not follow \
     symbolic links to directories, so the gateway does not load what it holds at this path";

/// What a walk did not read: [`Workspace::unread`] entries, and among
/// them the symbolic links to directories ([`Workspace::directory_links`]).
#[derive(Default)]
struct NotRead {
    problems: Vec<WorkspaceProblem>,
    links: Vec<PathBuf>,
}

impl NotRead {
    fn problem(&mut self, path: PathBuf, message: String) {
        self.problems.push(WorkspaceProblem { path, message });
    }

    fn link(&mut self, path: PathBuf) {
        self.links.push(path.clone());
        self.problem(path, DIRECTORY_LINK.to_owned());
    }
}

/// The walk down from a root collecting container directories. Symbolic
/// links to directories are not followed, like the gateway (verified on
/// 8.3.3: a project directory `projects/S -> ../Sreal` never starts, a
/// linked `core` collection keeps the gateway from starting). A link that
/// leads to a project or collection, or to a directory that holds one, is
/// recorded as not read ([`NotRead::link`]); other links outside every
/// container (a linked documentation folder) are ignored. Every directory
/// that could hold a container but is not read is recorded too.
struct ContainerWalk<'o> {
    out: &'o mut Vec<(PathBuf, bool, bool)>,
    not_read: &'o mut NotRead,
    /// Directories the walk may still visit.
    budget: usize,
    /// Stop at the first container found (or link to one).
    first_only: bool,
}

impl ContainerWalk<'_> {
    fn root(&mut self, root: &Path) {
        if let Err(e) = std::fs::metadata(root) {
            self.not_read
                .problem(root.to_path_buf(), format!("cannot read directory: {e}"));
            return;
        }
        self.dir(root);
    }

    fn done(&self) -> bool {
        self.budget == 0
            || (self.first_only && (!self.out.is_empty() || !self.not_read.links.is_empty()))
    }

    fn dir(&mut self, dir: &Path) {
        if self.done() {
            return;
        }
        if self.budget == 1 {
            self.budget = 0;
            self.not_read.problem(
                dir.to_path_buf(),
                format!(
                    "the walk for projects and collections stopped after {WALK_LIMIT} directories"
                ),
            );
            return;
        }
        self.budget -= 1;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                self.not_read
                    .problem(dir.to_path_buf(), format!("cannot read directory: {e}"));
                return;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    self.not_read
                        .problem(dir.to_path_buf(), format!("cannot read directory: {e}"));
                    continue;
                }
            };
            let name = entry.file_name();
            let path = entry.path();
            match entry_kind(&entry, &path) {
                Entry::File | Entry::Other => {}
                Entry::Dir => subdirs.push((path, name, false)),
                Entry::DirLink => subdirs.push((path, name, true)),
                Entry::Unread(message) => {
                    if !is_hidden(&name) {
                        self.not_read.problem(path, message);
                    }
                }
            }
        }
        subdirs.sort();
        for (sub, name, link) in subdirs {
            if self.done() {
                return;
            }
            // A linked Git work tree is another checkout, not part of
            // this one's gateway, wherever it lies.
            if git_top(&sub) == GitTop::LinkedWorktree {
                continue;
            }
            let container = if sub.join(PROJECT_MANIFEST).is_file() {
                Some(true)
            } else if sub.join(COLLECTION_MANIFEST).is_file() {
                Some(false)
            } else {
                None
            };
            match (container, link) {
                (Some(_), true) => self.not_read.link(sub),
                (Some(is_project), false) => self.out.push((sub, is_project, false)),
                (None, _) if skipped_by_name(&name) => {}
                (None, false) => self.dir(&sub),
                (None, true) => {
                    // Does the link lead to projects or collections? The
                    // probe walks the target with the remaining budget and
                    // follows no link either.
                    let mut found = Vec::new();
                    let mut probe_not_read = NotRead::default();
                    let mut probe = ContainerWalk {
                        out: &mut found,
                        not_read: &mut probe_not_read,
                        budget: self.budget,
                        first_only: true,
                    };
                    probe.dir(&sub);
                    let exhausted = probe.budget == 0;
                    self.budget = probe.budget;
                    if !found.is_empty() || !probe_not_read.links.is_empty() || exhausted {
                        self.not_read.link(sub);
                    }
                }
            }
        }
    }
}

/// What a directory entry is for a walk.
enum Entry {
    /// A file, or a symbolic link to one.
    File,
    Dir,
    /// A symbolic link to a directory (not followed).
    DirLink,
    /// Neither (a socket, a device).
    Other,
    /// It cannot be told (a broken link, an entry that cannot be read),
    /// and why.
    Unread(String),
}

fn entry_kind(entry: &std::fs::DirEntry, path: &Path) -> Entry {
    let Ok(file_type) = entry.file_type() else {
        return Entry::Unread("cannot read the directory entry".to_owned());
    };
    if file_type.is_dir() {
        return Entry::Dir;
    }
    if file_type.is_file() {
        return Entry::File;
    }
    if !file_type.is_symlink() {
        return Entry::Other;
    }
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Entry::DirLink,
        Ok(meta) if meta.is_file() => Entry::File,
        Ok(_) => Entry::Other,
        Err(e) => Entry::Unread(format!("broken symbolic link: {e}")),
    }
}

/// Whether `path` itself is a symbolic link.
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Adds parent projects that live next to discovered projects. A parent
/// directory that is a symbolic link is not loaded (the gateway does not
/// follow it), so the inheritance chain is incomplete.
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
        if parent_dir.join(PROJECT_MANIFEST).is_file() && !is_symlink(&parent_dir) {
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
        not_read: NotRead::default(),
    };
    let manifest_name = if is_project {
        PROJECT_MANIFEST
    } else {
        COLLECTION_MANIFEST
    };
    let (files, modules) = list_dir(&dir, &mut sc.not_read);
    for (name, path, link) in files {
        if name != manifest_name {
            sc.other_files.push((name, path, FileKind::Stray, link));
        }
    }
    for (module, module_dir) in modules {
        let (files, types) = list_dir(&module_dir, &mut sc.not_read);
        for (name, path, link) in files {
            sc.other_files
                .push((format!("{module}/{name}"), path, FileKind::Stray, link));
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
    let (files, subdirs) = list_dir(dir, &mut sc.not_read);
    let manifest = if files.iter().any(|(n, _, _)| n == RESOURCE_MANIFEST) {
        Some(RESOURCE_MANIFEST)
    } else if files.iter().any(|(n, _, _)| n == UNARY_MANIFEST) {
        Some(UNARY_MANIFEST)
    } else {
        None
    };
    let kind = ResourceKind::from_type(module_id, type_id);
    let implied = manifest.is_none()
        && files
            .iter()
            .any(|(name, _, _)| FileKind::of_resource_file(kind, name).implies_resource());
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
            for (name, file, link) in files {
                sc.other_files
                    .push((format!("{prefix}{name}"), file, FileKind::Stray, link));
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

/// A file of a listing: name, path, and whether the path is a symbolic
/// link (to a file).
type ListedFile = (String, PathBuf, bool);
type DirListing = (Vec<ListedFile>, Vec<(String, PathBuf)>);

/// Files and subdirectories of `dir`, sorted by name. Hidden files are
/// skipped (the gateway reads only the files a manifest names); hidden
/// directories are not: the gateway loads hidden resources and folders
/// (`script-python/.hidden/`, `views/.folder/V`, verified on 8.3.3), and
/// no name is skipped inside a container. Symbolic links to files are
/// files: the gateway reads them (verified on 8.3.3 for `code.py`,
/// `resource.json`, `project.json`, `config-mode.json` and `tags.json`).
/// Symbolic links to directories are not followed, like the gateway
/// (verified: a linked `ignition/script-python/lib`, `script-python` or
/// `ignition` gives `ImportError: No module named lib`, a linked tag
/// folder of a collection has no tags), and are recorded in `not_read`
/// with directories that cannot be read and broken links.
fn list_dir(dir: &Path, not_read: &mut NotRead) -> DirListing {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        not_read.problem(dir.to_path_buf(), format!("cannot read directory: {e}"));
                        continue;
                    }
                };
                let hidden = is_hidden(&entry.file_name());
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = entry.path();
                match entry_kind(&entry, &path) {
                    Entry::File | Entry::Other if hidden => {}
                    Entry::File => {
                        let link = entry.file_type().is_ok_and(|t| t.is_symlink());
                        files.push((name, path, link));
                    }
                    Entry::Other => {}
                    Entry::Dir => dirs.push((name, path)),
                    Entry::DirLink => not_read.link(path),
                    Entry::Unread(message) => not_read.problem(path, message),
                }
            }
        }
        Err(e) => not_read.problem(dir.to_path_buf(), format!("cannot read directory: {e}")),
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

    /// Review round 17 (Opus): what a `.git` entry makes a directory, as
    /// Git writes them: a linked work tree's `.git` file points at
    /// `<git dir>/worktrees/<name>` (absolute, or relative with
    /// `worktree.useRelativePaths`), which holds `commondir`; a submodule's
    /// and a `--separate-git-dir` repository's point at a whole Git
    /// directory.
    #[test]
    fn git_tops_and_linked_work_trees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "repo/.git/HEAD", "ref: refs/heads/main\n");
        write(root, "repo/.git/worktrees/x/commondir", "../..\n");
        write(root, "repo/.git/modules/sub/HEAD", "ref: refs/heads/main\n");
        write(
            root,
            "repo/.worktrees/x/.git",
            &format!("gitdir: {}\n", root.join("repo/.git/worktrees/x").display()),
        );
        write(root, "repo/rel/x/.git", "gitdir: ../../.git/worktrees/x\n");
        write(root, "repo/sub/.git", "gitdir: ../.git/modules/sub\n");
        write(root, "repo/broken/.git", "not a gitdir line\n");
        write(root, "repo/gone/.git", "gitdir: /nonexistent/worktrees/y\n");
        assert_eq!(git_top(&root.join("repo")), GitTop::Repository);
        assert_eq!(
            git_top(&root.join("repo/.worktrees/x")),
            GitTop::LinkedWorktree
        );
        assert_eq!(git_top(&root.join("repo/rel/x")), GitTop::LinkedWorktree);
        assert_eq!(git_top(&root.join("repo/sub")), GitTop::Repository);
        assert_eq!(git_top(&root.join("repo/broken")), GitTop::Repository);
        assert_eq!(git_top(&root.join("repo/gone")), GitTop::Repository);
        assert_eq!(git_top(&root.join("repo/.worktrees")), GitTop::None);
        // The walk does not enter linked work trees, hidden or not, and
        // `walk_reaches` says so.
        let repo = root.join("repo");
        project(&repo, "projects/A");
        project(&repo, "rel/x/projects/W");
        project(&repo, ".worktrees/x/projects/H");
        project(&repo, "sub/projects/S");
        let ws = Workspace::discover(&[&repo]);
        assert_eq!(names(&ws), ["project A", "project S"]);
        assert!(walk_reaches(&repo, &repo.join("projects/A")));
        assert!(walk_reaches(&repo, &repo.join("sub/projects/S")));
        assert!(!walk_reaches(&repo, &repo.join("rel/x/projects/W")));
        assert!(!walk_reaches(&repo, &repo.join(".worktrees/x/projects/H")));
        assert!(walk_reaches(
            &repo.join(".worktrees/x"),
            &repo.join(".worktrees/x/projects/H")
        ));
        assert!(!walk_reaches(&repo.join("projects"), &repo));
        assert_eq!(
            ws.unreached_container(&repo.join("rel/x/projects/W/project.json")),
            Some(repo.join("rel/x/projects/W"))
        );
        assert_eq!(
            ws.unreached_container(&repo.join("projects/A/project.json")),
            None
        );
        assert_eq!(ws.unreached_container(&repo.join("README.md")), None);
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
        // Inside a container no directory is skipped by name: hidden
        // resources and folders, and resources named like build or cache
        // directories, load on an 8.3.3 gateway (hidden files do not
        // count: the gateway reads the files a manifest names).
        let lib = "projects/target/ignition/script-python";
        for module in [
            ".hidden",
            "pkg/.inner",
            "target",
            "node_modules",
            "__pycache__",
        ] {
            write(
                root,
                &format!("{lib}/{module}/resource.json"),
                r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
            );
            write(root, &format!("{lib}/{module}/code.py"), "x = 1\n");
        }
        write(root, &format!("{lib}/.hidden/.notes.py"), "");
        let ws = Workspace::discover(&[root]);
        let mut modules: Vec<&str> = ws
            .resources
            .iter()
            .filter(|r| &*r.type_id == "script-python")
            .map(|r| &*r.path)
            .collect();
        modules.sort_unstable();
        assert_eq!(
            modules,
            [
                ".hidden",
                "__pycache__",
                "m",
                "m",
                "m",
                "node_modules",
                "pkg/.inner",
                "target"
            ]
        );
        assert!(!ws.files.iter().any(|f| f.name() == ".notes.py"));
        let empty = tempfile::tempdir().unwrap();
        project(empty.path(), "node_modules/pkg/projects/X");
        assert_eq!(holds_container(empty.path()), Some(false));
    }

    /// Reviewer round 16 B2: the gateway does not follow symbolic links
    /// to directories (verified on 8.3.3: `projects/S -> ../Sreal` never
    /// starts, a linked `core` keeps the gateway from starting, a linked
    /// `ignition/script-python/lib`, `ignition/script-python` or
    /// `ignition` gives `ImportError: No module named lib`, a linked tag
    /// folder has no tags). Neither does the workspace walk: the links are
    /// recorded as not read, and what they hold is not part of the
    /// workspace.
    #[cfg(unix)]
    #[test]
    fn symbolic_links_to_directories_are_not_followed() {
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
        project(&elsewhere, "holder/projects/H");
        write(&elsewhere, "docs/readme.txt", "");
        project(&root, "projects/B");
        std::fs::create_dir_all(root.join("config/resources")).unwrap();
        let link =
            |target: &Path, at: &str| std::os::unix::fs::symlink(target, root.join(at)).unwrap();
        link(&elsewhere.join("A"), "projects/A");
        link(&elsewhere.join("core"), "config/resources/core");
        link(&elsewhere.join("holder"), "holder");
        link(&elsewhere.join("docs"), "docs");
        // Inside a project: a resource folder, the script library and the
        // module directory.
        link(
            &elsewhere.join("A/ignition/script-python/m"),
            "projects/B/ignition/script-python/linked",
        );
        let ws = Workspace::discover(&[&root]);
        assert_eq!(names(&ws), ["project B"]);
        let rel = |p: &PathBuf| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert_eq!(
            ws.directory_links.iter().map(rel).collect::<Vec<_>>(),
            [
                "config/resources/core",
                "holder",
                "projects/A",
                "projects/B/ignition/script-python/linked"
            ],
            "a link to a directory without projects (docs) is ignored"
        );
        assert_eq!(ws.unread.len(), 4);
        assert!(ws.unread.iter().all(|u| u.message == DIRECTORY_LINK));
        assert!(ws.resources.iter().all(|r| &*r.path == "m"));
        assert_eq!(
            ws.directory_link_above(&root.join("projects/A/ignition/script-python/m/code.py")),
            Some(root.join("projects/A").as_path())
        );
        assert_eq!(
            ws.directory_link_above(&root.join("projects/B/project.json")),
            None
        );
        // The library and the module directory as links.
        std::fs::remove_file(root.join("projects/B/ignition/script-python/linked")).unwrap();
        std::fs::rename(
            root.join("projects/B/ignition/script-python"),
            dir.path().join("sp"),
        )
        .unwrap();
        link(&dir.path().join("sp"), "projects/B/ignition/script-python");
        let ws = Workspace::discover(&[&root]);
        assert!(ws.resources.is_empty());
        assert!(
            ws.directory_links
                .contains(&root.join("projects/B/ignition/script-python"))
        );
        // A project directory given as the root that is a link.
        let ws = Workspace::discover(&[root.join("projects/A")]);
        assert!(ws.projects.is_empty());
        assert_eq!(ws.directory_links, [root.join("projects/A")]);
        assert_eq!(holds_container(&root.join("holder")), Some(true));
    }

    /// Reviewer round 16 B1: symbolic links to files are read (the gateway
    /// reads them: verified for `code.py`, `project.json`,
    /// `resource.json`, `config-mode.json` and `tags.json`), and every
    /// path that reaches the same file is known, also through hard links.
    #[cfg(unix)]
    #[test]
    fn files_reached_through_links_are_known() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        project(root, "projects/P");
        project(root, "projects/Q");
        project(root, "projects/H");
        let code = |p: &str| root.join(format!("projects/{p}/ignition/script-python/m/code.py"));
        std::fs::remove_file(code("Q")).unwrap();
        std::os::unix::fs::symlink(code("P"), code("Q")).unwrap();
        std::fs::remove_file(code("H")).unwrap();
        std::fs::hard_link(code("P"), code("H")).unwrap();
        write(root, "outside/x.py", "x = 1\n");
        project(root, "projects/O");
        std::fs::remove_file(code("O")).unwrap();
        std::os::unix::fs::symlink(root.join("outside/x.py"), code("O")).unwrap();
        // A manifest that is a link.
        std::fs::rename(
            root.join("projects/O/project.json"),
            root.join("outside/O-project.json"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            root.join("outside/O-project.json"),
            root.join("projects/O/project.json"),
        )
        .unwrap();
        let ws = Workspace::discover(&[root]);
        assert_eq!(
            names(&ws),
            ["project H", "project O", "project P", "project Q"]
        );
        assert!(ws.unread.is_empty(), "{:?}", ws.unread);
        let entry = |p: &Path| ws.files.iter().find(|f| f.path == p).unwrap();
        assert!(entry(&code("Q")).symlink);
        assert!(!entry(&code("P")).symlink);
        assert!(entry(&root.join("projects/O/project.json")).symlink);

        let p = ws.shared_file(&code("P"));
        assert_eq!(p.link_target, None);
        assert_eq!(p.same_file, [code("H"), code("Q")]);
        assert_eq!(p.other_hard_links, 0);
        assert!(p.is_shared());
        let q = ws.shared_file(&code("Q"));
        assert_eq!(
            q.link_target,
            Some(std::fs::canonicalize(code("P")).unwrap())
        );
        assert_eq!(q.same_file, [code("H"), code("P")]);
        let o = ws.shared_file(&code("O"));
        assert!(o.link_target.is_some() && o.same_file.is_empty());
        assert_eq!(o.other_hard_links, 0);
        let plain = ws.shared_file(&root.join("projects/H/project.json"));
        assert!(!plain.is_shared(), "{plain:?}");
        // A hard link outside the workspace.
        std::fs::hard_link(
            root.join("projects/H/project.json"),
            root.join("outside/h.json"),
        )
        .unwrap();
        let h = Workspace::discover(&[root]).shared_file(&root.join("projects/H/project.json"));
        assert_eq!(h.other_hard_links, 1);
        assert!(h.same_file.is_empty());
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

        // Links back to a directory they are in are not followed either
        // (no cycle), and are recorded when they lead to projects.
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
        assert!(found.iter().all(|f| f.1 == DIRECTORY_LINK));
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
