//! Finding the files to lint.
//!
//! Paths given by the user are walked recursively (respecting `.gitignore`
//! when configured). Every file is classified by [`FileKind::classify`];
//! files that are not Ignition resources are skipped silently while walking,
//! but linted when named explicitly.
//!
//! A walk lints only Python files that belong to an Ignition tree: library
//! modules and resource scripts always do, and any other `.py` file
//! ([`FileKind::Python`]) only when it lies inside a project (an ancestor
//! directory holds `project.json`) or a gateway configuration collection (an
//! ancestor holds `config-mode.json`). Build scripts, tools and virtual
//! environments next to the projects are therefore not linted as Jython. A
//! loose Python file named explicitly is still linted. JSON kinds keep their
//! own rules ([`FileKind::discovered_by_walk`]), so a standalone `view.json`
//! is still found by a walk.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::settings::ProjectSettings;

/// What kind of file a path is, which decides how it is analysed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    /// A Perspective view, `view.json`.
    View,
    /// A project library script, `ignition/script-python/**/code.py`.
    ScriptModule,
    /// A script stored as a file in a resource directory: gateway and
    /// session events, WebDev handlers and similar.
    ResourceScript,
    /// Any other Python file.
    Python,
    /// A resource manifest: `resource.json`, `unary-resource.json`,
    /// `project.json` or `config-mode.json`.
    Manifest,
    /// A JSON data file of a resource (style classes, tags, named query
    /// metadata, page configuration, gateway configuration).
    ResourceJson,
    /// A JSON file outside any resource. Only linted when named explicitly.
    Json,
}

impl FileKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            FileKind::View => "view",
            FileKind::ScriptModule => "script-module",
            FileKind::ResourceScript => "resource-script",
            FileKind::Python => "python",
            FileKind::Manifest => "manifest",
            FileKind::ResourceJson => "resource-json",
            FileKind::Json => "json",
        }
    }

    /// Classifies `path` by its name and its neighbours on disk. Returns
    /// `None` for files iglint does not lint (images, CSS, text).
    pub fn classify(path: &Path) -> Option<FileKind> {
        classify_with(path, &mut has_manifest)
    }

    /// Whether files of this kind are picked up while walking directories.
    pub const fn discovered_by_walk(self) -> bool {
        !matches!(self, FileKind::Json)
    }

    pub const fn is_python(self) -> bool {
        matches!(
            self,
            FileKind::ScriptModule | FileKind::ResourceScript | FileKind::Python
        )
    }
}

const MANIFESTS: [&str; 4] = [
    "resource.json",
    "unary-resource.json",
    "project.json",
    "config-mode.json",
];

/// Files that mark the root of an Ignition tree: a project or a gateway
/// configuration collection.
const TREE_ROOTS: [&str; 2] = ["project.json", "config-mode.json"];

/// Memoised answers to "is this directory inside an Ignition tree?", so a
/// walk stats each directory's markers at most once.
#[derive(Default)]
struct IgnitionTrees {
    /// Directory -> the root of the tree it is in, if any.
    known: HashMap<PathBuf, Option<PathBuf>>,
}

impl IgnitionTrees {
    fn contains(&mut self, dir: &Path) -> bool {
        self.root_of(dir).is_some()
    }

    /// The project or collection directory that `dir` is in (or is).
    fn root_of(&mut self, dir: &Path) -> Option<PathBuf> {
        let dir = absolute(dir);
        let mut visited = Vec::new();
        let mut root = None;
        for ancestor in dir.ancestors() {
            if let Some(known) = self.known.get(ancestor) {
                root = known.clone();
                break;
            }
            visited.push(ancestor.to_path_buf());
            if TREE_ROOTS.iter().any(|m| ancestor.join(m).is_file()) {
                root = Some(ancestor.to_path_buf());
                break;
            }
        }
        for dir in visited {
            self.known.insert(dir, root.clone());
        }
        root
    }
}

/// Whether `file`, a JSON file in a directory without a manifest inside the
/// tree at `root`, is the main data file of a resource whose manifest is
/// missing (a `style.json` below `style-classes/`, a `tags.json` in a tag
/// folder, ...). The workspace lints such directories as resources.
fn implies_resource(file: &Path, root: &Path) -> bool {
    let Ok(relative) = absolute(file).strip_prefix(root).map(Path::to_path_buf) else {
        return false;
    };
    let parts: Vec<&str> = relative
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let [module, type_id, .., name] = parts.as_slice() else {
        return false;
    };
    if parts.len() < 3 {
        return false;
    }
    let kind = iglint_project::ResourceKind::from_type(module, type_id);
    iglint_project::FileKind::of_resource_file(kind, name).implies_resource()
}

fn has_manifest(dir: &Path) -> bool {
    MANIFESTS.iter().any(|m| dir.join(m).is_file())
}

fn classify_with(path: &Path, manifest_in: &mut dyn FnMut(&Path) -> bool) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?;
    let extension = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("py") => {
            let in_script_library = path
                .components()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w[0].as_os_str() == "ignition" && w[1].as_os_str() == "script-python");
            if name == "code.py" && in_script_library {
                Some(FileKind::ScriptModule)
            } else if path.parent().is_some_and(&mut *manifest_in) {
                Some(FileKind::ResourceScript)
            } else {
                Some(FileKind::Python)
            }
        }
        Some("json") => {
            if name == "view.json" {
                Some(FileKind::View)
            } else if MANIFESTS.contains(&name) {
                Some(FileKind::Manifest)
            } else if path.parent().is_some_and(&mut *manifest_in) {
                Some(FileKind::ResourceJson)
            } else {
                Some(FileKind::Json)
            }
        }
        _ => None,
    }
}

/// Why a file was not linted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Matched `exclude`.
    Excluded,
    /// Listed in an ign-lint whitelist.
    Whitelisted,
    /// Named explicitly but not a file iglint lints.
    UnsupportedFile,
    /// Found, but its content is not linted; the reason says why (for
    /// example a Python file of a resource type iglint does not model).
    NotModelled(&'static str),
}

impl SkipReason {
    /// Why the file was skipped, for the run summary.
    pub fn describe(&self) -> &'static str {
        match self {
            SkipReason::Excluded => "excluded",
            SkipReason::Whitelisted => "listed in the ign-lint whitelist",
            SkipReason::UnsupportedFile => "not a file iglint lints",
            SkipReason::NotModelled(reason) => reason,
        }
    }
}

/// The result of discovery, in a stable order (sorted by path).
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub files: Vec<(PathBuf, FileKind)>,
    pub skipped: Vec<(PathBuf, SkipReason)>,
    /// Paths that do not exist or could not be read.
    pub errors: Vec<(PathBuf, String)>,
}

/// Walks `paths` and classifies every file below them. Directory walks skip
/// hidden files, hidden directories outside every project and collection
/// (hidden projects, collections and resource directories load on the
/// gateway and are walked), `.gitignore`d files (when configured),
/// excluded paths and Python files outside any Ignition tree; explicitly
/// named files are still checked against `exclude` and the whitelist so
/// that editor and pre-commit integrations honour them.
pub fn discover(settings: &ProjectSettings, paths: &[PathBuf]) -> Discovery {
    let mut discovery = Discovery::default();
    let mut seen = BTreeSet::new();
    let mut manifest_cache: HashMap<PathBuf, bool> = HashMap::new();
    let mut manifest_in = |dir: &Path| {
        *manifest_cache
            .entry(dir.to_path_buf())
            .or_insert_with(|| has_manifest(dir))
    };
    let mut trees = IgnitionTrees::default();
    let skip = Arc::new(Skip::new(settings));
    for path in paths {
        if path.is_file() {
            if !seen.insert(path.clone()) {
                continue;
            }
            if let Some(reason) = skip.reason(path) {
                discovery.skipped.push((path.clone(), reason));
                continue;
            }
            match classify_with(path, &mut manifest_in) {
                Some(kind) => discovery.files.push((path.clone(), kind)),
                None => discovery
                    .skipped
                    .push((path.clone(), SkipReason::UnsupportedFile)),
            }
            continue;
        }
        if !path.is_dir() {
            discovery
                .errors
                .push((path.clone(), "No such file or directory".to_owned()));
            continue;
        }
        let mut builder = ignore::WalkBuilder::new(path);
        builder
            // Hidden entries are filtered below: hidden resource
            // directories inside a container load on the gateway.
            .hidden(false)
            .git_ignore(settings.respect_gitignore)
            .git_global(settings.respect_gitignore)
            .git_exclude(settings.respect_gitignore)
            .ignore(settings.respect_gitignore)
            .parents(settings.respect_gitignore)
            .require_git(false)
            // Links are followed, unlike the workspace walk and the
            // gateway: the files behind a link to a directory are linted
            // on their own, and REF009 says that the gateway does not
            // load them (no fixes are offered for them).
            .follow_links(true)
            .sort_by_file_name(|a, b| a.cmp(b));
        // Prune excluded directories early instead of filtering their files.
        let dir_skip = Arc::clone(&skip);
        builder.filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            // A linked Git work tree below the walked path is a checkout
            // of its own (the workspace walk does not enter it either).
            if is_dir
                && iglint_project::workspace::git_top(entry.path())
                    == iglint_project::workspace::GitTop::LinkedWorktree
            {
                return false;
            }
            if entry.file_name().to_string_lossy().starts_with('.')
                && !(is_dir && hidden_dir_loads(entry.path()))
            {
                return false;
            }
            !is_dir || !dir_skip.is_excluded(entry.path())
        });
        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
                // A link cycle or a broken link: nothing to lint there. The
                // workspace walk records the directory as not read, which
                // makes the closed world unknown.
                Err(error) if link_problem(&error) => continue,
                Err(error) => {
                    discovery.errors.push((path.clone(), error.to_string()));
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let file = entry.into_path();
            let Some(mut kind) = classify_with(&file, &mut manifest_in) else {
                continue;
            };
            if kind == FileKind::Json
                && let Some(root) = file.parent().and_then(|dir| trees.root_of(dir))
                && implies_resource(&file, &root)
            {
                kind = FileKind::ResourceJson;
            }
            if !kind.discovered_by_walk() {
                continue;
            }
            if kind == FileKind::Python && !file.parent().is_some_and(|dir| trees.contains(dir)) {
                continue;
            }
            if !seen.insert(file.clone()) {
                continue;
            }
            match skip.reason(&file) {
                Some(reason) => discovery.skipped.push((file, reason)),
                None => discovery.files.push((file, kind)),
            }
        }
    }
    discovery.files.sort();
    discovery.skipped.sort_by(|a, b| a.0.cmp(&b.0));
    discovery
}

/// Whether the gateway loads what the hidden directory `dir` holds: it is
/// a project or collection, or inside one (the gateway loads hidden
/// projects, resources and folders; verified on 8.3.3). Other hidden
/// directories (`.git`, `.venv`) are not walked.
fn hidden_dir_loads(dir: &Path) -> bool {
    dir.ancestors().any(is_container)
}

/// Whether a walk error is a symbolic link cycle or a broken symbolic
/// link.
fn link_problem(error: &ignore::Error) -> bool {
    match error {
        ignore::Error::Loop { .. } => true,
        ignore::Error::WithPath { path, err } => {
            (path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
                && !path.exists())
                || link_problem(err)
        }
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            link_problem(err)
        }
        _ => false,
    }
}

/// Where the workspace of a run is: the directories whose projects and
/// gateway collections form the closed world (see `docs/internal/fix-safety.md`,
/// "Closed world"), and the linted paths around which none was found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceRoots {
    /// Directories to discover the workspace in, sorted, none inside
    /// another.
    pub roots: Vec<PathBuf>,
    /// Linted paths with no workspace root: a loose file, or a project
    /// whose surroundings say nothing about the rest of its gateway. The
    /// workspace is then only what encloses the path, and the closed world
    /// is unknown.
    pub unknown: Vec<PathBuf>,
}

/// The workspace roots of a run over `paths`. Every project and gateway
/// collection below a root is analysed, whatever subset of files is
/// linted, so that linting one file (pre-commit, an editor, `cd
/// projects/A`) sees the same closed world and the same uses of library
/// names as linting the whole repository.
///
/// The root of a path is the outermost of these candidates, each counted
/// when it contains the path, is not inside (or equal to) a project or
/// gateway collection (the rest of the gateway lies outside it), and its
/// workspace walk reaches the path's project or collection
/// ([`iglint_project::workspace::walk_reaches`]: no hidden or otherwise
/// skipped directory and no linked Git work tree on the way; a symbolic
/// link to a directory on the way counts as reached, since the walk
/// records it and the files behind it are treated as such):
///
/// 1. the configuration file's directory, when a configuration file was
///    found;
/// 2. every Ignition layout root around the path: the directory that
///    holds the layout around the enclosing project or collection
///    (`<root>/projects/<project>`, `<root>/config/resources/<collection>`,
///    widened to its parent when that holds `projects/` too, and
///    `<root>/services/<gateway>/config/resources/<collection>`), and every
///    ancestor of the path that holds such a layout itself (a project in
///    its `projects/`, a collection in its `config/resources/` or in
///    `services/<gateway>/config/resources/`). The ancestors searched end
///    at the top of the outermost Git work tree that contains the path;
///    outside a Git work tree, at the outermost of the configuration file's
///    directory and the working directory that contain the path (only the
///    layout around the enclosing container counts when neither does);
/// 3. the top of every Git work tree that contains the path, when any
///    project or collection lies anywhere below it
///    ([`iglint_project::workspace::holds_container`], searched once per
///    work tree; a search that reaches its directory limit counts as
///    holding one, and the walk of the workspace then marks the closed
///    world unknown at the same limit). Work trees nest: a submodule, or
///    a repository made inside another one's work tree, is part of the
///    enclosing work tree, whose other directories may hold the rest of
///    the gateway, so the outermost one that holds a container wins. A
///    linked work tree (`git worktree add`) is not: it is a checkout of
///    its own wherever it lies, so the search for work trees stops at it,
///    and the walks of the main checkout do not enter it.
///
/// Without any candidate and without a configuration file, the working
/// directory is the root when it contains the path and holds a project or
/// a collection directly, or a `projects/`, `services/` or
/// `config/resources/` directory: a run from `/` or a home directory does
/// not walk it.
///
/// A root inside another root is kept when the other root's walk does
/// not reach it.
///
/// Exclusions and `.gitignore` do not apply to the workspace: an excluded
/// resource still runs on the gateway, so it still counts for the closed
/// world and for the uses of library names. The roots configured with
/// `projects` and `gateways` are added. A directory outside every project
/// and collection is its own root when nothing above it is one. Any other
/// path with no root is returned in [`WorkspaceRoots::unknown`], with its
/// enclosing container (or itself) as the root, so that the run still has
/// the context it had before.
pub fn workspace_roots(settings: &ProjectSettings, paths: &[PathBuf]) -> WorkspaceRoots {
    let base = if settings.root.as_os_str().is_empty() {
        None
    } else {
        Some(absolute(&settings.root))
    };
    let (configured, cwd) = match base {
        Some(base) if settings.root_is_configured => (Some(base), None),
        Some(base) => (None, Some(base)),
        None => (None, None),
    };
    let mut out = WorkspaceRoots::default();
    let mut roots: Vec<PathBuf> = Vec::new();
    // Git work tree -> whether a project or collection lies below it.
    let mut git_holds: HashMap<PathBuf, bool> = HashMap::new();
    let canonical_configured = configured
        .as_deref()
        .and_then(|d| std::fs::canonicalize(d).ok());
    let canonical_cwd = cwd.as_deref().and_then(|d| std::fs::canonicalize(d).ok());
    for path in paths {
        let path = absolute(path);
        // The configuration directory and the working directory as `path`
        // spells them: through the same symbolic links (the working
        // directory has its links resolved, a path given on the command
        // line or by an editor may not).
        let configured = configured
            .as_deref()
            .map(|d| same_directory_above(&path, d, canonical_configured.as_deref()));
        let cwd = cwd
            .as_deref()
            .map(|d| same_directory_above(&path, d, canonical_cwd.as_deref()));
        let container = enclosing_container(&path);
        // A candidate must contain the path, lie outside its container and
        // reach the container in the workspace walk: a root whose walk
        // skips a directory on the way (a hidden directory, a linked Git
        // work tree) would analyse a workspace without the path's own
        // project.
        let usable = |dir: &Path| {
            path.starts_with(dir)
                && container.as_ref().is_none_or(|c| !dir.starts_with(c))
                && !is_container(dir)
                && container
                    .as_ref()
                    .is_none_or(|c| iglint_project::workspace::walk_reaches(dir, c))
        };
        // Every Git work tree around the path, nearest first: a submodule
        // or a nested repository is part of the work tree around it,
        // which may hold the rest of the gateway.
        let work_trees = git_work_trees(&path);
        let git = work_trees.last().cloned();
        let mut candidates: Vec<PathBuf> = Vec::new();
        candidates.extend(configured.clone().filter(|d| usable(d)));
        candidates.extend(
            container
                .as_deref()
                .and_then(layout_root)
                .filter(|d| usable(d)),
        );
        // Where the search for layout roots among the ancestors ends.
        let top = git.clone().or_else(|| {
            [configured.as_ref(), cwd.as_ref()]
                .into_iter()
                .flatten()
                .filter(|d| path.starts_with(d))
                .min_by_key(|d| d.components().count())
                .cloned()
        });
        if let Some(top) = &top {
            for ancestor in path.ancestors().take_while(|a| a.starts_with(top)) {
                if usable(ancestor) && holds_layout_with_container(ancestor) {
                    candidates.push(ancestor.to_path_buf());
                }
            }
        }
        for tree in work_trees.iter().filter(|d| usable(d)) {
            let holds = *git_holds
                .entry(tree.clone())
                .or_insert_with(|| iglint_project::workspace::holds_container(tree) != Some(false));
            if holds {
                candidates.push(tree.clone());
            }
        }
        let root = candidates
            .into_iter()
            .min_by_key(|d| d.components().count())
            .or_else(|| {
                cwd.clone()
                    .filter(|d| usable(d) && holds_ignition_layout(d))
            });
        match root {
            Some(root) => roots.push(root),
            // A directory outside every project and collection is the
            // tree the user asked about.
            None if container.is_none() && path.is_dir() => roots.push(path),
            None => {
                out.unknown.push(path.clone());
                roots.push(container.unwrap_or(path));
            }
        }
    }
    for extra in settings.projects.iter().chain(&settings.gateways) {
        roots.push(absolute(&settings.root.join(extra)));
    }
    roots.sort();
    roots.dedup();
    // Drop roots inside another root whose walk reaches them.
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !kept
            .iter()
            .any(|k| root.starts_with(k) && iglint_project::workspace::walk_reaches(k, &root))
        {
            kept.push(root);
        }
    }
    out.roots = kept;
    out
}

/// `dir` as spelled in `path`: `dir` itself when `path` lies below it,
/// else the ancestor of `path` that is the same directory as `dir` through
/// symbolic links (`canonical_dir` is `dir` with its links resolved), and
/// `dir` when there is none. On macOS the temporary directories
/// (`/var/folders/...`) are links into `/private/var`; the working
/// directory always has its links resolved.
fn same_directory_above(path: &Path, dir: &Path, canonical_dir: Option<&Path>) -> PathBuf {
    if path.starts_with(dir) {
        return dir.to_path_buf();
    }
    canonical_dir
        .filter(|c| std::fs::canonicalize(path).is_ok_and(|p| p.starts_with(c)))
        .and_then(|c| {
            path.ancestors()
                .find(|a| std::fs::canonicalize(a).is_ok_and(|a| a == c))
        })
        .unwrap_or(dir)
        .to_path_buf()
}

/// Whether `dir` holds an Ignition layout with a container in it: a
/// project in `projects/`, or a collection in `config/resources/` or in
/// `services/<gateway>/config/resources/`.
fn holds_layout_with_container(dir: &Path) -> bool {
    let any_child = |d: &Path, test: &dyn Fn(&Path) -> bool| {
        std::fs::read_dir(d).is_ok_and(|entries| entries.flatten().any(|e| test(&e.path())))
    };
    let project = |p: &Path| p.join("project.json").is_file();
    let collection = |c: &Path| c.join("config-mode.json").is_file();
    any_child(&dir.join("projects"), &project)
        || any_child(&dir.join("config/resources"), &collection)
        || any_child(&dir.join("services"), &|g: &Path| {
            any_child(&g.join("config/resources"), &collection)
        })
}

/// The project or gateway collection directory that `path` is in (or is).
fn enclosing_container(path: &Path) -> Option<PathBuf> {
    let start = if path.is_dir() { path } else { path.parent()? };
    start
        .ancestors()
        .find(|d| is_container(d))
        .map(Path::to_path_buf)
}

fn is_container(dir: &Path) -> bool {
    TREE_ROOTS.iter().any(|m| dir.join(m).is_file())
}

/// The directory that holds the Ignition layout around `container`:
/// `<root>/projects/<project>`, `<root>/config/resources/<collection>`
/// (widened to `<root>/..` when it is `services/<gateway>` or when its
/// parent holds `projects/`), or `None` for any other layout.
fn layout_root(container: &Path) -> Option<PathBuf> {
    let parent = container.parent()?;
    let name = |p: &Path| p.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    if container.join("project.json").is_file() {
        return (name(parent).as_deref() == Some("projects"))
            .then(|| parent.parent().map(Path::to_path_buf))
            .flatten();
    }
    if name(parent).as_deref() != Some("resources") {
        return None;
    }
    let config = parent.parent()?;
    if name(config).as_deref() != Some("config") {
        return None;
    }
    let data = config.parent()?;
    if data.join("projects").is_dir() {
        return Some(data.to_path_buf());
    }
    let up = data.parent();
    Some(
        match up {
            Some(up) if name(up).as_deref() == Some("services") => up.parent().unwrap_or(up),
            Some(up) if up.join("projects").is_dir() => up,
            _ => data,
        }
        .to_path_buf(),
    )
}

/// Whether `dir` holds a project or a gateway collection directly, or a
/// `projects/`, `services/` or `config/resources/` directory.
fn holds_ignition_layout(dir: &Path) -> bool {
    ["projects", "services", "config/resources"]
        .iter()
        .any(|d| dir.join(d).is_dir())
        || std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| e.file_type().is_ok_and(|t| t.is_dir()) && is_container(&e.path()))
        })
}

/// The top directories of the Git work trees that contain `path`, nearest
/// first: every ancestor with a `.git` directory or file. More than one
/// means nested work trees: a submodule (its `.git` is a file that points
/// into the enclosing repository's `.git/modules`), or a repository made
/// inside another one's work tree.
///
/// A linked work tree (`git worktree add`, see
/// [`iglint_project::workspace::GitTop::LinkedWorktree`]) is a checkout of
/// its own: the search stops there, wherever it lies (`.worktrees/x` inside
/// the main checkout is not part of the main checkout's work tree).
fn git_work_trees(path: &Path) -> Vec<PathBuf> {
    use iglint_project::workspace::{GitTop, git_top};
    let mut out = Vec::new();
    for dir in path.ancestors() {
        match git_top(dir) {
            GitTop::None => {}
            GitTop::Repository => out.push(dir.to_path_buf()),
            GitTop::LinkedWorktree => {
                out.push(dir.to_path_buf());
                break;
            }
        }
    }
    out
}

/// Why `file` is not linted (excluded or whitelisted), if it is not.
pub fn skip_reason(settings: &ProjectSettings, file: &Path) -> Option<SkipReason> {
    Skip::new(settings).reason(file)
}

/// `exclude` and whitelist checks with the whitelist resolved once, so that
/// a walk costs one hash lookup per file instead of normalising every
/// whitelist entry for every file. Same results as
/// [`ProjectSettings::is_excluded`] and [`skip_reason`].
struct Skip {
    root: PathBuf,
    exclude: Vec<String>,
    cwd: PathBuf,
    /// Whitelist entries as given.
    listed: HashSet<PathBuf>,
    /// Whitelist entries relative to the root, `/`-separated.
    listed_relative: HashSet<String>,
    /// Whitelist entries made absolute.
    listed_absolute: HashSet<PathBuf>,
}

impl Skip {
    fn new(settings: &ProjectSettings) -> Skip {
        let cwd = std::env::current_dir().unwrap_or_default();
        Skip {
            root: settings.root.clone(),
            exclude: settings.exclude.clone(),
            listed: settings.whitelist.iter().cloned().collect(),
            listed_relative: settings
                .whitelist
                .iter()
                .map(|w| settings.relative_path(w))
                .collect(),
            listed_absolute: settings
                .whitelist
                .iter()
                .map(|w| absolute_from(&cwd, w))
                .collect(),
            cwd,
        }
    }

    fn relative(&self, file: &Path) -> String {
        slash_path(file.strip_prefix(&self.root).unwrap_or(file))
    }

    fn excluded_by_glob(&self, relative: &str) -> bool {
        self.exclude
            .iter()
            .any(|p| crate::settings::glob_matches(p, relative))
    }

    fn is_excluded(&self, file: &Path) -> bool {
        if self.exclude.is_empty() && self.listed.is_empty() {
            return false;
        }
        let relative = self.relative(file);
        self.excluded_by_glob(&relative)
            || self.listed.contains(file)
            || self.listed_relative.contains(&relative)
    }

    fn reason(&self, file: &Path) -> Option<SkipReason> {
        if !self.exclude.is_empty() && self.excluded_by_glob(&self.relative(file)) {
            return Some(SkipReason::Excluded);
        }
        if !self.listed_absolute.is_empty()
            && self
                .listed_absolute
                .contains(&absolute_from(&self.cwd, file))
        {
            return Some(SkipReason::Whitelisted);
        }
        None
    }
}

/// `path` with `/` between components on every platform: `a/b/c.py`,
/// `/abs/c.py`, `C:/abs/c.py` (Windows). Used for everything users see or
/// store (displayed paths, baseline paths, glob matching).
pub fn slash_path(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => {
                out.push_str(&prefix.as_os_str().to_string_lossy());
            }
            std::path::Component::RootDir => out.push('/'),
            other => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&other.as_os_str().to_string_lossy());
            }
        }
    }
    out
}

/// Absolute, lexically normalised form of `path` (no symlink resolution, so
/// it works for paths that do not exist yet).
pub fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return absolute_from(Path::new(""), path);
    }
    absolute_from(&std::env::current_dir().unwrap_or_default(), path)
}

/// [`absolute`] with the current directory given.
fn absolute_from(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(path: &str, manifest_dirs: &[&str]) -> Option<FileKind> {
        classify_with(Path::new(path), &mut |dir| {
            manifest_dirs.iter().any(|d| Path::new(d) == dir)
        })
    }

    #[test]
    fn classification() {
        assert_eq!(kind("p/views/Main/view.json", &[]), Some(FileKind::View));
        assert_eq!(
            kind("p/ignition/script-python/a/b/code.py", &[]),
            Some(FileKind::ScriptModule)
        );
        assert_eq!(
            kind(
                "p/ignition/timer/t/handleTimerEvent.py",
                &["p/ignition/timer/t"]
            ),
            Some(FileKind::ResourceScript)
        );
        assert_eq!(kind("tools/build.py", &[]), Some(FileKind::Python));
        assert_eq!(kind("p/x/resource.json", &[]), Some(FileKind::Manifest));
        assert_eq!(kind("p/project.json", &[]), Some(FileKind::Manifest));
        assert_eq!(
            kind("p/style-classes/A/style.json", &["p/style-classes/A"]),
            Some(FileKind::ResourceJson)
        );
        assert_eq!(kind("package.json", &[]), Some(FileKind::Json));
        assert_eq!(kind("p/views/Main/thumbnail.png", &[]), None);
        assert_eq!(kind("p/stylesheet.css", &[]), None);
    }

    #[test]
    fn ignition_trees_are_found_through_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("projects/p/a/b")).unwrap();
        std::fs::create_dir_all(root.join("config/x")).unwrap();
        std::fs::create_dir_all(root.join("tools/sub")).unwrap();
        std::fs::write(root.join("projects/p/project.json"), "{}").unwrap();
        std::fs::write(root.join("config/config-mode.json"), "{}").unwrap();
        let mut trees = IgnitionTrees::default();
        assert!(trees.contains(&root.join("projects/p/a/b")));
        assert!(trees.contains(&root.join("projects/p/a")));
        assert!(trees.contains(&root.join("projects/p")));
        assert!(!trees.contains(&root.join("projects")));
        assert!(trees.contains(&root.join("config/x")));
        assert!(!trees.contains(&root.join("tools/sub")));
        assert!(!trees.contains(&root.join("tools")));
    }

    #[test]
    fn absolute_normalises() {
        let root = if cfg!(windows) { r"C:\" } else { "/" };
        assert_eq!(
            absolute(&Path::new(root).join("a/./b/../c")),
            Path::new(root).join("a").join("c")
        );
    }
}
