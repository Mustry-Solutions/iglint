//! iglint configuration: `iglint.toml`, `.iglint.toml`, `[tool.iglint]` in
//! `pyproject.toml`, and legacy ign-lint `rule_config.json`.
//!
//! [`load`] finds the configuration for a run, validates it against the rule
//! registry and resolves it, together with command-line overrides, into the
//! [`ProjectSettings`] the linter uses.
//!
//! Rule selection follows ruff: the default set is every rule with
//! `default_enabled`; each configuration layer (an extended file, the file
//! itself, the command line) applies `select`, `extend-select`, `ignore` and
//! `extend-ignore`, where the most specific selector wins and `ignore` wins
//! ties. A layer with `select` starts from an empty set. `[rules.<rule>]`
//! `enabled` has the last word.

pub mod compat;
mod error;
pub mod migrate;
mod resolve;
mod schema;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use error::ConfigError;
use iglint_linter::Registry;
use iglint_linter::settings::ProjectSettings;

/// Configuration file names, in the order they are tried in each directory.
pub const CONFIG_FILES: [&str; 3] = ["iglint.toml", ".iglint.toml", "pyproject.toml"];

/// Values accepted by `output-format` (and `--output-format`).
pub const OUTPUT_FORMATS: [&str; 11] = [
    "text",
    "concise",
    "grouped",
    "json",
    "json-lines",
    "sarif",
    "github",
    "azure",
    "junit",
    "pylint",
    "gitlab",
];

/// Settings given on the command line, applied after the files.
#[derive(Clone, Debug, Default)]
pub struct CliOverrides {
    pub select: Option<Vec<String>>,
    pub extend_select: Vec<String>,
    pub ignore: Vec<String>,
    pub extend_ignore: Vec<String>,
    pub strict: Option<bool>,
    /// `--whitelist`: an ign-lint whitelist file, relative to the cwd.
    pub whitelist: Option<PathBuf>,
}

/// How to find and read the configuration.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    /// Relative paths on the command line are relative to this.
    pub cwd: PathBuf,
    /// Where the upward search for a configuration file starts, usually the
    /// first path being linted.
    pub start: Option<PathBuf>,
    /// `--config`: use this file and do not search.
    pub config_file: Option<PathBuf>,
    /// `--isolated`: ignore every configuration file.
    pub isolated: bool,
    pub cli: CliOverrides,
}

/// Where the configuration came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigSource {
    /// No file: defaults (and command-line options) only.
    Defaults,
    /// `iglint.toml` or `.iglint.toml`.
    Toml(PathBuf),
    /// `[tool.iglint]` in this `pyproject.toml`.
    PyProject(PathBuf),
    /// A legacy ign-lint `rule_config.json`.
    IgnLint(PathBuf),
}

impl ConfigSource {
    pub fn path(&self) -> Option<&Path> {
        match self {
            ConfigSource::Defaults => None,
            ConfigSource::Toml(path)
            | ConfigSource::PyProject(path)
            | ConfigSource::IgnLint(path) => Some(path),
        }
    }
}

/// A loaded and resolved configuration.
#[derive(Clone, Debug)]
pub struct Configuration {
    pub source: ConfigSource,
    pub settings: ProjectSettings,
    /// `baseline`, absolute.
    pub baseline: Option<PathBuf>,
    /// `cache-dir`, absolute.
    pub cache_dir: Option<PathBuf>,
    /// `output-format`, one of [`OUTPUT_FORMATS`].
    pub output_format: Option<String>,
    /// Messages for the user: deprecations and settings that could not be
    /// carried over from ign-lint.
    pub notices: Vec<String>,
}

/// Finds the configuration file for a run: `--config`, or the first
/// `iglint.toml`, `.iglint.toml` or `pyproject.toml` with `[tool.iglint]`
/// from `start` upward, or else a legacy `rule_config.json` from `start`
/// upward.
pub fn find_config(options: &LoadOptions) -> Result<ConfigSource, ConfigError> {
    find_config_recording(options, &mut Vec::new())
}

/// [`find_config`], adding every file it looks at or reads to `read`.
fn find_config_recording(
    options: &LoadOptions,
    read: &mut Vec<PathBuf>,
) -> Result<ConfigSource, ConfigError> {
    if let Some(file) = &options.config_file {
        let path = absolutize(&options.cwd, file);
        read.push(path.clone());
        if !path.is_file() {
            return Err(ConfigError::new(
                &path,
                None,
                "configuration file not found".to_owned(),
            ));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        return Ok(if name.ends_with(".json") {
            ConfigSource::IgnLint(path)
        } else if name == "pyproject.toml" {
            ConfigSource::PyProject(path)
        } else {
            ConfigSource::Toml(path)
        });
    }
    if options.isolated {
        return Ok(ConfigSource::Defaults);
    }
    let start = options
        .start
        .as_ref()
        .map(|s| absolutize(&options.cwd, s))
        .unwrap_or_else(|| options.cwd.clone());
    let start = if start.is_file() {
        start.parent().map(Path::to_path_buf).unwrap_or(start)
    } else {
        start
    };
    for dir in start.ancestors() {
        for name in CONFIG_FILES {
            let candidate = dir.join(name);
            read.push(candidate.clone());
            if !candidate.is_file() {
                continue;
            }
            if name == "pyproject.toml" {
                if schema::pyproject_has_iglint(&candidate)? {
                    return Ok(ConfigSource::PyProject(candidate));
                }
            } else {
                return Ok(ConfigSource::Toml(candidate));
            }
        }
    }
    for dir in start.ancestors() {
        let candidate = dir.join(compat::RULE_CONFIG_FILE);
        read.push(candidate.clone());
        if candidate.is_file() {
            return Ok(ConfigSource::IgnLint(candidate));
        }
    }
    Ok(ConfigSource::Defaults)
}

/// Finds, reads, validates and resolves the configuration.
pub fn load(options: &LoadOptions, registry: &Registry) -> Result<Configuration, ConfigError> {
    load_recording(options, registry, &mut Vec::new())
}

/// [`load`], adding to `read` every file the load looked at or read, also
/// when it fails: the configuration candidates, the configuration file and
/// every file it extends, `rule_config.json` and the whitelist files.
pub fn load_recording(
    options: &LoadOptions,
    registry: &Registry,
    read: &mut Vec<PathBuf>,
) -> Result<Configuration, ConfigError> {
    let source = find_config_recording(options, read)?;
    resolve::load_source(source, options, registry, read)
}

/// Joins `path` to `base` unless it is absolute, and normalises `.`/`..`.
pub fn absolutize(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
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
