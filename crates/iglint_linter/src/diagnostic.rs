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
    /// Invalid UTF-8 sequences of the file on disk; `text` has
    /// [`iglint_text::INVALID_UTF8_SUBSTITUTE`] there.
    invalid_utf8: Box<[TextRange]>,
    line_index: std::sync::OnceLock<LineIndex>,
    protocol_line_index: std::sync::OnceLock<LineIndex>,
}

impl SourceFile {
    pub fn new(path: impl Into<PathBuf>, text: impl Into<Arc<str>>) -> Self {
        Self::decoded(path, text, Vec::new())
    }

    /// A file read with [`iglint_text::decode_utf8`]: `invalid_utf8` are the
    /// ranges of its invalid UTF-8 sequences.
    pub fn decoded(
        path: impl Into<PathBuf>,
        text: impl Into<Arc<str>>,
        invalid_utf8: Vec<TextRange>,
    ) -> Self {
        SourceFile {
            inner: Arc::new(SourceFileInner {
                path: path.into(),
                text: text.into(),
                invalid_utf8: invalid_utf8.into_boxed_slice(),
                line_index: std::sync::OnceLock::new(),
                protocol_line_index: std::sync::OnceLock::new(),
            }),
        }
    }

    /// Ranges of the invalid UTF-8 sequences of the file on disk. Findings
    /// that touch them are dropped (`VIEW007` reports them) and the file
    /// gets no fixes: writing the text back would change those bytes.
    pub fn invalid_utf8(&self) -> &[TextRange] {
        &self.inner.invalid_utf8
    }

    /// Whether `range` touches an invalid UTF-8 sequence.
    pub fn touches_invalid_utf8(&self, range: TextRange) -> bool {
        self.inner
            .invalid_utf8
            .iter()
            .any(|bad| range.start() < bad.end() && bad.start() <= range.end())
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

    /// Lines and columns as Jython numbers them ([`LineIndex::python`]),
    /// for everything iglint shows.
    pub fn line_index(&self) -> &LineIndex {
        self.inner
            .line_index
            .get_or_init(|| LineIndex::python(&self.inner.text))
    }

    /// Lines as the Language Server Protocol counts them ([`LineIndex::new`]:
    /// a lone `\r` ends a line too), for positions sent to an editor.
    pub fn protocol_line_index(&self) -> &LineIndex {
        if !self.inner.text.contains('\r') {
            return self.line_index();
        }
        self.inner
            .protocol_line_index
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
    /// Set when the fix deletes a statement of a suite (a block of
    /// statements): fixes with the same guard never delete all statements
    /// of the suite in one pass.
    pub suite: Option<SuiteGuard>,
    /// Why the fix is unsafe. Every unsafe fix has one: the code around
    /// it that keeps it from being safe (code that is not vetted, or that
    /// reads a frame), or what the fix itself may change.
    pub unsafe_reason: Option<UnsafeReason>,
    /// The fix changes only layout: whitespace or the text of comments.
    /// Such a fix is safe only when every script of the file lexes to the
    /// same tokens after it (`safety::same_compilation`), which the safety
    /// net checks: whitespace the lexer reads as part of a token (after a
    /// backslash in a string Jython drops, after a stray backslash) is not
    /// layout.
    pub layout: bool,
}

/// Why a fix that would otherwise be safe is unsafe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsafeReason {
    /// What keeps the fix from being safe, without the location.
    pub message: String,
    /// The construct and where it is.
    pub site: Option<ReasonSite>,
    /// The site is the first use of code that is not vetted, or that reads
    /// a frame, in the whole workspace (the closed-world rule). It may be
    /// in another file and is filled in after the cache, from the current
    /// project index.
    pub workspace: bool,
}

/// A location named by an [`UnsafeReason`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasonSite {
    pub path: PathBuf,
    /// 1-based line and column (characters).
    pub line: u32,
    pub column: u32,
    /// The construct, as written (one line, shortened).
    pub what: String,
}

impl UnsafeReason {
    /// A reason without a site.
    pub fn new(message: impl Into<String>) -> UnsafeReason {
        UnsafeReason {
            message: message.into(),
            site: None,
            workspace: false,
        }
    }

    /// The reason as one sentence, with the location shown by `display`.
    pub fn describe(&self, display: impl Fn(&Path) -> String) -> String {
        match &self.site {
            Some(site) => format!(
                "{}: `{}` at {}:{}:{}",
                self.message,
                site.what,
                display(&site.path),
                site.line,
                site.column
            ),
            None => self.message.clone(),
        }
    }
}

/// The suite a statement-deleting fix removes a statement from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SuiteGuard {
    /// Identifies the suite within the file (the file offset of its first
    /// statement).
    pub key: u32,
    /// The number of statements in the suite.
    pub statements: u32,
}

impl Fix {
    /// Records why the fix is unsafe (see [`UnsafeReason`]); `None` keeps
    /// the reason it has.
    pub fn because(mut self, reason: Option<UnsafeReason>) -> Self {
        if reason.is_some() {
            self.unsafe_reason = reason;
        }
        self
    }

    /// Records `message` as why the fix is unsafe, when it is unsafe and
    /// has no reason yet. Every unsafe fix says why.
    pub fn reason_if_unsafe(mut self, message: impl Into<String>) -> Self {
        if self.applicability == Applicability::Unsafe && self.unsafe_reason.is_none() {
            self.unsafe_reason = Some(UnsafeReason::new(message));
        }
        self
    }

    /// Marks the fix as changing only whitespace or comments (see
    /// [`Fix::layout`]).
    pub fn layout_only(mut self) -> Self {
        self.layout = true;
        self
    }

    /// Marks the fix as deleting a statement of the suite `guard`.
    pub fn deleting_from(mut self, guard: SuiteGuard) -> Self {
        self.suite = Some(guard);
        self
    }

    pub fn safe(message: impl Into<String>, edits: Vec<FileEdit>) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::Safe,
            edits,
            suite: None,
            unsafe_reason: None,
            layout: false,
        }
    }

    /// An unsafe fix, with why it is unsafe.
    pub fn unsafe_edit(
        message: impl Into<String>,
        edits: Vec<FileEdit>,
        reason: impl Into<String>,
    ) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::Unsafe,
            edits,
            suite: None,
            unsafe_reason: Some(UnsafeReason::new(reason)),
            layout: false,
        }
    }

    pub fn display_only(message: impl Into<String>, edits: Vec<FileEdit>) -> Self {
        Fix {
            message: message.into(),
            applicability: Applicability::DisplayOnly,
            edits,
            suite: None,
            unsafe_reason: None,
            layout: false,
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
    /// The rule chose this finding's severity from its options (for
    /// example `SCR900`'s `category-mapping`). Where the finding lands in
    /// another file than the one whose settings the rule ran with, the
    /// severity is kept when both files have the same settings for the
    /// rule, and replaced by that file's rule severity otherwise.
    pub severity_from_options: bool,
}

impl Diagnostic {
    pub fn new(
        rule: RuleCode,
        severity: Severity,
        message: impl Into<String>,
        file: SourceFile,
        range: TextRange,
    ) -> Self {
        let range = char_range(file.text(), range);
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
            severity_from_options: false,
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

/// `range` widened to whole characters of `text` ([`TextRange::snap_to_chars`]).
/// Every range of a finding goes through here, so a rule that computes a
/// range in bytes (one byte at the start of a file that begins with a byte
/// order mark) cannot make the output or the language server panic.
/// Debug builds assert that the rule's range was already on character
/// boundaries, so tests find such rules.
pub(crate) fn char_range(text: &str, range: TextRange) -> TextRange {
    debug_assert!(
        range.end().to_usize() > text.len() || range.is_on_char_boundaries(text),
        "range {range:?} splits a character of {:?}",
        text.get(
            iglint_text::floor_char_boundary(text, range.start().to_usize().saturating_sub(4))
                ..iglint_text::ceil_char_boundary(text, range.end().to_usize() + 4)
        )
    );
    range.snap_to_chars(text)
}
