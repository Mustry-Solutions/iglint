//! SCR124 trailing-whitespace.

use iglint_text::TextRange;

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, RuleImpl, ScriptRule, ScriptUnit};
use crate::rules::script::{fix, in_header, report, string_ranges, trailing_whitespace};

pub static META: RuleMeta = RuleMeta {
    code: "SCR124",
    name: "trailing-whitespace",
    category: Category::Script,
    default_severity: Severity::Info,
    default_enabled: false,
    fix: FixAvailability::Always,
    summary: "Spaces or tabs at the end of a script line",
    explanation: r#"## What it does

Reports spaces, tabs and form feeds at the end of script lines, also on
lines that hold nothing else, in library modules, event files and every
script embedded in a view or another resource.

Whitespace inside a multi-line string literal is part of the string's
value and is not reported. Scripts that do not parse are not checked
(`SCR001` reports them).

## Why is this bad?

Trailing whitespace is invisible in the Designer's script editor, makes
diffs noisy and often differs between developers' editors, so the same
script keeps changing in version control.

## Example

```python
def runAction(self, event):
	value = event.value   
	self.props.text = value
```

Use instead:

```python
def runAction(self, event):
	value = event.value
	self.props.text = value
```

## Fix

Removes the whitespace. Safe: the whitespace is in no value and no line
moves. Like every fix, it is unsafe while code in the workspace may read the
files a fix edits (a read whose path is not a literal outside the
workspace, or code that is not vetted); the project setting `[fix]
assume-scripts-do-not-read-workspace-files = true` declares that no script
does (see `docs/internal/fix-safety.md`, "File reads").

## pylint

Corresponds to `C0303 trailing-whitespace`.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Script(&TrailingWhitespace)],
    options: None,
};

struct TrailingWhitespace;

impl ScriptRule for TrailingWhitespace {
    fn check_script(&self, cx: &mut Checker<'_, '_>, unit: &ScriptUnit<'_, '_>) {
        if !unit.parsed.errors.is_empty() {
            return;
        }
        let script = unit.script;
        let source: &str = &script.source;
        let strings = string_ranges(unit.parsed);
        let in_string = |r: TextRange| {
            strings
                .iter()
                .any(|s| s.start() < r.start() && r.start() < s.end())
        };
        let found: Vec<TextRange> =
            trailing_whitespace(source, script.source_map.body_start(), true, |c| {
                matches!(c, ' ' | '\t' | '\x0c')
            })
            .into_iter()
            .filter(|r| !in_header(unit, *r) && !in_string(*r))
            .collect();
        for range in found {
            let edit = fix(
                cx,
                unit,
                "Remove trailing whitespace",
                Applicability::Safe,
                vec![(range, String::new())],
            );
            let _ = report(cx, unit, range, "Trailing whitespace").fix_opt(edit);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;

    #[test]
    fn reports_line_ends_outside_strings() {
        let code = "x = 1  \n\t\ny = '''a  \nb'''   \nz = 2\t\r\nw = 3 \x0c";
        let d = lint_module("SCR124", code);
        assert_eq!(
            lines(&d),
            [
                (1, "  ".to_owned()),
                (2, "\t".to_owned()),
                (4, "   ".to_owned()),
                (5, "\t".to_owned()),
                (6, " \x0c".to_owned())
            ]
        );
        let f = library("m", code);
        let fixed = f.fixed(&[("SCR124", "")], false, &library_path(&f, "m"));
        assert_eq!(fixed, "x = 1\n\ny = '''a  \nb'''\nz = 2\r\nw = 3");
    }

    /// Whitespace outside string literals is not in any value and line
    /// numbers do not move: Jython has no source for scripts compiled from
    /// strings (`inspect.getsource` fails, tracebacks show no lines;
    /// verified on Ignition 8.3's Jython). But code that is not vetted or
    /// reads frames may read the file itself (`open` reached under another
    /// name), so it makes the fix unsafe, unless the project declares that
    /// no script reads workspace files.
    #[test]
    fn dynamic_code_makes_the_fix_unsafe_unless_declared() {
        let code = "import __builtin__  \nimport inspect\nev = eval  \nexec 'x = 1'  \nprint getattr(x, n)  \nprint inspect.getsource(f), inspect.currentframe().f_lineno  \n";
        let f = library("m", code);
        let d = f.lint(&["SCR124"]);
        assert_eq!(d.len(), 5);
        for d in &d {
            let fix = d.fix.as_ref().unwrap();
            assert_eq!(fix.applicability, crate::Applicability::Unsafe);
            let reason = fix.unsafe_reason.as_ref().unwrap();
            assert!(reason.message.contains("may read the files"), "{reason:?}");
            assert_eq!(reason.site.as_ref().unwrap().what, "__builtin__");
        }
        let d = f
            .run_assuming(&[("SCR124", "")], crate::FixMode::None, true)
            .diagnostics;
        assert_eq!(d.len(), 5);
        assert!(
            d.iter()
                .all(|d| d.fix.as_ref().unwrap().applicability == crate::Applicability::Safe)
        );
    }

    /// Reviewer round 10 (Codex) blocker: a script that reads a file with
    /// trailing whitespace sees the fix (`open(...).read().splitlines()`).
    /// A read of a computed path makes the fix unsafe, with the site named;
    /// a read of a literal path outside the workspace does not.
    #[test]
    fn file_reads_make_the_fix_unsafe() {
        let reader = "def last(p):\n    return open(p).read().splitlines()[-1]\n";
        let f = with_library(library("m", "x = 1  # note  \n"), "r", reader);
        let d = f.lint(&["SCR124"]);
        assert_eq!(d.len(), 1);
        let fix = d[0].fix.as_ref().unwrap();
        assert_eq!(fix.applicability, crate::Applicability::Unsafe);
        let site = fix.unsafe_reason.as_ref().unwrap().site.as_ref().unwrap();
        assert_eq!(site.what, "open(p)");
        assert_eq!((site.line, site.column), (2, 12));
        assert!(!safe_fix_applies(&f, "SCR124", "m"));
        // Opt-in: the user declares that no script reads workspace files.
        assert_eq!(
            f.fixed_assuming(&[("SCR124", "")], false, &library_path(&f, "m"), true),
            "x = 1  # note\n"
        );
        for outside in [
            "x = open('/tmp/report.csv').read()\n",
            "def w(p, s):\n    open(p, 'w').write(s)\n",
            "import os\nx = os.listdir(p)\n",
        ] {
            let f = with_library(library("m", "x = 1  \n"), "r", outside);
            let d = f.lint(&["SCR124"]);
            assert_eq!(
                d[0].fix.as_ref().unwrap().applicability,
                crate::Applicability::Safe,
                "{outside}"
            );
        }
    }

    #[test]
    fn clean_and_broken_scripts_are_not_reported() {
        assert!(lint_module("SCR124", "x = 1\ny = 2\n").is_empty());
        assert!(lint_module("SCR124", "x = (  \n").is_empty());
    }

    #[test]
    fn embedded_scripts_keep_their_escapes() {
        let f = view(
            ViewScript::Action,
            "\tx \u{3d} 1 \t\n\tself.props.text \u{3d} x  ",
        );
        let d = f.lint(&["SCR124"]);
        assert_eq!(decoded_flagged(&d), [" \t", "  "]);
        let fixed = f.fixed(&[("SCR124", "")], false, &view_path(&f));
        assert!(
            fixed.contains(r#""script": "\tx \u003d 1\n\tself.props.text \u003d x""#),
            "{fixed}"
        );
    }
}
