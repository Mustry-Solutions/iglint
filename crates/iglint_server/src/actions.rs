//! Code actions, hover and commands.

use std::collections::HashMap;
use std::path::Path;

use iglint_linter::rules::REGISTRY;
use iglint_linter::{Applicability, Diagnostic, FileEdit, RuleCode};
use iglint_text::LineIndex;
use lsp_types::request::ApplyWorkspaceEdit;
use lsp_types::{
    ApplyWorkspaceEditParams, CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    ExecuteCommandParams, Hover, HoverContents, HoverParams, MarkupContent, MarkupKind, Range,
    TextEdit, Url, WorkspaceEdit,
};
use serde_json::{Value, json};

use crate::convert::{self, path_of, uri_of};
use crate::server::{COMMAND_FIX_ALL, COMMAND_RULE_DOCS, FIX_ALL_KIND, Server};

impl Server {
    pub fn code_actions(&mut self, params: CodeActionParams) -> Option<Vec<CodeActionOrCommand>> {
        let uri = params.text_document.uri;
        let path = path_of(&uri)?;
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
        let folder = self.folder_for(&path);
        if wants(CodeActionKind::QUICKFIX.as_str()) {
            let published = self.results.get(&path)?;
            let in_range: Vec<(Diagnostic, lsp_types::Diagnostic)> = published
                .diagnostics
                .iter()
                .zip(&published.lsp)
                .filter(|(_, lsp)| overlaps(lsp.range, params.range))
                .map(|(d, lsp)| (d.clone(), lsp.clone()))
                .collect();
            for (d, lsp) in &in_range {
                // The fix goes through the same function as fix-all and
                // `--fix` (`Live::fix_one`): one that would make the file
                // worse, or a safe fix that fails the safe-fix net, is not
                // offered.
                if let Some(fix) = &d.fix
                    && fix.applicability != Applicability::DisplayOnly
                    && let Some(edit) = self.quick_fix_edit(&uri, &path, folder, d)
                {
                    let safe = fix.applicability == Applicability::Safe;
                    let title = if safe {
                        format!("{}: {}", d.rule.code(), fix.message)
                    } else {
                        format!("{}: {} (unsafe)", d.rule.code(), fix.message)
                    };
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title,
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![lsp.clone()]),
                        edit: Some(edit),
                        is_preferred: Some(safe),
                        ..CodeAction::default()
                    }));
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
        if wants(FIX_ALL_KIND) && self.has_fix_all(&path) {
            let mut action = CodeAction {
                title: "iglint: fix all auto-fixable problems".to_owned(),
                kind: Some(CodeActionKind::new(FIX_ALL_KIND)),
                data: Some(json!({ "uri": uri })),
                ..CodeAction::default()
            };
            if !self.resolve_support {
                action.edit = self.fix_all_edit(&uri);
                if action.edit.is_none() {
                    return Some(actions);
                }
            }
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
        Some(actions)
    }

    pub fn resolve_code_action(&mut self, mut action: CodeAction) -> CodeAction {
        if action.edit.is_none()
            && let Some(uri) = action
                .data
                .as_ref()
                .and_then(|d| d.get("uri"))
                .and_then(|u| serde_json::from_value::<Url>(u.clone()).ok())
        {
            action.edit = self.fix_all_edit(&uri);
        }
        action
    }

    /// Whether the published findings of `uri` were computed from the text
    /// the editor has now (fix ranges are byte offsets into that text).
    fn is_current(&self, uri: &Url, diagnostics: &[Diagnostic]) -> bool {
        match (self.documents.get(uri), diagnostics.first()) {
            (Some(doc), Some(d)) => d.file.text() == doc.text,
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

    /// The edit of "fix all" for a document: one replacement of the part
    /// that changes.
    pub fn fix_all_edit(&mut self, uri: &Url) -> Option<WorkspaceEdit> {
        let path = path_of(uri)?;
        self.flush_document(uri);
        let folder = self.folder_for(&path);
        let unsafe_fixes = self.options.unsafe_fixes;
        let current = self.folders[folder].live.source(&path)?;
        let fixed = self.folders[folder].live.fix_all(&path, unsafe_fixes)?;
        let (range, content) = convert::minimal_edit(current.text(), &fixed.fixed);
        let edit = TextEdit {
            range: convert::source_range(&current, range),
            new_text: content,
        };
        Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), vec![edit])])),
            ..WorkspaceEdit::default()
        })
    }

    /// The edit of the quick fix of `d`, applied through
    /// `Live::fix_one` (see [`Server::code_actions`]).
    fn quick_fix_edit(
        &mut self,
        uri: &Url,
        path: &Path,
        folder: usize,
        d: &Diagnostic,
    ) -> Option<WorkspaceEdit> {
        let current = self.folders[folder].live.source(path)?;
        let fixed = self.folders[folder].live.fix_one(path, d)?;
        if fixed.applied == 0 {
            return None;
        }
        let (range, content) = convert::minimal_edit(current.text(), &fixed.fixed);
        Some(WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri.clone(),
                vec![TextEdit {
                    range: convert::source_range(&current, range),
                    new_text: content,
                }],
            )])),
            ..WorkspaceEdit::default()
        })
    }

    /// Fix edits as a workspace edit, against the current text of each file.
    fn workspace_edit(&self, folder: usize, edits: &[FileEdit]) -> Option<WorkspaceEdit> {
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
        for e in edits {
            let source = self.folders[folder].live.source(&e.path)?;
            if e.edit.range.end().to_usize() > source.text().len() {
                return None;
            }
            changes
                .entry(uri_of(&iglint_linter::discovery::absolute(&e.path))?)
                .or_default()
                .push(TextEdit {
                    range: convert::source_range(&source, e.edit.range),
                    new_text: e.edit.content.clone(),
                });
        }
        Some(WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        })
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
        let offset = convert::offset(&index, text, position);
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
                    Some(edit) => {
                        self.request::<ApplyWorkspaceEdit>(ApplyWorkspaceEditParams {
                            label: Some("iglint: fix all".to_owned()),
                            edit,
                        });
                        Value::Bool(true)
                    }
                    None => Value::Bool(false),
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
                        rule.meta().explanation
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
        meta.explanation.trim()
    )
}

fn overlaps(a: Range, b: Range) -> bool {
    a.start <= b.end && b.start <= a.end
}
