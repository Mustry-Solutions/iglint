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
used (the whole line when the comment is alone on it). Safe.
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

fn verdict(selector: &str, set: &RuleSet) -> Verdict {
    let matched: Vec<_> = script_rules()
        .filter(|m| selected_by(selector, m))
        .collect();
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
    for s in &suppressions.items {
        let mut unused: Vec<(usize, String)> = Vec::new();
        if s.selectors.is_empty() {
            if s.used {
                continue;
            }
            if script_rules().all(|m| set.active.iter().any(|a| a.rule.code() == m.code)) {
                unused.push((0, "Unused `# iglint: ignore` comment".into()));
            }
        } else {
            for (i, selector) in s.selectors.iter().enumerate() {
                if s.used_selectors.get(i).copied().unwrap_or(false) {
                    continue;
                }
                match verdict(selector, set) {
                    Verdict::Unused(message) => unused.push((i, message)),
                    Verdict::Unknown => {}
                }
            }
        }
        if unused.is_empty() {
            continue;
        }
        let all_unused = s.selectors.is_empty() || unused.len() == s.selectors.len();
        let edit = if all_unused {
            remove_comment(source, s.comment)
        } else {
            rewrite_selectors(source, s.comment, &s.selectors, &unused)
        };
        let fix = edit.and_then(|e| {
            fix(
                cx,
                unit,
                "Remove the unused suppression",
                Applicability::Safe,
                vec![e],
            )
        });
        for (_, message) in unused {
            let _ = report(cx, unit, s.comment, message).fix_opt(fix.clone());
        }
    }
}

/// Deletes a comment and the whitespace before it, or its whole line when
/// it is alone on the line.
fn remove_comment(source: &str, comment: TextRange) -> Option<(TextRange, String)> {
    let start = comment.start().to_usize();
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let end = comment.end().to_usize();
    if source[line_start..start].trim().is_empty() {
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
            "import os  # iglint: ignore[SCR102]\nprint os\n# iglint: ignore[SCR102]\nx = 1\n",
        );
        let fixed = f.fixed(
            &[("SCR102", ""), ("SCR123", "")],
            false,
            &library_path(&f, "m"),
        );
        assert_eq!(fixed, "import os\nprint os\nx = 1\n");
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
}
