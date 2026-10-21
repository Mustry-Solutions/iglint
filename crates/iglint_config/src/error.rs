use std::fmt;
use std::path::{Path, PathBuf};

/// A configuration problem, with the file and position it was found at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub path: Option<PathBuf>,
    /// One-based line and column.
    pub position: Option<(u32, u32)>,
    pub message: String,
}

impl ConfigError {
    pub(crate) fn new(path: &Path, position: Option<(u32, u32)>, message: String) -> Self {
        ConfigError {
            path: Some(path.to_path_buf()),
            position,
            message,
        }
    }

    /// An error in a command-line option.
    pub(crate) fn cli(message: String) -> Self {
        ConfigError {
            path: None,
            position: None,
            message,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.path, self.position) {
            (Some(path), Some((line, column))) => {
                write!(f, "{}:{line}:{column}: {}", path.display(), self.message)
            }
            (Some(path), None) => write!(f, "{}: {}", path.display(), self.message),
            (None, _) => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigError {}
