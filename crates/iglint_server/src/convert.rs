//! Conversions between iglint and LSP: positions (UTF-16), paths and
//! diagnostics.

use std::path::{Path, PathBuf};

use iglint_linter::{Diagnostic, Severity, SourceFile};
use iglint_text::{LineIndex, TextRange, TextSize, Utf16Position};
use lsp_types::{
    CodeDescription, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    NumberOrString, Position, Range, Url,
};

use crate::options::ServerOptions;

pub const SOURCE: &str = "iglint";

/// The file path of a `file:` URI.
pub fn path_of(uri: &Url) -> Option<PathBuf> {
    if uri.scheme() != "file" {
        return None;
    }
    uri.to_file_path()
        .ok()
        .map(|p| iglint_linter::discovery::absolute(&p))
}

pub fn uri_of(path: &Path) -> Option<Url> {
    Url::from_file_path(path).ok()
}

/// The largest char boundary at or before `offset`, within the text.
fn floor(text: &str, offset: TextSize) -> TextSize {
    let mut at = offset.to_usize().min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    TextSize::from(at)
}

pub fn position(index: &LineIndex, text: &str, offset: TextSize) -> Position {
    let Utf16Position { line, character } = index.utf16_position(floor(text, offset), text);
    Position { line, character }
}

pub fn range(index: &LineIndex, text: &str, range: TextRange) -> Range {
    Range {
        start: position(index, text, range.start()),
        end: position(index, text, range.end()),
    }
}

pub fn offset(index: &LineIndex, text: &str, position: Position) -> TextSize {
    index.offset_of_utf16(
        Utf16Position {
            line: position.line,
            character: position.character,
        },
        text,
    )
}

/// `r` in the editor's lines: the Language Server Protocol ends a line at
/// a lone `\r` too (where iglint's own output follows Jython's numbering).
pub fn source_range(file: &SourceFile, r: TextRange) -> Range {
    range(file.protocol_line_index(), file.text(), r)
}

pub fn severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
    }
}

/// Tags the editor renders specially: faded for unused things, struck
/// through for deprecated ones.
pub fn tags(d: &Diagnostic) -> Option<Vec<DiagnosticTag>> {
    let name = d.rule.name();
    let mut tags = Vec::new();
    if name.starts_with("unused-") {
        tags.push(DiagnosticTag::UNNECESSARY);
    }
    if name.contains("deprecated") {
        tags.push(DiagnosticTag::DEPRECATED);
    }
    (!tags.is_empty()).then_some(tags)
}

pub fn diagnostic(d: &Diagnostic, options: &ServerOptions) -> lsp_types::Diagnostic {
    let related: Vec<DiagnosticRelatedInformation> = d
        .labels
        .iter()
        .filter_map(|label| {
            Some(DiagnosticRelatedInformation {
                location: Location {
                    uri: uri_of(label.file.path())?,
                    range: source_range(&label.file, label.range),
                },
                message: label.message.clone(),
            })
        })
        .chain(
            d.fix
                .as_ref()
                .and_then(|f| f.unsafe_reason.as_ref())
                .and_then(|reason| {
                    // Why the fix is unsafe, at the code that makes it so.
                    let site = reason.site.as_ref()?;
                    let at =
                        Position::new(site.line.saturating_sub(1), site.column.saturating_sub(1));
                    Some(DiagnosticRelatedInformation {
                        location: Location {
                            uri: uri_of(&site.path)?,
                            range: lsp_types::Range::new(at, at),
                        },
                        message: format!(
                            "the fix is unsafe because {}: `{}`",
                            reason.message, site.what
                        ),
                    })
                }),
        )
        .collect();
    lsp_types::Diagnostic {
        range: source_range(&d.file, d.range),
        severity: Some(severity(d.severity)),
        code: Some(NumberOrString::String(d.rule.code().to_owned())),
        code_description: Url::parse(&options.rule_docs_url(d.rule.code(), d.rule.name()))
            .ok()
            .map(|href| CodeDescription { href }),
        source: Some(SOURCE.to_owned()),
        message: d.message.clone(),
        related_information: (!related.is_empty()).then_some(related),
        tags: tags(d),
        data: None,
    }
}

/// Applies an LSP content change to `text`.
pub fn apply_change(text: &mut String, change: &lsp_types::TextDocumentContentChangeEvent) {
    match change.range {
        None => text.clone_from(&change.text),
        Some(r) => {
            let index = LineIndex::new(text);
            let start = offset(&index, text, r.start).to_usize();
            let end = offset(&index, text, r.end).to_usize().max(start);
            text.replace_range(start..end, &change.text);
        }
    }
}

/// One edit that turns `old` into `new`: the differing middle part.
pub fn minimal_edit(old: &str, new: &str) -> (TextRange, String) {
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let prefix = {
        let mut p = prefix.min(new.len());
        while !new.is_char_boundary(p) || !old.is_char_boundary(p) {
            p -= 1;
        }
        p
    };
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    // `prefix` was moved back to a boundary of both texts above.
    #[allow(clippy::string_slice)]
    let mut suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>()
        .min(max_suffix);
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    // Both ends were moved to boundaries of `new` above.
    #[allow(clippy::string_slice)]
    let replacement = new[prefix..new.len() - suffix].to_owned();
    (TextRange::from(prefix..old.len() - suffix), replacement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::TextDocumentContentChangeEvent;

    #[test]
    fn utf16_positions() {
        let text = "a\n\u{1F600}b\r\nc";
        let index = LineIndex::new(text);
        // After the emoji (4 bytes, 2 UTF-16 units).
        assert_eq!(
            position(&index, text, TextSize::new(6)),
            Position {
                line: 1,
                character: 2
            }
        );
        // Inside the emoji: floored to its start.
        assert_eq!(
            position(&index, text, TextSize::new(4)),
            Position {
                line: 1,
                character: 0
            }
        );
        // Past the end: clamped.
        assert_eq!(
            position(&index, text, TextSize::new(99)),
            Position {
                line: 2,
                character: 1
            }
        );
        assert_eq!(
            offset(
                &index,
                text,
                Position {
                    line: 1,
                    character: 2
                }
            ),
            TextSize::new(6)
        );
    }

    #[test]
    fn incremental_changes() {
        let mut text = "h\u{e9}llo\nworld".to_owned();
        apply_change(
            &mut text,
            &TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(0, 1), Position::new(1, 1))),
                range_length: None,
                text: "E".to_owned(),
            },
        );
        assert_eq!(text, "hEorld");
        apply_change(
            &mut text,
            &TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "new".to_owned(),
            },
        );
        assert_eq!(text, "new");
    }

    #[test]
    fn minimal_edits() {
        assert_eq!(
            minimal_edit("abcdef", "abXdef"),
            (TextRange::from(2..3), "X".to_owned())
        );
        assert_eq!(
            minimal_edit("aaa", "aa"),
            (TextRange::from(2..3), String::new())
        );
        assert_eq!(
            minimal_edit("same", "same"),
            (TextRange::from(4..4), String::new())
        );
        assert_eq!(
            minimal_edit("\u{e9}\u{e8}", "\u{e9}\u{ea}"),
            (TextRange::from(2..4), "\u{ea}".to_owned())
        );
    }
}
