//! Applying fixes to the text of one file.

use std::path::Path;

use iglint_text::{TextEdit, TextRange, TextSize, apply_edits};

use crate::diagnostic::{Applicability, Diagnostic};
use crate::engine::{AppliedFix, SkippedFix};

/// The line terminator a file predominantly uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub(crate) fn detect(text: &str) -> LineEnding {
        match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => LineEnding::CrLf,
            _ => LineEnding::Lf,
        }
    }

    /// Rewrites the newlines in inserted text to this line ending.
    pub(crate) fn normalize(self, content: &str) -> String {
        match self {
            LineEnding::Lf => content.to_owned(),
            LineEnding::CrLf => content.replace("\r\n", "\n").replace('\n', "\r\n"),
        }
    }
}

/// One fix pass over `text`: takes the fixes `wanted` accepts, in position
/// order, skipping any that overlap an earlier accepted fix. Returns the new
/// text and the edits applied (ranges in `text`), or `None` if nothing was
/// applied.
pub(crate) fn apply_pass(
    path: &Path,
    text: &str,
    diagnostics: &[Diagnostic],
    wanted: &dyn Fn(&Diagnostic) -> Result<(), String>,
    applied: &mut Vec<AppliedFix>,
    skipped: &mut Vec<SkippedFix>,
) -> Option<(String, Vec<TextEdit>)> {
    let line_ending = LineEnding::detect(text);
    let mut candidates: Vec<(&Diagnostic, Vec<TextEdit>)> = Vec::new();
    for diagnostic in diagnostics {
        let Some(fix) = &diagnostic.fix else {
            continue;
        };
        let skip = |reason: String| SkippedFix {
            rule: diagnostic.rule,
            message: fix.message.clone(),
            reason,
        };
        if let Err(reason) = wanted(diagnostic) {
            skipped.push(skip(reason));
            continue;
        }
        if fix.edits.iter().any(|e| e.path != path) {
            skipped.push(skip("the fix changes other files".to_owned()));
            continue;
        }
        if fix
            .edits
            .iter()
            .any(|e| e.edit.range.end().to_usize() > text.len())
        {
            skipped.push(skip("the fix is out of date".to_owned()));
            continue;
        }
        let edits = fix
            .edits
            .iter()
            .map(|e| TextEdit::replace(e.edit.range, line_ending.normalize(&e.edit.content)))
            .collect();
        candidates.push((diagnostic, edits));
    }
    candidates.sort_by(|a, b| {
        let start = |edits: &[TextEdit]| edits.iter().map(|e| e.range.start()).min();
        start(&a.1)
            .cmp(&start(&b.1))
            .then_with(|| a.0.rule.cmp(&b.0.rule))
    });
    let mut accepted: Vec<TextEdit> = Vec::new();
    let mut any = false;
    for (diagnostic, edits) in candidates {
        let fix = diagnostic.fix.as_ref().expect("candidate has a fix");
        let mut trial = accepted.clone();
        trial.extend(edits.iter().cloned());
        // Identical fixes (the same edit reported twice) apply once.
        if edits.iter().all(|e| accepted.contains(e)) && !edits.is_empty() {
            applied.push(AppliedFix {
                rule: diagnostic.rule,
                message: fix.message.clone(),
            });
            continue;
        }
        if apply_edits(text, &trial).is_ok() && overlap_free(&trial) {
            accepted = trial;
            any = true;
            applied.push(AppliedFix {
                rule: diagnostic.rule,
                message: fix.message.clone(),
            });
        }
        // Overlapping fixes are retried in the next pass, after the text
        // has been re-analysed.
    }
    if !any {
        return None;
    }
    let fixed = apply_edits(text, &accepted).ok()?;
    Some((fixed, accepted))
}

/// Where `range` of the text before `edits` (non-overlapping, ranges in the
/// old text) lies after them, or `None` when an edit touches it.
pub(crate) fn shift_range(range: TextRange, edits: &[TextEdit]) -> Option<TextRange> {
    let mut delta: i64 = 0;
    for edit in edits {
        let e = edit.range;
        let touches = if range.is_empty() {
            e.start() < range.start() && range.start() < e.end()
        } else if e.is_empty() {
            range.start() < e.start() && e.start() < range.end()
        } else {
            e.start() < range.end() && range.start() < e.end()
        };
        if touches {
            return None;
        }
        if e.end() <= range.start() {
            delta += edit.content.len() as i64 - i64::from(e.len().to_u32());
        }
    }
    let start = i64::from(range.start().to_u32()) + delta;
    let start = u32::try_from(start).ok()?;
    Some(TextRange::at(TextSize::new(start), range.len()))
}

/// `apply_edits` accepts two insertions at the same offset; for edits from
/// different fixes their order would be arbitrary, so that counts as overlap.
fn overlap_free(edits: &[TextEdit]) -> bool {
    let mut ranges: Vec<_> = edits.iter().map(|e| e.range).collect();
    ranges.sort();
    ranges.windows(2).all(|w| {
        let (a, b) = (w[0], w[1]);
        a.end() <= b.start() && !(a == b && a.is_empty())
    })
}

/// Whether a fix of this applicability is applied in this mode.
pub(crate) fn applicability_allowed(applicability: Applicability, unsafe_fixes: bool) -> bool {
    match applicability {
        Applicability::Safe => true,
        Applicability::Unsafe => unsafe_fixes,
        Applicability::DisplayOnly => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifts_ranges_through_edits() {
        let r = |a: u32, b: u32| TextRange::new(TextSize::new(a), TextSize::new(b));
        let edits = vec![
            TextEdit::replace(r(0, 2), "abcd"),
            TextEdit::replace(r(20, 25), ""),
        ];
        assert_eq!(shift_range(r(5, 8), &edits), Some(r(7, 10)));
        assert_eq!(shift_range(r(30, 30), &edits), Some(r(27, 27)));
        assert_eq!(shift_range(r(1, 4), &edits), None);
        assert_eq!(shift_range(r(22, 22), &edits), None);
        assert_eq!(shift_range(r(20, 20), &edits), Some(r(22, 22)));
        // An insertion at the start moves the range; at its end it does not.
        let insert = vec![TextEdit::replace(r(5, 5), "xy")];
        assert_eq!(shift_range(r(5, 8), &insert), Some(r(7, 10)));
        assert_eq!(shift_range(r(2, 5), &insert), Some(r(2, 5)));
        assert_eq!(shift_range(r(4, 6), &insert), None);
    }

    #[test]
    fn line_endings() {
        assert_eq!(LineEnding::detect("a\r\nb\n"), LineEnding::CrLf);
        assert_eq!(LineEnding::detect("a\nb\r\n"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("abc"), LineEnding::Lf);
        assert_eq!(LineEnding::CrLf.normalize("x\ny\r\n"), "x\r\ny\r\n");
    }
}
