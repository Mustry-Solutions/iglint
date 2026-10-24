//! SCR123 unused-suppression.
//!
//! Runs after every script rule of both phases (see `analyze::run_phase`).

use iglint_text::{TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, RuleSet, ScriptRule, ScriptUnit};
use crate::rules::script::{fix, report};
use crate::suppression::Suppressions;

pub static META: RuleMeta = RuleMeta {
    code: "SCR123",
    name: "unused-suppression",
    category: Category::Script,
    default_severity: Severity::Info,
    default_enabled: true,
    fix: FixAvailability::Always,
    fix_safety: crate::registry::FixSafety::Depends,
    summary: "`# iglint: ignore[...]` that suppresses nothing",
    explanation: r#"## What it does

Reports `# iglint: ignore[...]` and `# iglint: ignore-file[...]` comments,
or single codes in them, that did not suppress any finding in this run.

A code is only reported when every script rule it selects is enabled
(otherwise the comment may be needed under another configuration).
Codes that select no rule at all, or only rules that never report in
scripts, are always reported. A bare `# iglint: ignore` is reported only
when every script rule is enabled.

## Why is this bad?

Stale suppressions hide future findings on the same line and make readers
believe there is a problem there.

## Example

```python
total = 0  # iglint: ignore[SCR103]
return total
```

Use instead:

```python
total = 0
return total
```

## Fix

Removes the unused codes, or the whole comment when none of its codes is
used. A comment alone on its line leaves the line empty, so no line number
changes. Safe, except when the whole comment would go and it holds more
than the suppression (a reason, a `# pylint:` directive, a `coding:`
declaration); that fix is unsafe and removes the comment's line too.

Like every fix, it is unsafe while code in the workspace may read the
files a fix edits (any file access, read, write, rename or delete, whose path
is not a literal outside the
workspace, or code that is not vetted); the project setting `[fix]
assume-scripts-do-not-read-workspace-files = true` declares that no script
does (see `docs/internal/fix-safety.md`, "File reads").
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Project,
    hooks: &[Hook::Script(&Marker)],
    options: None,
};

/// Makes the driver walk scripts in the project phase; the work happens in
/// [`report`], after the other script rules.
struct Marker;

impl ScriptRule for Marker {
    fn check_script(&self, _cx: &mut Checker<'_, '_>, _unit: &ScriptUnit<'_, '_>) {}
}

/// The slot of SCR123 in `set`, when it is enabled.
pub(crate) fn slot(set: &RuleSet) -> Option<usize> {
    set.active.iter().position(|a| a.rule.code() == META.code)
}

/// Rule codes that can report findings inside scripts.
fn script_rules() -> impl Iterator<Item = &'static crate::registry::RuleMeta> {
    crate::rules::IMPLS
        .iter()
        .filter(|imp| {
            imp.hooks.iter().any(|h| matches!(h, Hook::Script(_))) && imp.meta.code != META.code
        })
        .map(|imp| imp.meta)
}

fn selected_by(selector: &str, meta: &crate::registry::RuleMeta) -> bool {
    meta.name == selector
        || meta
            .code
            .starts_with(selector.to_ascii_uppercase().as_str())
}

enum Verdict {
    Unknown,
    Unused(String),
}

/// `pylint_ran`: whether `SCR900 pylint` results were available for the
/// unit (they are not when pylint could not run, or while a file is
/// re-analysed after fixes), so its suppressions can be judged.
fn verdict(selector: &str, set: &RuleSet, pylint_ran: bool) -> Verdict {
    let matched: Vec<_> = script_rules()
        .filter(|m| selected_by(selector, m))
        .collect();
    if !pylint_ran && matched.iter().any(|m| m.code == super::pylint::META.code) {
        return Verdict::Unknown;
    }
    if matched.is_empty() {
        let any_rule = crate::rules::ALL.iter().any(|m| selected_by(selector, m));
        return Verdict::Unused(if any_rule {
            format!("`{selector}` never reports inside scripts")
        } else {
            format!("`{selector}` is not a rule code or name")
        });
    }
    if matched
        .iter()
        .all(|m| set.active.iter().any(|a| a.rule.code() == m.code))
    {
        Verdict::Unused(format!("Unused suppression of `{selector}`"))
    } else {
        Verdict::Unknown
    }
}

pub(crate) fn report_unused(
    cx: &mut Checker<'_, '_>,
    unit: &ScriptUnit<'_, '_>,
    suppressions: &Suppressions,
    set: &RuleSet,
) {
    let source: &str = &unit.script.source;
    let pylint_ran = cx.unit().cache.pylint.get().is_some();
    for s in &suppressions.items {
        let mut unused: Vec<(usize, String)> = Vec::new();
        if s.selectors.is_empty() {
            if s.used {
                continue;
            }
            let all_enabled =
                script_rules().all(|m| set.active.iter().any(|a| a.rule.code() == m.code));
            if all_enabled && pylint_ran {
                unused.push((0, "Unused `# iglint: ignore` comment".into()));
            }
        } else {
            for (i, selector) in s.selectors.iter().enumerate() {
                if s.used_selectors.get(i).copied().unwrap_or(false) {
                    continue;
                }
                match verdict(selector, set, pylint_ran) {
                    Verdict::Unused(message) => unused.push((i, message)),
                    Verdict::Unknown => {}
                }
            }
        }
        if unused.is_empty() {
            continue;
        }
        let all_unused = s.selectors.is_empty() || unused.len() == s.selectors.len();
        // A comment is safe to delete when it holds nothing but the
        // suppression: other text (a reason, a `# pylint:` directive) and a
        // coding declaration are not iglint's to remove.
        let comment = s.comment.slice(source);
        let applicability = if all_unused
            && (!directive_rest(comment).trim().is_empty() || is_coding_declaration(comment))
        {
            Applicability::Unsafe
        } else {
            Applicability::Safe
        };
        // The safe fix keeps the comment's line (empty) so that no line
        // number changes; the unsafe one removes it.
        let edit = if all_unused {
            remove_comment(source, s.comment, applicability == Applicability::Safe)
        } else {
            rewrite_selectors(source, s.comment, &s.selectors, &unused)
        };
        let fix = edit.and_then(|e| {
            fix(
                cx,
                unit,
                "Remove the unused suppression",
                applicability,
                vec![e],
            )
        });
        for (_, message) in unused {
            let _ = report(cx, unit, s.comment, message).fix_opt(fix.clone());
        }
    }
}

/// The text of a suppression comment after its directive
/// (`# iglint: ignore[X]  reason` gives `  reason`).
fn directive_rest(comment: &str) -> &str {
    let Some(at) = comment.find("ignore") else {
        return comment;
    };
    let rest = &comment[at..];
    let rest = rest
        .strip_prefix("ignore-file")
        .or_else(|| rest.strip_prefix("ignore"))
        .unwrap_or(rest);
    match rest.trim_start().strip_prefix('[') {
        Some(list) => list.find(']').map_or("", |end| &list[end + 1..]),
        None => rest,
    }
}

/// Whether a comment matches PEP 263's `coding[:=]` (only meaningful on
/// the first two lines, but never worth deleting).
fn is_coding_declaration(comment: &str) -> bool {
    comment
        .match_indices("coding")
        .any(|(i, m)| comment[i + m.len()..].starts_with([':', '=']))
}

/// Deletes a comment and the whitespace before it. A comment alone on its
/// line leaves an empty line with `keep_line`, and takes its line with it
/// otherwise.
fn remove_comment(
    source: &str,
    comment: TextRange,
    keep_line: bool,
) -> Option<(TextRange, String)> {
    let start = comment.start().to_usize();
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let end = comment.end().to_usize();
    if source[line_start..start].trim().is_empty() {
        if keep_line {
            return Some((TextRange::from(line_start..end), String::new()));
        }
        let line_end = source[end..]
            .find('\n')
            .map_or(source.len(), |i| end + i + 1);
        return Some((TextRange::from(line_start..line_end), String::new()));
    }
    let code_end = source[..start].trim_end_matches([' ', '\t']).len();
    Some((
        TextRange::new(TextSize::from(code_end), comment.end()),
        String::new(),
    ))
}

/// Rewrites the selector list without the unused selectors.
fn rewrite_selectors(
    source: &str,
    comment: TextRange,
    selectors: &[String],
    unused: &[(usize, String)],
) -> Option<(TextRange, String)> {
    let text = comment.slice(source);
    let open = text.find('[')?;
    let close = text[open..].find(']')? + open;
    let kept: Vec<&str> = selectors
        .iter()
        .enumerate()
        .filter(|(i, _)| !unused.iter().any(|(u, _)| u == i))
        .map(|(_, s)| s.as_str())
        .collect();
    let range = TextRange::new(
        comment.start() + TextSize::from(open + 1),
        comment.start() + TextSize::from(close),
    );
    Some((range, kept.join(", ")))
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_unused_codes() {
        let code = "def f():\n    x = 1  # iglint: ignore[SCR103]\n    y = 2  # iglint: ignore[SCR103, SCR101]\n    return z  # iglint: ignore[SCR101]\n";
        let f = library("m", code);
        let d = f.lint(&["SCR101", "SCR103", "SCR123"]);
        assert_eq!(messages(&d), ["Unused suppression of `SCR101`"]);
        assert_eq!(lines(&d)[0].0, 3);
        let fixed = f.fixed(
            &[("SCR101", ""), ("SCR103", ""), ("SCR123", "")],
            false,
            &library_path(&f, "m"),
        );
        assert!(
            fixed.contains("y = 2  # iglint: ignore[SCR103]\n"),
            "{fixed}"
        );
    }

    #[test]
    fn disabled_rules_are_not_judged() {
        let f = library("m", "x = 1  # iglint: ignore[SCR101]\n");
        assert!(f.lint(&["SCR123"]).is_empty());
        // Unknown codes are always reported.
        let f = library("m", "x = 1  # iglint: ignore[NOPE42]\n");
        assert_eq!(
            messages(&f.lint(&["SCR123"])),
            ["`NOPE42` is not a rule code or name"]
        );
    }

    #[test]
    fn removes_whole_comments() {
        let f = library(
            "m",
            "import os  # iglint: ignore[SCR102]\nprint os.sep\n# iglint: ignore[SCR102]\nx = 1\n",
        );
        let fixed = f.fixed(
            &[("SCR102", ""), ("SCR123", "")],
            false,
            &library_path(&f, "m"),
        );
        // The comment's own line stays, empty: no line number changes.
        assert_eq!(fixed, "import os\nprint os.sep\n\nx = 1\n");
    }

    /// The safe fix never moves a line, so it stays safe where code reads
    /// line numbers or frames; the unsafe fix (a comment with a reason)
    /// removes the whole line. (Reviewer repros R6-A: `warnings`,
    /// `logging`, Java stack traces.)
    #[test]
    fn safe_removal_keeps_lines() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        let line = "# iglint: ignore[SCR101]\nx = 1\n";
        for reader in [
            "import sys\nprint sys._getframe(0).f_lineno\n",
            "import warnings\nwith warnings.catch_warnings(record=True) as w:\n    warnings.warn('x')\n    print w[0].lineno\n",
            "from java.lang import Thread\nprint Thread.currentThread().getStackTrace()[1].getLineNumber()\n",
            "import traceback\nx = traceback.format_exc()\n",
        ] {
            let code = format!("{line}{reader}");
            let f = library("m", &code);
            // Each reader is code that is not vetted or reads frames, which
            // may read the file itself: unsafe, unless the project declares
            // that no script reads workspace files.
            let applicability = |assume| -> Vec<_> {
                f.run_assuming(
                    &[("SCR101", ""), ("SCR123", "")],
                    crate::FixMode::None,
                    assume,
                )
                .diagnostics
                .iter()
                .filter(|d| d.rule.code() == "SCR123")
                .map(|d| d.fix.as_ref().map(|f| f.applicability))
                .collect()
            };
            assert_eq!(applicability(false), [Some(Unsafe)], "{reader}");
            assert_eq!(applicability(true), [Some(Safe)], "{reader}");
            let fixed = f.fixed_assuming(
                &[("SCR101", ""), ("SCR123", "")],
                false,
                &library_path(&f, "m"),
                true,
            );
            assert_eq!(fixed, format!("\nx = 1\n{reader}"));
        }
        // The unsafe fix removes the line.
        let f = library("m", "# iglint: ignore[SCR101] legacy\nx = 1\n");
        let d = f.lint(&["SCR101", "SCR123"]);
        assert_eq!(d[0].fix.as_ref().map(|f| f.applicability), Some(Unsafe));
        let fixed = f.fixed(
            &[("SCR101", ""), ("SCR123", "")],
            true,
            &library_path(&f, "m"),
        );
        assert_eq!(fixed, "x = 1\n");
    }

    #[test]
    fn suppressions_in_view_scripts() {
        let f = view(
            ViewScript::Action,
            "\tx \u{3d} undefinedName  # iglint: ignore[SCR101]\n\ty \u{3d} 1  # iglint: ignore[SCR101]",
        );
        let d = f.lint(&["SCR101", "SCR123"]);
        assert_eq!(messages(&d), ["Unused suppression of `SCR101`"]);
        let fixed = f.fixed(&[("SCR101", ""), ("SCR123", "")], false, &view_path(&f));
        assert!(fixed.contains("\\ty \\u003d 1\""), "{fixed}");
    }

    #[test]
    fn comments_with_other_text_have_unsafe_fixes() {
        use crate::diagnostic::Applicability::{Safe, Unsafe};
        for (comment, expected) in [
            ("# iglint: ignore[SCR101]", Safe),
            ("# iglint: ignore[SCR101]   ", Safe),
            ("# iglint: ignore[SCR101] legacy API", Unsafe),
            ("# iglint: ignore[SCR101]  # pylint: disable=W0611", Unsafe),
            ("# iglint: ignore[SCR101] -*- coding: latin-1 -*-", Unsafe),
        ] {
            let f = library("m", &format!("x = 1  {comment}\n"));
            let d = f.lint(&["SCR123", "SCR101"]);
            let found: Vec<_> = d
                .iter()
                .filter(|d| d.rule.code() == "SCR123")
                .map(|d| d.fix.as_ref().map(|f| f.applicability))
                .collect();
            assert_eq!(found, [Some(expected)], "{comment}");
        }
    }
}
