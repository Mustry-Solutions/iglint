//! Suppression comments in scripts.
//!
//! ```python
//! x = undefined_thing  # iglint: ignore[SCR101]
//! y = 1  # iglint: ignore[unused-variable, SCR1]
//! # iglint: ignore-file[SCR102]
//! ```
//!
//! `ignore` applies to findings that start on the comment's line, or on
//! an earlier line of the same logical line: a statement continued inside
//! brackets, after a backslash or inside a triple-quoted string can carry
//! its comment on the line where it ends (a finding that starts on a line
//! opening a triple-quoted string has no room for a comment there).
//! `ignore-file` applies to every finding of the script. Selectors are rule codes,
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
    /// File range from the start of the logical line the comment ends to
    /// the end of the comment's line (whole script for `ignore-file`).
    pub scope: Option<TextRange>,
    /// Selectors; empty means all rules.
    pub selectors: Vec<String>,
    pub file_wide: bool,
    /// Whether it suppressed at least one finding.
    pub used: bool,
    /// Per selector: whether it matched a suppressed finding.
    pub used_selectors: Vec<bool>,
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
        let mut continued: Option<Vec<usize>> = None;
        for comment in comments(source) {
            let text = comment.slice(source);
            let Some(parsed) = parse_comment(text) else {
                continue;
            };
            let continued = continued.get_or_insert_with(|| continued_lines(source));
            let mut line = line_range(source, comment.start());
            // Back to the first physical line of the logical line.
            while line.start() > TextSize::ZERO
                && continued.binary_search(&line.start().to_usize()).is_ok()
            {
                let previous = line_range(source, line.start() - TextSize::new(1));
                line = TextRange::new(previous.start(), line.end());
            }
            items.push(Suppression {
                comment,
                scope: if parsed.1 {
                    None
                } else {
                    Some(script.file_range(line))
                },
                used_selectors: vec![false; parsed.0.len()],
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
                    for (selector, used) in s.selectors.iter().zip(&mut s.used_selectors) {
                        *used |= selects(std::slice::from_ref(selector), d);
                    }
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
pub(crate) fn parse_comment(comment: &str) -> Option<(Vec<String>, bool)> {
    let body = comment.strip_prefix('#')?.trim_start();
    let rest = body.strip_prefix("iglint:")?.trim_start();
    let (file_wide, rest) = match rest.strip_prefix("ignore-file") {
        Some(r) => (true, r),
        None => (false, rest.strip_prefix("ignore")?),
    };
    let rest = rest.trim_start();
    if let Some(list) = rest.strip_prefix('[') {
        let (inside, _) = list.split_once(']')?;
        let selectors = inside
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
    let at = iglint_text::floor_char_boundary(text, offset.to_usize());
    let (before, after) = text.split_at_checked(at).unwrap_or((text, ""));
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let end = after.find('\n').map_or(text.len(), |i| at + i);
    TextRange::from(start..end)
}

/// Offsets (ascending) of the line starts that continue the logical line
/// before them: inside brackets, after a backslash, or inside a string
/// (a triple-quoted one, or a quoted one continued with a backslash).
fn continued_lines(source: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'\\' => {
                // A backslash at the end of a line (before `\n` or `\r\n`).
                let next = if b.get(i + 1) == Some(&b'\r') {
                    i + 2
                } else {
                    i + 1
                };
                if b.get(next) == Some(&b'\n') {
                    out.push(next + 1);
                    i = next + 1;
                } else {
                    i += 1;
                }
            }
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\n' => {
                if depth > 0 {
                    out.push(i + 1);
                }
                i += 1;
            }
            q @ (b'\'' | b'"') => {
                let triple = b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q);
                i += if triple { 3 } else { 1 };
                while i < b.len() {
                    match b[i] {
                        b'\\' => {
                            if b.get(i + 1) == Some(&b'\n') {
                                out.push(i + 2);
                            }
                            i += 2;
                        }
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
                        b'\n' if triple => {
                            out.push(i + 1);
                            i += 1;
                        }
                        b'\n' => break,
                        _ => i += 1,
                    }
                }
            }
            _ => i += 1,
        }
    }
    out
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
                let end = source
                    .get(i..)
                    .and_then(|rest| rest.find('\n'))
                    .map_or(b.len(), |e| i + e);
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
    fn continued_lines_follow_brackets_backslashes_and_strings() {
        let src = "a = f(1,\n  2)\nb = 1 + \\\n  2\nc = '''x\ny'''\nd = 'x\\\ny'\n# (\ne = 1\n";
        let starts: Vec<usize> = continued_lines(src);
        let lines: Vec<&str> = starts
            .iter()
            .map(|&s| src.get(s..).unwrap().lines().next().unwrap())
            .collect();
        assert_eq!(lines, ["  2)", "  2", "y'''", "y'"]);
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
