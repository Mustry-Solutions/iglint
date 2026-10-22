//! Finding the files to lint.
//!
//! Paths given by the user are walked recursively (respecting `.gitignore`
//! when configured). Every file is classified by [`FileKind::classify`];
//! files that are not Ignition resources are skipped silently while walking,
//! but linted when named explicitly.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

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
/// hidden directories, `.gitignore`d files (when configured) and excluded
/// paths; explicitly named files are still checked against `exclude` and the
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
    for path in paths {
        if path.is_file() {
            if !seen.insert(path.clone()) {
                continue;
            }
            if let Some(reason) = skip_reason(settings, path) {
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
        let exclude_settings = settings.clone();
        builder.filter_entry(move |entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|t| t.is_dir())
                || !exclude_settings.is_excluded(entry.path())
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
            let Some(kind) = classify_with(&file, &mut manifest_in) else {
                continue;
            };
            if !kind.discovered_by_walk() || !seen.insert(file.clone()) {
                continue;
            }
            match skip_reason(settings, &file) {
                Some(reason) => discovery.skipped.push((file, reason)),
                None => discovery.files.push((file, kind)),
            }
        }
    }
    discovery.files.sort();
    discovery.skipped.sort_by(|a, b| a.0.cmp(&b.0));
    discovery
}

/// Why `file` is not linted (excluded or whitelisted), if it is not.
pub fn skip_reason(settings: &ProjectSettings, file: &Path) -> Option<SkipReason> {
    let relative = settings.relative_path(file);
    if settings
        .exclude
        .iter()
        .any(|p| crate::settings::glob_matches(p, &relative))
    {
        return Some(SkipReason::Excluded);
    }
    if !settings.whitelist.is_empty() {
        let absolute = absolute(file);
        if settings
            .whitelist
            .iter()
            .any(|w| absolute == crate::discovery::absolute(w))
        {
            return Some(SkipReason::Whitelisted);
        }
    }
    None
}

/// Absolute, lexically normalised form of `path` (no symlink resolution, so
/// it works for paths that do not exist yet).
pub fn absolute(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
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
    fn absolute_normalises() {
        assert_eq!(absolute(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }
}
