//! Diagnostics, severities and fixes.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iglint_text::{LineIndex, TextEdit, TextRange};

use crate::registry::RuleCode;

/// How serious a finding is. Ordered from least to most severe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Severity> {
        match value.to_ascii_lowercase().as_str() {
            "info" | "information" | "note" => Some(Severity::Info),
            "warning" | "warn" => Some(Severity::Warning),
            "error" | "err" => Some(Severity::Error),
            _ => None,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a fix can be applied without changing behaviour the user relies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Applicability {
    /// Shown to the user, never applied automatically.
    DisplayOnly,
    /// Applied only with `--unsafe-fixes`.
    Unsafe,
    /// Applied with `--fix`.
    Safe,
}

/// A source file known to the linter. Cheap to clone.
#[derive(Clone, Debug)]
pub struct SourceFile {
    inner: Arc<SourceFileInner>,
}

#[derive(Debug)]
struct SourceFileInner {
    /// Path as given or discovered, used for display.
    path: PathBuf,
    text: Arc<str>,
    line_index: std::sync::OnceLock<LineIndex>,
}

impl SourceFile {
    pub fn new(path: impl Into<PathBuf>, text: impl Into<Arc<str>>) -> Self {
        SourceFile {
            inner: Arc::new(SourceFileInner {
                path: path.into(),
                text: text.into(),
                line_index: std::sync::OnceLock::new(),
            }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn text(&self) -> &str {
        &self.inner.text
    }

    pub fn shared_text(&self) -> Arc<str> {
        Arc::clone(&self.inner.text)
    }

    pub fn line_index(&self) -> &LineIndex {
        self.inner
            .line_index
            .get_or_init(|| LineIndex::new(&self.inner.text))
    }
}

impl PartialEq for SourceFile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner) || self.inner.path == other.inner.path
    }
}

impl Eq for SourceFile {}

impl Hash for SourceFile {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.path.hash(state);
    }
}

/// An edit to one file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileEdit {
    pub path: PathBuf,
    pub edit: TextEdit,
}

/// A proposed change that resolves a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub message: String,
    pub applicability: Applicability,
    pub edits: Vec<FileEdit>,
}

impl Fix {
    pub fn safe(message: impl Into<String>, edits: Vec<FileEdit>) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::Safe,
            edits,
        }
    }

    pub fn unsafe_edit(message: impl Into<String>, edits: Vec<FileEdit>) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::Unsafe,
            edits,
        }
    }

    pub fn display_only(message: impl Into<String>, edits: Vec<FileEdit>) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::DisplayOnly,
            edits,
        }
    }
}

/// A secondary location that helps explain a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub file: SourceFile,
    pub range: TextRange,
    pub message: String,
}

/// One finding.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub rule: RuleCode,
    pub severity: Severity,
    pub message: String,
    pub file: SourceFile,
    /// Byte range in `file` on disk.
    pub range: TextRange,
    /// Logical location inside a resource, e.g. `root/Header/Title.props.text`
    /// for views, or a script's qualified function name.
    pub node_path: Option<String>,
    /// The node path in ign-lint's format, for rules with an ign-lint
    /// equivalent (`iglint ign-lint` prints it instead of `node_path`).
    pub ign_lint_path: Option<String>,
    /// ign-lint's wording of the finding, when the rule reproduces it
    /// (`iglint ign-lint` prints it instead of `message`).
    pub ign_lint_message: Option<String>,
    pub labels: Vec<Label>,
    pub fix: Option<Fix>,
    /// Text that identifies the finding independently of its line, used for
    /// the baseline fingerprint. Rules set this when the message alone is not
    /// stable (for example when it contains a count).
    pub fingerprint_key: Option<String>,
}

impl Diagnostic {
    pub fn new(
        rule: RuleCode,
        severity: Severity,
        message: impl Into<String>,
        file: SourceFile,
        range: TextRange,
    ) -> Self {
        Diagnostic {
            rule,
            severity,
            message: message.into(),
            file,
            range,
            node_path: None,
            ign_lint_path: None,
            ign_lint_message: None,
            labels: Vec::new(),
            fix: None,
            fingerprint_key: None,
        }
    }

    #[must_use]
    pub fn with_node_path(mut self, path: impl Into<String>) -> Self {
        self.node_path = Some(path.into());
        self
    }

    #[must_use]
    pub fn with_fix(mut self, fix: Fix) -> Self {
        self.fix = Some(fix);
        self
    }

    #[must_use]
    pub fn with_label(mut self, label: Label) -> Self {
        self.labels.push(label);
        self
    }

    #[must_use]
    pub fn with_fingerprint_key(mut self, key: impl Into<String>) -> Self {
        self.fingerprint_key = Some(key.into());
        self
    }

    /// A fingerprint that survives unrelated edits elsewhere in the file:
    /// rule, file path, logical node path and message (or explicit key), plus
    /// the flagged source text. Line numbers are deliberately excluded.
    pub fn fingerprint(&self, relative_path: &str) -> u64 {
        // Every part is length-prefixed with a fixed-width integer so the hash
        // is identical on every platform and Rust version.
        fn part(hasher: &mut Fnv64, bytes: &str) {
            hasher.write(&(bytes.len() as u64).to_le_bytes());
            hasher.write(bytes.as_bytes());
        }
        let mut hasher = Fnv64::default();
        part(&mut hasher, self.rule.code());
        part(&mut hasher, &relative_path.replace('\\', "/"));
        part(&mut hasher, self.node_path.as_deref().unwrap_or(""));
        part(
            &mut hasher,
            self.fingerprint_key.as_deref().unwrap_or(&self.message),
        );
        let flagged = self
            .file
            .text()
            .get(self.range.to_std())
            .unwrap_or_default();
        part(&mut hasher, flagged);
        hasher.finish()
    }

    /// Sort key giving deterministic output: path, position, rule.
    pub fn sort_key(&self) -> (&Path, TextRange, &'static str, &str) {
        (
            self.file.path(),
            self.range,
            self.rule.code(),
            &self.message,
        )
    }
}

/// FNV-1a, used for fingerprints because it is stable across Rust versions
/// and platforms (unlike `DefaultHasher`).
#[derive(Clone, Copy)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}
