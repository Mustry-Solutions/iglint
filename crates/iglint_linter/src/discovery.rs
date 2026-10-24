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
/// hidden directories, `.gitignore`d files (when configured), excluded
/// paths and Python files outside any Ignition tree; explicitly named files are still checked against `exclude` and the
/// whitelist so that editor and pre-commit integrations honour them.
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
            .hidden(true)
            .git_ignore(settings.respect_gitignore)
            .git_global(settings.respect_gitignore)
            .git_exclude(settings.respect_gitignore)
            .ignore(settings.respect_gitignore)
            .parents(settings.respect_gitignore)
            .require_git(false)
            .sort_by_file_name(|a, b| a.cmp(b));
        // Prune excluded directories early instead of filtering their files.
        let dir_skip = Arc::clone(&skip);
        builder.filter_entry(move |entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || !dir_skip.is_excluded(entry.path())
        });
        for entry in builder.build() {
            let entry = match entry {
                Ok(entry) => entry,
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
/// names as linting the whole repository. The root of a path is, in this
/// order:
///
/// 1. the outermost of the configuration file's directory (when a
///    configuration file was found) and the directory that holds the
///    Ignition layout around the path: `<root>/projects/<project>`,
///    `<root>/config/resources/<collection>` (a gateway data directory,
///    widened to its parent when that holds `projects/` too) and
///    `<root>/services/<gateway>/config/resources/<collection>`;
/// 2. the Git work tree that contains the path;
/// 3. without a configuration file, the working directory, when it
///    contains the path.
///
/// The Git work tree and the working directory count only when they hold
/// a project or a gateway collection directly, or a `projects/`,
/// `services/` or `config/resources/` directory: a run from `/` or a home
/// directory does not walk it, and a repository that is not an Ignition
/// workspace (a tool's own test fixtures) does not become one.
///
/// A candidate inside (or equal to) a project or a gateway collection is
/// not a root: the rest of the gateway lies outside it. Exclusions and
/// `.gitignore` do not apply to the workspace: an excluded resource still
/// runs on the gateway, so it still counts for the closed world and for
/// the uses of library names. The roots configured with `projects` and
/// `gateways` are added. A directory outside every project and collection
/// is its own root when nothing above it is one. Any other path with no
/// root is returned in
/// [`WorkspaceRoots::unknown`], with its enclosing container (or itself)
/// as the root, so that the run still has the context it had before.
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
    for path in paths {
        let path = absolute(path);
        let container = enclosing_container(&path);
        let usable = |dir: &Path| {
            path.starts_with(dir)
                && container.as_ref().is_none_or(|c| !dir.starts_with(c))
                && !is_container(dir)
        };
        let layout = container.as_deref().and_then(layout_root);
        let first = [
            configured.clone().filter(|d| usable(d)),
            layout.filter(|d| usable(d)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|d| d.components().count());
        let root = first
            .or_else(|| git_work_tree(&path).filter(|d| usable(d) && holds_ignition_layout(d)))
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
    // Drop roots inside another root.
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !kept.iter().any(|k| root.starts_with(k)) {
            kept.push(root);
        }
    }
    out.roots = kept;
    out
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

/// The top directory of the Git work tree that contains `path`: the
/// nearest ancestor with a `.git` directory or file.
fn git_work_tree(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
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
        assert_eq!(absolute(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }
}
