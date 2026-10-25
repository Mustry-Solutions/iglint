//! Code actions, hover and commands.

use std::collections::HashMap;
use std::path::Path;

use iglint_linter::rules::REGISTRY;
use iglint_linter::{Applicability, Diagnostic, FileEdit, RuleCode};
use iglint_text::LineIndex;
use lsp_types::request::ApplyWorkspaceEdit;
use lsp_types::{
    ApplyWorkspaceEditParams, CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    DocumentChanges, ExecuteCommandParams, Hover, HoverContents, HoverParams, MarkupContent,
    MarkupKind, OneOf, OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit, TextEdit,
    Url, WorkspaceEdit,
};
use serde_json::{Value, json};

use crate::convert::{self, path_of, uri_of};
use crate::fresh::{FindingKey, FixWanted, NO_FIX_STALE};
use crate::server::{COMMAND_FIX_ALL, COMMAND_RULE_DOCS, FIX_ALL_KIND, Server};

impl Server {
    pub fn code_actions(&mut self, params: CodeActionParams) -> Option<Vec<CodeActionOrCommand>> {
        let uri = params.text_document.uri;
        let path = path_of(&uri)?;
        // The list follows the editor's text of every open document
        // (review round 30, B2).
        self.sync_documents();
        self.flush_document(&uri);
        let only = params.context.only.as_deref();
        let wants = |kind: &str| {
            only.is_none_or(|o| {
                o.iter()
                    .any(|k| kind == k.as_str() || kind.starts_with(&format!("{}.", k.as_str())))
            })
        };
        let mut actions = Vec::new();
        let published = self.results.get(&path)?;
        if !self.is_current(&uri, &published.diagnostics) {
            return Some(actions);
        }
        // The findings in the range with a fix to offer.
        let keys: Vec<FindingKey> = published
            .diagnostics
            .iter()
            .zip(&published.lsp)
            .filter(|(d, lsp)| {
                overlaps(lsp.range, params.range)
                    && d.fix
                        .as_ref()
                        .is_some_and(|f| f.applicability != Applicability::DisplayOnly)
            })
            .filter_map(|(d, _)| FindingKey::of(d))
            .collect();
        let quick_fixes = wants(CodeActionKind::QUICKFIX.as_str()) && !keys.is_empty();
        let fix_all = wants(FIX_ALL_KIND) && self.has_fix_all(&path);
        let folder = self.folder_for(&path);
        if quick_fixes || fix_all {
            // What is shown is brought up to date first (review rounds 29
            // and 31). The edits do not depend on it: they come from a
            // fresh check ([`crate::fresh`]).
            self.refresh_disk(folder);
            if self.republish_if_stale(folder, &path) {
                return Some(actions);
            }
        }
        // While another open document has unsaved changes no fix is
        // computed ([`crate::fresh`]); the fixes are shown disabled when
        // the client can show that.
        let withheld = if quick_fixes || fix_all {
            self.withheld(&path).err()
        } else {
            None
        };
        let disabled = withheld.as_ref().map(|w| lsp_types::CodeActionDisabled {
            reason: w.reason.to_owned(),
        });
        // Without `codeAction/resolve` the edits go with the list: the
        // findings are those of a fresh check (published again when they
        // differ), with the fix of each finding in the range (review
        // round 33, M1: only those).
        let eager = if quick_fixes && !self.resolve_support && withheld.is_none() {
            match self.fresh(&path, &FixWanted::Keys(keys)) {
                Ok(fresh) => Some(fresh),
                Err(_) => return Some(actions),
            }
        } else {
            None
        };
        let folder = self.folder_for(&path);
        if wants(CodeActionKind::QUICKFIX.as_str()) {
            let Some(published) = self.results.get(&path) else {
                return Some(actions);
            };
            let in_range: Vec<(Diagnostic, lsp_types::Diagnostic)> = published
                .diagnostics
                .iter()
                .zip(&published.lsp)
                .filter(|(_, lsp)| overlaps(lsp.range, params.range))
                .map(|(d, lsp)| (d.clone(), lsp.clone()))
                .collect();
            for (d, lsp) in &in_range {
                if let Some(fix) = &d.fix
                    && fix.applicability != Applicability::DisplayOnly
                    && let Some(key) = FindingKey::of(d)
                {
                    let safe = fix.applicability == Applicability::Safe;
                    let root = self.folders[folder].root.clone();
                    let title = match (&fix.unsafe_reason, safe) {
                        (_, true) => format!("{}: {}", d.rule.code(), fix.message),
                        (Some(reason), false) => format!(
                            "{}: {} (unsafe: {})",
                            d.rule.code(),
                            fix.message,
                            reason.describe(|p| {
                                iglint_linter::discovery::slash_path(
                                    p.strip_prefix(&root).unwrap_or(p),
                                )
                            })
                        ),
                        (None, false) => format!("{}: {} (unsafe)", d.rule.code(), fix.message),
                    };
                    let mut action = CodeAction {
                        title,
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![lsp.clone()]),
                        is_preferred: Some(safe),
                        data: Some(json!({ "uri": uri, "fix": key })),
                        ..CodeAction::default()
                    };
                    if withheld.is_some() {
                        if self.disabled_support {
                            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                                is_preferred: None,
                                data: None,
                                disabled: disabled.clone(),
                                ..action
                            }));
                        }
                    } else if !self.resolve_support {
                        // Without `codeAction/resolve`, the edit of a
                        // fresh check goes with the list; a fix it does
                        // not make is not offered.
                        action.edit = eager
                            .as_ref()
                            .and_then(|fresh| self.document_edit(&uri, fresh.fix_of(&key)).ok());
                        if action.edit.is_some() {
                            actions.push(CodeActionOrCommand::CodeAction(action));
                        }
                    } else {
                        actions.push(CodeActionOrCommand::CodeAction(action));
                    }
                }
                let live = &self.folders[folder].live;
                if live.suppressible(d.rule)
                    && let Some(edit) = live.suppression_edit(d)
                    && let Some(edit) = self.workspace_edit(folder, &[edit])
                {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: format!("{}: suppress on this line", d.rule.code()),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![lsp.clone()]),
                        edit: Some(edit),
                        ..CodeAction::default()
                    }));
                }
            }
        }
        if fix_all {
            let mut action = CodeAction {
                title: "iglint: fix all auto-fixable problems".to_owned(),
                kind: Some(CodeActionKind::new(FIX_ALL_KIND)),
                data: Some(json!({ "uri": uri })),
                ..CodeAction::default()
            };
            if withheld.is_some() {
                if self.disabled_support {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        data: None,
                        disabled,
                        ..action
                    }));
                }
                return Some(actions);
            }
            if !self.resolve_support {
                action.edit = self.fix_all_edit(&uri).ok();
                if action.edit.is_none() {
                    return Some(actions);
                }
            }
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
        Some(actions)
    }

    /// `codeAction/resolve`: the edit of a quick fix or of "fix all", from
    /// a fresh check ([`crate::fresh`]); none when that check does not
    /// make the fix, and then, for clients that show disabled actions, the
    /// action disabled with the reason (review round 34, Opus nit 2).
    pub fn resolve_code_action(&mut self, mut action: CodeAction) -> CodeAction {
        if action.edit.is_some() {
            return action;
        }
        let data = action.data.clone().unwrap_or(Value::Null);
        let Some(uri) = data
            .get("uri")
            .and_then(|u| serde_json::from_value::<Url>(u.clone()).ok())
        else {
            return action;
        };
        let edit = match data.get("fix") {
            Some(key) => match (
                serde_json::from_value::<FindingKey>(key.clone()),
                path_of(&uri),
            ) {
                (Ok(key), Some(path)) => self.quick_fix_edit(&uri, &path, &key),
                _ => return action,
            },
            None => self.fix_all_edit(&uri),
        };
        match edit {
            Ok(edit) => action.edit = Some(edit),
            Err(reason) if self.disabled_support => {
                action.disabled = Some(lsp_types::CodeActionDisabled {
                    reason: reason.to_owned(),
                });
                action.is_preferred = None;
                action.data = None;
            }
            Err(_) => {}
        }
        action
    }

    /// Whether the published findings of `uri` were computed from the text
    /// the editor has now (fix ranges are byte offsets into that text), or
    /// from it with the byte order mark the editor hides
    /// ([`crate::fresh::as_saved`]).
    fn is_current(&self, uri: &Url, diagnostics: &[Diagnostic]) -> bool {
        match (self.documents.get(uri), diagnostics.first()) {
            (Some(doc), Some(d)) => {
                let text = d.file.text();
                text == doc.text || text.strip_prefix('\u{feff}') == Some(doc.text.as_str())
            }
            _ => true,
        }
    }

    fn has_fix_all(&self, path: &Path) -> bool {
        let unsafe_fixes = self.options.unsafe_fixes;
        self.results.get(path).is_some_and(|p| {
            p.diagnostics.iter().any(|d| {
                d.fix.as_ref().is_some_and(|f| match f.applicability {
                    Applicability::Safe => true,
                    Applicability::Unsafe => unsafe_fixes,
                    Applicability::DisplayOnly => false,
                })
            })
        })
    }

    /// The edit of "fix all" for a document, from a fresh `--fix`
    /// ([`crate::fresh`]): one replacement of the part that changes; or why
    /// there is none.
    pub fn fix_all_edit(&mut self, uri: &Url) -> Result<WorkspaceEdit, &'static str> {
        let path = path_of(uri).ok_or(NO_FIX_STALE)?;
        self.sync_documents();
        self.flush_document(uri);
        let wanted = FixWanted::All {
            unsafe_fixes: self.options.unsafe_fixes,
        };
        let fresh = self.fresh(&path, &wanted)?;
        self.document_edit(uri, fresh.fixed)
    }

    /// The edit of the quick fix of the finding `key`, from a fresh check
    /// ([`crate::fresh`]) (`codeAction/resolve`); or why there is none.
    fn quick_fix_edit(
        &mut self,
        uri: &Url,
        path: &Path,
        key: &FindingKey,
    ) -> Result<WorkspaceEdit, &'static str> {
        self.sync_documents();
        self.flush_document(uri);
        let fresh = self.fresh(path, &FixWanted::One(key.clone()))?;
        self.document_edit(uri, fresh.fixed)
    }

    /// The edit that turns the text a fresh run linted into its fixed
    /// text, while the document still has that text: for the version of
    /// the document it was computed for, when the client takes versioned
    /// edits (review round 34, Opus nit 3).
    fn document_edit(
        &self,
        uri: &Url,
        fixed: Option<(iglint_linter::SourceFile, String)>,
    ) -> Result<WorkspaceEdit, &'static str> {
        let (original, fixed) = fixed.ok_or(NO_FIX_STALE)?;
        let version = match self.documents.get(uri) {
            Some(doc) if doc.text != original.text() => return Err(NO_FIX_STALE),
            Some(doc) => Some(doc.version),
            None => None,
        };
        let (range, content) = convert::minimal_edit(original.text(), &fixed);
        let edit = TextEdit {
            range: convert::source_range(&original, range, self.mark_in_editor(original.path())),
            new_text: content,
        };
        Ok(self.edit_of(vec![(uri.clone(), version, vec![edit])]))
    }

    /// A workspace edit of text edits per document (with the version of
    /// the document they were computed for, `None` for a file that is not
    /// open): `documentChanges` when the client supports them, `changes`
    /// otherwise.
    fn edit_of(&self, edits: Vec<(Url, Option<i32>, Vec<TextEdit>)>) -> WorkspaceEdit {
        if self.document_changes {
            let changes = edits
                .into_iter()
                .map(|(uri, version, edits)| TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                    edits: edits.into_iter().map(OneOf::Left).collect(),
                })
                .collect();
            WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(changes)),
                ..WorkspaceEdit::default()
            }
        } else {
            let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
            for (uri, _, edits) in edits {
                changes.entry(uri).or_default().extend(edits);
            }
            WorkspaceEdit {
                changes: Some(changes),
                ..WorkspaceEdit::default()
            }
        }
    }

    /// Checks `path` again with the live workspace; when the findings
    /// differ from the published ones (rule, place, message, fix),
    /// publishes the new ones and returns true.
    fn republish_if_stale(&mut self, folder: usize, path: &Path) -> bool {
        let Some(fresh) = self.folders[folder]
            .live
            .check(std::slice::from_ref(&path.to_path_buf()))
            .pop()
        else {
            return false;
        };
        let current = self.folders[folder].filter_baseline(fresh.diagnostics.clone());
        let same = self
            .results
            .get(path)
            .is_some_and(|p| iglint_linter::live::same_findings(&p.diagnostics, &current));
        if !same {
            self.publish_result(folder, fresh);
        }
        !same
    }

    /// Fix edits as a workspace edit, against the current text of each
    /// file (for an open document, the version that has that text; none
    /// when the document has another text).
    fn workspace_edit(&self, folder: usize, edits: &[FileEdit]) -> Option<WorkspaceEdit> {
        let mut out: Vec<(Url, Option<i32>, Vec<TextEdit>)> = Vec::new();
        for e in edits {
            let source = self.folders[folder].live.source(&e.path)?;
            if e.edit.range.end().to_usize() > source.text().len() {
                return None;
            }
            let path = iglint_linter::discovery::absolute(&e.path);
            let open = self.documents.iter().find(|(_, d)| d.path == path);
            let (uri, version) = match open {
                Some((uri, doc)) => {
                    let text = source.text();
                    if text != doc.text && text.strip_prefix('\u{feff}') != Some(doc.text.as_str())
                    {
                        return None;
                    }
                    (uri.clone(), Some(doc.version))
                }
                None => (uri_of(&path)?, None),
            };
            let edit = TextEdit {
                range: convert::source_range(&source, e.edit.range, self.mark_in_editor(&path)),
                new_text: e.edit.content.clone(),
            };
            match out.iter_mut().find(|(u, _, _)| *u == uri) {
                Some((_, _, edits)) => edits.push(edit),
                None => out.push((uri, version, vec![edit])),
            }
        }
        Some(self.edit_of(out))
    }

    pub fn hover(&mut self, params: HoverParams) -> Option<Hover> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let path = path_of(&uri)?;
        self.flush_document(&uri);
        let published = self.results.get(&path)?;
        let first = published.diagnostics.first()?;
        let text = first.file.text();
        if !self.is_current(&uri, &published.diagnostics) {
            return None;
        }
        let index = LineIndex::new(text);
        let offset = convert::source_offset(&index, text, position, self.mark_in_editor(&path));
        let mut sections: Vec<String> = Vec::new();
        let mut range: Option<Range> = None;
        let mut seen: Vec<RuleCode> = Vec::new();
        for (d, lsp) in published.diagnostics.iter().zip(&published.lsp) {
            if !d.range.contains_inclusive(offset) {
                continue;
            }
            range.get_or_insert(lsp.range);
            let mut section = format!("**{}** `{}`: {}", d.rule.code(), d.rule.name(), d.message);
            if let Some(fix) = &d.fix {
                section.push_str(&format!("\n\nFix: {}", fix.message));
            }
            if !seen.contains(&d.rule) {
                seen.push(d.rule);
                section.push_str("\n\n");
                section.push_str(&rule_docs(
                    d.rule,
                    &self.options.rule_docs_url(d.rule.code(), d.rule.name()),
                ));
            }
            sections.push(section);
        }
        if sections.is_empty() {
            return None;
        }
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: sections.join("\n\n---\n\n"),
            }),
            range,
        })
    }

    pub fn execute_command(&mut self, params: ExecuteCommandParams) -> Value {
        let argument = params.arguments.first().cloned().unwrap_or(Value::Null);
        match params.command.as_str() {
            COMMAND_FIX_ALL => {
                let uri = argument
                    .get("uri")
                    .cloned()
                    .unwrap_or(argument)
                    .as_str()
                    .and_then(|s| Url::parse(s).ok());
                let Some(uri) = uri else {
                    return Value::Null;
                };
                match self.fix_all_edit(&uri) {
                    Ok(edit) => {
                        self.request::<ApplyWorkspaceEdit>(ApplyWorkspaceEditParams {
                            label: Some("iglint: fix all".to_owned()),
                            edit,
                        });
                        Value::Bool(true)
                    }
                    Err(_) => Value::Bool(false),
                }
            }
            COMMAND_RULE_DOCS => {
                let key = argument.as_str().unwrap_or("");
                match REGISTRY.find(key) {
                    Some(rule) => Value::String(format!(
                        "# {} {}\n\n{}\n\n{}",
                        rule.code(),
                        rule.name(),
                        rule.meta().summary,
                        rule.meta().documented_explanation()
                    )),
                    None => Value::Null,
                }
            }
            _ => Value::Null,
        }
    }
}

/// The documentation of a rule for a hover.
fn rule_docs(rule: RuleCode, url: &str) -> String {
    let meta = rule.meta();
    format!(
        "{}\n\n{}\n\n[Rule documentation]({url})",
        meta.summary,
        meta.documented_explanation().trim()
    )
}

fn overlaps(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}
