//! A workspace folder: its configuration and its live workspace.

use std::path::{Path, PathBuf};

use iglint_config::{ConfigSource, LoadOptions};
use iglint_linter::Diagnostic;
use iglint_linter::baseline::{Baseline, BaselineOrigin};
use iglint_linter::live::LiveWorkspace;
use iglint_linter::rules::REGISTRY;
use iglint_linter::settings::ProjectSettings;

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
}

impl Folder {
    /// Loads the configuration of `root` and discovers its workspace. A
    /// broken configuration falls back to the defaults, with a notice.
    pub fn load(root: &Path) -> Folder {
        let mut notices = Vec::new();
        let options = LoadOptions {
            cwd: root.to_path_buf(),
            start: Some(root.to_path_buf()),
            ..LoadOptions::default()
        };
        let configuration = match iglint_config::load(&options, &REGISTRY) {
            Ok(c) => Some(c),
            Err(error) => {
                notices.push(format!("iglint configuration error: {error}"));
                iglint_config::load(
                    &LoadOptions {
                        isolated: true,
                        ..options
                    },
                    &REGISTRY,
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
        let baseline_origin = BaselineOrigin::new(&settings, &[root.to_path_buf()]);
        let baseline = baseline_path
            .map(|configured| configured.unwrap_or_else(|| baseline_origin.default_file()))
            .filter(|p| p.is_file())
            .and_then(|p| match Baseline::load(&p) {
                Ok(b) => Some(b),
                Err(error) => {
                    notices.push(format!("cannot read baseline {}: {error}", p.display()));
                    None
                }
            });
        let mut roots: Vec<PathBuf> = settings
            .projects
            .iter()
            .chain(&settings.gateways)
            .cloned()
            .collect();
        if roots.is_empty() {
            roots.push(root.to_path_buf());
        }
        Folder {
            root: root.to_path_buf(),
            config,
            live: LiveWorkspace::new(settings, roots),
            baseline,
            baseline_origin,
            notices,
            adhoc: false,
            files: None,
            pending: Vec::new(),
            pending_all: false,
        }
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
        if self.config.as_deref() == Some(path) {
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
