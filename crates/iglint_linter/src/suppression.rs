//! Suppression comments in scripts.
//!
//! ```python
//! x = undefined_thing  # iglint: ignore[SCR101]
//! y = 1  # iglint: ignore[unused-variable, SCR1]
//! # iglint: ignore-file[SCR102]
//! ```
//!
//! `ignore` applies to findings that start on the comment's line,
//! `ignore-file` to every finding of the script. Selectors are rule codes,
//! code prefixes (`SCR1`) or rule names; an empty or missing list means
//! every rule. Comments are found with a small Python lexer, so `#` inside
//! string literals is never a comment.
//!
//! The driver applies a script's suppressions to the findings its script
//! rules report ([`Suppressions::apply`]) and keeps track of which
//! suppressions matched, for `SCR123 unused-suppression`.

use iglint_project::EmbeddedScript;
use iglint_text::{TextRange, TextSize};

use crate::diagnostic::Diagnostic;

/// One suppression comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suppression {
    /// Range of the comment in the script's `source`.
    pub comment: TextRange,
    /// File range of the line the comment is on (whole script for
    /// `ignore-file`).
    pub scope: Option<TextRange>,
    /// Selectors; empty means all rules.
    pub selectors: Vec<String>,
    pub file_wide: bool,
    /// Whether it suppressed at least one finding.
    pub used: bool,
}

/// The suppressions of one script.
#[derive(Clone, Debug, Default)]
pub struct Suppressions {
    pub items: Vec<Suppression>,
}

impl Suppressions {
    /// Finds the suppression comments of `script`.
    pub fn of(script: &EmbeddedScript<'_>) -> Suppressions {
        let source: &str = &script.source;
        let mut items = Vec::new();
        for comment in comments(source) {
            let text = comment.slice(source);
            let Some(parsed) = parse_comment(text) else {
                continue;
            };
            let line = line_range(source, comment.start());
            items.push(Suppression {
                comment,
                scope: if parsed.1 {
                    None
                } else {
                    Some(script.file_range(line))
                },
                selectors: parsed.0,
                file_wide: parsed.1,
                used: false,
            });
        }
        Suppressions { items }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Removes suppressed findings from `diagnostics` and marks the
    /// suppressions that matched.
    pub fn apply(&mut self, diagnostics: &mut Vec<Diagnostic>) {
        if self.items.is_empty() {
            return;
        }
        diagnostics.retain(|d| {
            let mut suppressed = false;
            for s in &mut self.items {
                let in_scope = match s.scope {
                    None => true,
                    Some(scope) => scope.contains_inclusive(d.range.start()),
                };
                if in_scope && selects(&s.selectors, d) {
                    s.used = true;
                    suppressed = true;
                }
            }
            !suppressed
        });
    }
}

fn selects(selectors: &[String], d: &Diagnostic) -> bool {
    selectors.is_empty()
        || selectors.iter().any(|s| {
            d.rule.name() == s || d.rule.code().starts_with(s.to_ascii_uppercase().as_str())
        })
}

/// Parses `# iglint: ignore[...]` / `# iglint: ignore-file[...]`. Returns
/// the selectors and whether it is file-wide.
fn parse_comment(comment: &str) -> Option<(Vec<String>, bool)> {
    let body = comment.strip_prefix('#')?.trim_start();
    let rest = body.strip_prefix("iglint:")?.trim_start();
    let (file_wide, rest) = if let Some(r) = rest.strip_prefix("ignore-file") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix("ignore") {
        (false, r)
    } else {
        return None;
    };
    let rest = rest.trim_start();
    if let Some(list) = rest.strip_prefix('[') {
        let end = list.find(']')?;
        let selectors = list[..end]
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        Some((selectors, file_wide))
    } else if rest.is_empty() || rest.starts_with('#') || rest.starts_with(' ') {
        Some((Vec::new(), file_wide))
    } else {
        None
    }
}

/// The line (without its line break) containing `offset`.
fn line_range(text: &str, offset: TextSize) -> TextRange {
    let at = offset.to_usize();
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = text[at..].find('\n').map_or(text.len(), |i| at + i);
    TextRange::from(start..end)
}

/// Ranges of `#` comments in Python source, skipping string literals
/// (single, double and triple quoted, any prefix).
pub fn comments(source: &str) -> Vec<TextRange> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                let end = source[i..].find('\n').map_or(b.len(), |e| i + e);
                let end = if end > i && b[end - 1] == b'\r' {
                    end - 1
                } else {
                    end
                };
                out.push(TextRange::from(i..end));
                i = end;
            }
            q @ (b'\'' | b'"') => {
                let triple = b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q);
                i += if triple { 3 } else { 1 };
                while i < b.len() {
                    match b[i] {
                        b'\\' => i += 2,
                        c if c == q => {
                            if !triple {
                                i += 1;
                                break;
                            }
                            if b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q) {
                                i += 3;
                                break;
                            }
                            i += 1;
                        }
                        b'\n' if !triple => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_comments_outside_strings() {
        let src = "a = '#no' # yes\nb = \"\"\"\n# no\n\"\"\"  # also\nc = 'x\\'#' #last";
        let found: Vec<&str> = comments(src).iter().map(|r| r.slice(src)).collect();
        assert_eq!(found, vec!["# yes", "# also", "#last"]);
    }

    #[test]
    fn parses_markers() {
        assert_eq!(
            parse_comment("# iglint: ignore[SCR101, unused-variable]"),
            Some((vec!["SCR101".into(), "unused-variable".into()], false))
        );
        assert_eq!(parse_comment("# iglint: ignore"), Some((vec![], false)));
        assert_eq!(
            parse_comment("#iglint: ignore-file[SCR1]"),
            Some((vec!["SCR1".into()], true))
        );
        assert_eq!(parse_comment("# iglint: ignored"), None);
        assert_eq!(parse_comment("# noqa"), None);
    }
}
