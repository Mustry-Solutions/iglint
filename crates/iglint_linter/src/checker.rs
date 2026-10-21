//! The context rules run in and the API they report through.

use iglint_json::{JsonEditor, Value};
use iglint_project::model::{FileError, ModelData};
use iglint_project::view::View;
use iglint_project::workspace::Resource;
use iglint_project::{
    EmbeddedExpression, EmbeddedScript, FileId, ProjectId, ProjectIndex, Workspace,
};
use iglint_text::{TextEdit, TextRange};

use crate::diagnostic::{Diagnostic, FileEdit, Fix, Label, Severity, SourceFile};
use crate::registry::RuleCode;
use crate::rule::ActiveRule;

/// Source files a run may report on, by workspace file id. Files outside
/// the workspace (standalone `view.json` files, stdin) get ids after the
/// workspace's own.
#[derive(Default)]
pub struct FileTable {
    files: Vec<Option<SourceFile>>,
}

impl FileTable {
    pub fn with_len(len: usize) -> Self {
        FileTable {
            files: vec![None; len],
        }
    }

    pub fn insert(&mut self, id: FileId, file: SourceFile) {
        let i = id.index();
        if i >= self.files.len() {
            self.files.resize(i + 1, None);
        }
        self.files[i] = Some(file);
    }

    pub fn get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.index())?.as_ref()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// One unit of analysis: a resource (or a standalone file, or a project)
/// with its typed model.
pub struct Unit<'a> {
    pub ws: &'a Workspace,
    /// `None` for standalone files and project units.
    pub resource: Option<&'a Resource>,
    /// The project that owns the resource; `None` for gateway collections
    /// and standalone files.
    pub project: Option<ProjectId>,
    pub data: &'a ModelData<'a>,
    /// Unreadable files and JSON syntax errors of the unit.
    pub errors: &'a [FileError],
    /// The file diagnostics go to unless a rule names another one.
    pub primary: FileId,
    pub files: &'a FileTable,
    /// Report dynamic references that cannot be resolved statically.
    pub strict: bool,
}

/// What a rule sees while it runs, and how it reports.
pub struct Checker<'c, 'a> {
    unit: &'c Unit<'a>,
    index: Option<&'c ProjectIndex<'a>>,
    active: &'c ActiveRule,
    sink: &'c mut Vec<Diagnostic>,
}

impl<'c, 'a> Checker<'c, 'a> {
    pub fn new(
        unit: &'c Unit<'a>,
        index: Option<&'c ProjectIndex<'a>>,
        active: &'c ActiveRule,
        sink: &'c mut Vec<Diagnostic>,
    ) -> Self {
        Checker {
            unit,
            index,
            active,
            sink,
        }
    }

    // ---- the running rule ----

    pub fn rule(&self) -> RuleCode {
        self.active.rule
    }

    /// The configured severity of the running rule.
    pub fn severity(&self) -> Severity {
        self.active.severity
    }

    /// The parsed options of the running rule.
    ///
    /// # Panics
    ///
    /// When `T` is not the type the rule's options parser produces; that is
    /// a bug in the rule, caught by its tests.
    pub fn options<T: 'static>(&self) -> &T {
        self.active
            .options
            .as_ref()
            .and_then(|o| o.downcast_ref::<T>())
            .unwrap_or_else(|| {
                panic!(
                    "{}: options are not of type {}",
                    self.active.rule,
                    std::any::type_name::<T>()
                )
            })
    }

    // ---- what is being linted ----

    pub fn unit(&self) -> &Unit<'a> {
        self.unit
    }

    pub fn workspace(&self) -> &'a Workspace {
        self.unit.ws
    }

    /// The project index; `Some` for rules in [`crate::rule::Phase::Project`].
    pub fn index(&self) -> Option<&'c ProjectIndex<'a>> {
        self.index
    }

    /// The project index and the unit's project, when both exist: the
    /// precondition of every cross-resource lookup.
    pub fn project_index(&self) -> Option<(&'c ProjectIndex<'a>, ProjectId)> {
        Some((self.index?, self.unit.project?))
    }

    pub fn project(&self) -> Option<ProjectId> {
        self.unit.project
    }

    pub fn resource(&self) -> Option<&'a Resource> {
        self.unit.resource
    }

    pub fn data(&self) -> &'a ModelData<'a> {
        self.unit.data
    }

    /// The view, when the unit is a view.
    pub fn view(&self) -> Option<&'a View<'a>> {
        self.unit.data.view()
    }

    pub fn strict(&self) -> bool {
        self.unit.strict
    }

    pub fn primary_file(&self) -> FileId {
        self.unit.primary
    }

    /// A source file of the run, if it may be reported on.
    pub fn file(&self, id: FileId) -> Option<&'a SourceFile> {
        self.unit.files.get(id)
    }

    // ---- reporting ----

    /// Reports a finding in the unit's primary file.
    pub fn report(&mut self, range: TextRange, message: impl Into<String>) -> Report<'_> {
        let file = self.unit.primary;
        self.report_in(file, range, message)
    }

    /// Reports a finding in `file`. Findings in files that are not part of
    /// the run (context-only resources) are dropped.
    pub fn report_in(
        &mut self,
        file: FileId,
        range: TextRange,
        message: impl Into<String>,
    ) -> Report<'_> {
        let diagnostic = self.unit.files.get(file).and_then(|source| {
            (range.end().to_usize() <= source.text().len()).then(|| {
                Diagnostic::new(
                    self.active.rule,
                    self.active.severity,
                    message,
                    source.clone(),
                    range,
                )
            })
        });
        Report {
            sink: self.sink,
            files: self.unit.files,
            diagnostic,
        }
    }

    /// Reports a range of an embedded expression's text, mapped to the file.
    pub fn report_expression(
        &mut self,
        file: FileId,
        expression: &EmbeddedExpression<'_>,
        range: TextRange,
        message: impl Into<String>,
    ) -> Report<'_> {
        let range = expression.file_range(range);
        self.report_in(file, range, message)
    }

    /// Reports a range of a script's `source`, mapped to the file.
    pub fn report_script(
        &mut self,
        file: FileId,
        script: &EmbeddedScript<'_>,
        range: TextRange,
        message: impl Into<String>,
    ) -> Report<'_> {
        let range = script.file_range(range);
        self.report_in(file, range, message)
    }

    // ---- fixes ----

    /// A formatting-preserving JSON editor for `file`, when it is a parsed
    /// JSON file of the run and `root` is its parse tree.
    pub fn json_editor(&self, file: FileId, root: &Value<'_>) -> Option<JsonEditor<'a>> {
        let source = self.unit.files.get(file)?;
        Some(JsonEditor::new(source.text(), root))
    }

    /// Turns text edits of `file` into fix edits.
    pub fn file_edits(&self, file: FileId, edits: Vec<TextEdit>) -> Vec<FileEdit> {
        let Some(source) = self.unit.files.get(file) else {
            return Vec::new();
        };
        edits
            .into_iter()
            .map(|edit| FileEdit {
                path: source.path().to_path_buf(),
                edit,
            })
            .collect()
    }
}

/// A finding being built. It is recorded when dropped.
pub struct Report<'r> {
    sink: &'r mut Vec<Diagnostic>,
    files: &'r FileTable,
    diagnostic: Option<Diagnostic>,
}

impl Report<'_> {
    /// iglint node path (see `iglint_project::node_path`).
    pub fn node_path(mut self, path: impl Into<String>) -> Self {
        if let Some(d) = &mut self.diagnostic {
            d.node_path = Some(path.into());
        }
        self
    }

    /// ign-lint node path, shown by `iglint ign-lint`.
    pub fn ign_lint_path(mut self, path: Option<impl Into<String>>) -> Self {
        if let (Some(d), Some(path)) = (&mut self.diagnostic, path) {
            d.ign_lint_path = Some(path.into());
        }
        self
    }

    /// ign-lint's message text, when it differs from iglint's (parity mode
    /// prints `{ign_lint_path}: {ign_lint_message}`).
    pub fn ign_lint_message(mut self, message: impl Into<String>) -> Self {
        if let Some(d) = &mut self.diagnostic {
            d.ign_lint_message = Some(message.into());
        }
        self
    }

    pub fn fix(mut self, fix: Fix) -> Self {
        if let Some(d) = &mut self.diagnostic {
            d.fix = Some(fix);
        }
        self
    }

    pub fn fix_opt(self, fix: Option<Fix>) -> Self {
        match fix {
            Some(fix) => self.fix(fix),
            None => self,
        }
    }

    /// A secondary location in `file`.
    pub fn label(mut self, file: FileId, range: TextRange, message: impl Into<String>) -> Self {
        if let (Some(d), Some(source)) = (&mut self.diagnostic, self.files.get(file)) {
            d.labels.push(Label {
                file: source.clone(),
                range,
                message: message.into(),
            });
        }
        self
    }

    /// Stable identity for the baseline when the message is not stable.
    pub fn fingerprint_key(mut self, key: impl Into<String>) -> Self {
        if let Some(d) = &mut self.diagnostic {
            d.fingerprint_key = Some(key.into());
        }
        self
    }

    /// Overrides the configured severity for this finding (rules whose
    /// options set severities per element).
    pub fn severity(mut self, severity: Severity) -> Self {
        if let Some(d) = &mut self.diagnostic {
            d.severity = severity;
        }
        self
    }
}

impl Drop for Report<'_> {
    fn drop(&mut self) {
        if let Some(d) = self.diagnostic.take() {
            self.sink.push(d);
        }
    }
}
