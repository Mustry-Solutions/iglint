//! A workspace folder: its configuration and its live workspace.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use iglint_config::{ConfigSource, LoadOptions};
use iglint_linter::Diagnostic;
use iglint_linter::baseline::{Baseline, BaselineOrigin};
use iglint_linter::live::LiveWorkspace;
use iglint_linter::rules::REGISTRY;
use iglint_linter::settings::ProjectSettings;
use rustc_hash::FxHashMap;

/// Configuration file names whose change reloads the folder.
pub const CONFIG_NAMES: [&str; 4] = [
    "iglint.toml",
    ".iglint.toml",
    "pyproject.toml",
    "rule_config.json",
];

pub struct Folder {
    pub root: PathBuf,
    /// Where the configuration came from (`None`: defaults).
    pub config: Option<PathBuf>,
    pub live: LiveWorkspace,
    pub baseline: Option<Baseline>,
    /// Where the baseline's paths start (as for `iglint check` run in the
    /// folder).
    baseline_origin: BaselineOrigin,
    /// Messages for the user (configuration errors and notices).
    pub notices: Vec<String>,
    /// Created for a file outside every workspace folder.
    pub adhoc: bool,
    /// Every lintable file, computed on first use.
    files: Option<Vec<PathBuf>>,
    /// Published files whose findings may have changed and that are not
    /// checked again yet: dependents of editor text given to the live
    /// workspace before the run trigger fired, and what a check left when
    /// a message arrived (review round 30). Possibly with repeats.
    pub pending: Vec<PathBuf>,
    /// Every published file is pending (cheaper than listing them).
    pub pending_all: bool,
    /// The open documents whose editor text the live workspace has, with
    /// the generation of that text (review round 31, B1: every live
    /// workspace whose index holds a document gets its text, not only
    /// the folder that publishes it).
    pub applied: FxHashMap<PathBuf, u64>,
    /// Every file the configuration and the baseline could come from
    /// ([`Configured::sources`]), with its [`FileStamp`] when it was
    /// loaded.
    config_seen: Vec<(PathBuf, Option<FileStamp>)>,
}

impl Folder {
    /// Loads the configuration of `root` and discovers its workspace. A
    /// broken configuration falls back to the defaults, with a notice.
    pub fn load(root: &Path) -> Folder {
        let configured = Configured::load(root, true);
        let mut notices = configured.notices;
        let baseline = match configured.baseline {
            Ok(b) => b,
            Err(error) => {
                notices.push(error);
                None
            }
        };
        let config_seen = configured
            .sources
            .into_iter()
            .map(|path| {
                let stamp = file_stamp(&path);
                (path, stamp)
            })
            .collect();
        Folder {
            root: root.to_path_buf(),
            config: configured.config,
            live: LiveWorkspace::new(configured.settings, configured.roots),
            baseline,
            baseline_origin: configured.baseline_origin,
            notices,
            adhoc: false,
            files: None,
            pending: Vec::new(),
            pending_all: false,
            applied: FxHashMap::default(),
            config_seen,
        }
    }

    /// Whether a file the configuration could come from changed on disk
    /// since it was loaded (a change the client did not report).
    pub fn config_changed(&self) -> bool {
        self.config_seen
            .iter()
            .any(|(path, stamp)| file_stamp(path) != *stamp)
    }

    /// Whether files wait to be checked again.
    pub fn has_pending(&self) -> bool {
        self.pending_all || !self.pending.is_empty()
    }

    /// Every file a check of the folder would lint.
    pub fn workspace_files(&mut self) -> &[PathBuf] {
        self.files.get_or_insert_with(|| self.live.lintable_files())
    }

    /// Whether `path` belongs to this folder: it is below the folder and
    /// the folder's workspace walk reached its project or collection (a
    /// file of a linked Git work tree or of a hidden directory below the
    /// folder gets a folder of its own).
    pub fn owns(&self, path: &Path) -> bool {
        self.surrounds(path) && self.live.reaches(path)
    }

    /// Whether `path` is below the folder or one of its roots, reached by
    /// its workspace walk or not (a new project's files are not reached
    /// until the folder is discovered again).
    pub fn surrounds(&self, path: &Path) -> bool {
        path.starts_with(&self.root) || self.live.covers(path)
    }

    /// Whether a change of `path` (a configuration file) affects this folder.
    pub fn config_affected_by(&self, path: &Path) -> bool {
        if self.config.as_deref() == Some(path) || self.config_seen.iter().any(|(p, _)| p == path) {
            return true;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        CONFIG_NAMES.contains(&name) && path.parent().is_some_and(|dir| self.root.starts_with(dir))
    }

    /// Removes findings covered by the baseline.
    pub fn filter_baseline(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        match &self.baseline {
            Some(b) if !diagnostics.is_empty() => b.filter(&self.baseline_origin, diagnostics).new,
            _ => diagnostics,
        }
    }
}

/// The configuration of a folder, loaded the way `iglint check` run in
/// the folder loads it: the settings, the roots to lint, the baseline.
pub struct Configured {
    pub settings: ProjectSettings,
    /// Where the configuration came from (`None`: defaults).
    pub config: Option<PathBuf>,
    /// The paths to lint: the configured projects and gateways, else the
    /// folder.
    pub roots: Vec<PathBuf>,
    /// The baseline, if there is one, or why it cannot be read.
    pub baseline: Result<Option<Baseline>, String>,
    pub baseline_origin: BaselineOrigin,
    /// The configured baseline file, or the default one.
    pub baseline_path: Option<PathBuf>,
    /// The configuration names a baseline file (`iglint check` fails when
    /// it is missing).
    pub baseline_required: bool,
    /// Every file the configuration and the baseline could come from: each
    /// configuration name in the folder and the directories above it, every
    /// file the load looked at or read (the configuration file and every
    /// file it extends, `rule_config.json`, the whitelist files), and the
    /// baseline file (review rounds 32, N1, and 33, B1).
    pub sources: Vec<PathBuf>,
    pub notices: Vec<String>,
    /// The configuration could not be read (`lenient` used the defaults).
    pub error: Option<String>,
}

impl Configured {
    /// Loads the configuration of `root`. With `lenient`, a broken
    /// configuration falls back to the defaults (with a notice in
    /// [`Configured::error`] and the notices); otherwise the defaults are
    /// still returned, and the caller decides with [`Configured::error`].
    pub fn load(root: &Path, lenient: bool) -> Configured {
        let mut notices = Vec::new();
        let mut error = None;
        let options = LoadOptions {
            cwd: root.to_path_buf(),
            start: Some(root.to_path_buf()),
            ..LoadOptions::default()
        };
        let mut read: Vec<PathBuf> = Vec::new();
        let configuration = match iglint_config::load_recording(&options, &REGISTRY, &mut read) {
            Ok(c) => Some(c),
            Err(e) => {
                let message = format!("iglint configuration error: {e}");
                if lenient {
                    notices.push(message.clone());
                }
                error = Some(message);
                iglint_config::load_recording(
                    &LoadOptions {
                        isolated: true,
                        ..options
                    },
                    &REGISTRY,
                    &mut read,
                )
                .ok()
            }
        };
        let (settings, config, baseline_path) = match configuration {
            Some(c) => {
                notices.extend(c.notices.iter().cloned());
                let config = match &c.source {
                    ConfigSource::Defaults => None,
                    other => other.path().map(Path::to_path_buf),
                };
                (c.settings, config, Some(c.baseline.clone()))
            }
            None => (
                ProjectSettings {
                    root: root.to_path_buf(),
                    ..ProjectSettings::default()
                },
                None,
                None,
            ),
        };
        let baseline_required = matches!(baseline_path, Some(Some(_)));
        let baseline_origin = BaselineOrigin::new(&settings, &[root.to_path_buf()]);
        let baseline_path = baseline_path
            .map(|configured| configured.unwrap_or_else(|| baseline_origin.default_file()));
        let baseline = match baseline_path.as_ref().filter(|p| p.is_file()) {
            None => Ok(None),
            Some(p) => Baseline::load(p)
                .map(Some)
                .map_err(|e| format!("cannot read baseline {}: {e}", p.display())),
        };
        let mut roots: Vec<PathBuf> = settings
            .projects
            .iter()
            .chain(&settings.gateways)
            .cloned()
            .collect();
        if roots.is_empty() {
            roots.push(root.to_path_buf());
        }
        let mut sources: Vec<PathBuf> = config
            .iter()
            .cloned()
            .chain(read)
            .chain(
                root.ancestors()
                    .flat_map(|dir| CONFIG_NAMES.iter().map(move |name| dir.join(name))),
            )
            .chain(baseline_path.iter().cloned())
            .map(|p| iglint_linter::discovery::absolute(&p))
            .collect();
        let mut seen = rustc_hash::FxHashSet::default();
        sources.retain(|p| seen.insert(p.clone()));
        Configured {
            settings,
            config,
            roots,
            baseline,
            baseline_origin,
            baseline_path,
            baseline_required,
            sources,
            notices,
            error,
        }
    }

    /// Removes findings covered by the baseline.
    pub fn filter_baseline(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        match &self.baseline {
            Ok(Some(b)) if !diagnostics.is_empty() => {
                b.filter(&self.baseline_origin, diagnostics).new
            }
            _ => diagnostics,
        }
    }
}

/// What a configuration file is on disk, by its path: whether the path is
/// a symbolic link and its target, and the file it reaches: identity
/// (device and inode, or the canonical path), size and times (review
/// round 33: a file replaced by another one, or a link retargeted, with
/// the same size and modification time is a change).
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    link: Option<PathBuf>,
    file: Option<(Identity, u64, Option<SystemTime>, Option<SystemTime>)>,
}

#[cfg(unix)]
type Identity = (u64, u64, i64, i64);
#[cfg(not(unix))]
type Identity = Option<PathBuf>;

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let own = std::fs::symlink_metadata(path).ok()?;
    let link = own
        .file_type()
        .is_symlink()
        .then(|| std::fs::read_link(path).unwrap_or_default());
    let file = std::fs::metadata(path).ok().map(|meta| {
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
        };
        #[cfg(not(unix))]
        let identity = std::fs::canonicalize(path).ok();
        (
            identity,
            meta.len(),
            meta.modified().ok(),
            meta.created().ok(),
        )
    });
    Some(FileStamp { link, file })
}
