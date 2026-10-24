//! SCR301 invalid-event-script-header.

use iglint_project::model::ModelData;
use iglint_project::{HeaderStatus, RunScope, ScriptFile};
use iglint_python::ast::Expr;
use iglint_text::{TextEdit, TextRange, TextSize};

use crate::checker::Checker;
use crate::diagnostic::{Applicability, Fix, Severity};
use crate::registry::{Category, FixAvailability, RuleMeta};
use crate::rule::{Hook, Phase, ResourceRule, RuleImpl};
use crate::rules::script::walk_exprs;

pub static META: RuleMeta = RuleMeta {
    code: "SCR301",
    name: "invalid-event-script-header",
    category: Category::Script,
    default_severity: Severity::Error,
    default_enabled: true,
    fix: FixAvailability::Sometimes,
    fix_safety: crate::registry::FixSafety::Unsafe,
    summary: "Event script file whose first line is not the canonical header",
    explanation: r#"## What it does

Checks line 1 of the `.py` files that hold a complete function: gateway
events (startup, shutdown, update, timer, scheduled, tag change, message),
Perspective session and page events and WebDev handlers. Ignition never
runs the header written in the file: when line 1 starts with
`def <name>(`, it drops the whole line and compiles the rest under its
own canonical header (for example `def handleMessage(payload):`);
otherwise it compiles the whole file as the body. The rule reports:

* a header with other parameters or defaults: they have no effect, and
  parameters that are not in the canonical header are undefined at run
  time;
* a one-line function (`def handleTimerEvent(): doWork()`): the body on
  line 1 is dropped with the header and never runs;
* a header wrapped onto several lines: only line 1 is dropped, and the
  rest of the header becomes a syntax error;
* a file whose first code line is not indented and not the header (top
  level code, a header below comments or blank lines, a function with
  another name): Ignition compiles it as the body of the canonical
  header, which fails because the body is not indented.

Differences that change nothing are not reported: whitespace
(`def onStartup( ):`), a comment after the colon, and defaults on the
canonical parameters (`def handleMessage(payload=None):`), since Ignition
always passes those arguments. A header-less file whose code is indented is the legacy
body-only style and compiles; it is not reported. Vision Client event
files are not checked.

## Why is this bad?

The script silently runs differently from what the file says, or does not
compile at all.

## Example

```python
def handleMessage(payload, retries=3):
	for i in range(retries):
		process(payload)
```

Use instead:

```python
def handleMessage(payload):
	retries = 3
	for i in range(retries):
		process(payload)
```

## Fix

Rewrites line 1 to the canonical header when it is a complete one-line
`def` (keeping a comment after the colon); moves a one-line body to its
own indented line; for a header-less file of top-level code, inserts the
canonical header and indents every line with one tab, unless the file has
a string that spans lines (indenting would change its value). Wrapped
headers and misplaced headers are not fixed. Unsafe: code that relied on
written parameters or defaults must be adapted.
"#,
    ..RuleMeta::DEFAULT
};

pub static RULE: RuleImpl = RuleImpl {
    meta: &META,
    phase: Phase::Local,
    hooks: &[Hook::Resource(&InvalidEventScriptHeader)],
    options: None,
};

struct InvalidEventScriptHeader;

impl ResourceRule for InvalidEventScriptHeader {
    fn check_resource(&self, cx: &mut Checker<'_, '_>, data: &ModelData<'_>) {
        match data {
            ModelData::EventScript(e) => {
                if let Some(script) = &e.script {
                    check(cx, script);
                }
            }
            ModelData::WebDev(w) => {
                for m in &w.methods {
                    if let Some(script) = &m.script {
                        check(cx, script);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Line 1 when it starts with `def <name>(`.
enum DefLine<'t> {
    /// The parameter list does not close on line 1.
    Wrapped,
    /// `def name(...)` not followed by `:`.
    NoColon,
    Complete {
        /// The text between the parentheses.
        params: &'t str,
        /// The line up to and including the `:`.
        header: &'t str,
        /// What follows the colon.
        tail: &'t str,
    },
}

fn parse_def_line<'t>(line: &'t str, function: &str) -> DefLine<'t> {
    // Only called for a line that starts with `def function(`.
    let Some(after_open) = line
        .strip_prefix("def ")
        .and_then(|r| r.strip_prefix(function))
        .and_then(|r| r.strip_prefix('('))
    else {
        return DefLine::NoColon;
    };
    let mut depth = 1;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut close = None;
    for (i, c) in after_open.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            '#' => break,
            _ => {}
        }
    }
    let Some(close) = close else {
        return DefLine::Wrapped;
    };
    // `close` is where `char_indices` found a one-byte closing bracket.
    let (params, after_close) = after_open.split_at(close);
    let rest = after_close
        .strip_prefix([')', ']', '}'])
        .unwrap_or(after_close);
    let Some(tail) = rest.trim_start().strip_prefix(':') else {
        return DefLine::NoColon;
    };
    DefLine::Complete {
        params,
        header: line.strip_suffix(tail).unwrap_or(line),
        tail,
    }
}

fn without_whitespace(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The parameter names written in a header (best effort, for messages).
fn written_names(params: &str) -> Vec<&str> {
    params
        .split(',')
        .filter_map(|p| {
            let name = p.split('=').next()?.trim().trim_start_matches('*').trim();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

fn check(cx: &mut Checker<'_, '_>, script: &ScriptFile<'_>) {
    if script.kind.run_scope() == RunScope::VisionClient {
        return;
    }
    let text: &str = &script.written.source;
    let function = script.kind.function_name();
    let canonical = script.kind.canonical_header();
    let line1 = script.first_line.slice(text);
    let newline = if text
        .get(line1.len()..)
        .is_some_and(|r| r.starts_with("\r\n"))
    {
        "\r\n"
    } else {
        "\n"
    };
    let (message, edit) = match script.header {
        HeaderStatus::Canonical => return,
        HeaderStatus::Replaced => match parse_def_line(line1, function) {
            DefLine::Wrapped => (
                format!(
                    "The header continues past line 1: Ignition drops only line 1 and compiles the rest under `{canonical}`, which is a syntax error; write the header on one line"
                ),
                None,
            ),
            DefLine::NoColon => (
                format!(
                    "Line 1 is not the header `{canonical}`; Ignition drops it and compiles the rest under its own header"
                ),
                None,
            ),
            DefLine::Complete {
                params,
                header,
                tail,
            } => {
                let tail_code = tail.trim();
                if tail_code.is_empty() || tail_code.starts_with('#') {
                    if without_whitespace(header) == without_whitespace(&canonical) {
                        return;
                    }
                    let canonical_params = script.kind.params();
                    // Defaults on the canonical parameters change nothing:
                    // Ignition always passes those arguments.
                    if !params.contains('*') && written_names(params) == canonical_params {
                        return;
                    }
                    let extra: Vec<String> = written_names(params)
                        .into_iter()
                        .filter(|n| !canonical_params.contains(n))
                        .map(|n| format!("`{n}`"))
                        .collect();
                    let mut message = format!(
                        "Ignition replaces this header with `{canonical}`: the parameters written here have no effect"
                    );
                    if params.contains('=') {
                        message.push_str(", defaults are ignored");
                    }
                    if !extra.is_empty() {
                        message.push_str(&format!(
                            ", and {} {} undefined at run time",
                            extra.join(", "),
                            if extra.len() == 1 { "is" } else { "are" }
                        ));
                    }
                    (
                        message,
                        Some(TextEdit::replace(
                            TextRange::new(TextSize::ZERO, TextSize::of(header)),
                            canonical.clone(),
                        )),
                    )
                } else {
                    let message = format!(
                        "One-line function: Ignition drops the whole of line 1, so `{tail_code}` never runs"
                    );
                    (
                        message,
                        move_body_edit(text, line1, &canonical, tail_code, newline),
                    )
                }
            }
        },
        HeaderStatus::Missing => {
            let body = text.strip_prefix('\u{feff}').unwrap_or(text);
            let bom = text.len() - body.len();
            let mut offset = bom;
            let mut first_code = None;
            for line in body.split_inclusive('\n') {
                let content = line.trim_end_matches(['\n', '\r']);
                if !content.trim().is_empty() && !content.trim_start().starts_with('#') {
                    first_code = Some((offset, content));
                    break;
                }
                offset += line.len();
            }
            let Some((start, code_line)) = first_code else {
                return;
            };
            if code_line.starts_with([' ', '\t']) {
                // Legacy body-only file: compiles under the canonical header.
                return;
            }
            let range = TextRange::at(TextSize::new(start as u32), TextSize::of(code_line));
            let message = if code_line.starts_with(&format!("def {function}(")) {
                format!(
                    "The header must be the first line: Ignition only recognizes `def {function}(` on line 1, so it compiles the whole file as the body of `{canonical}`, which fails because the body is not indented"
                )
            } else if code_line.starts_with("def ") {
                format!(
                    "Line 1 does not define `{function}`: Ignition compiles the whole file as the body of `{canonical}`, which fails because the body is not indented"
                )
            } else {
                format!(
                    "The file has no `{canonical}` header: Ignition compiles the whole file as the body of that header, which fails because the code is not indented"
                )
            };
            let edit = (!code_line.starts_with("def ") && bom == 0)
                .then(|| insert_header_edit(text, &canonical, newline))
                .flatten();
            let fix =
                edit.map(|e| make_fix(cx, script, "Insert the header and indent the file", e));
            let _ = cx
                .report_in(script.file, range, message)
                .node_path(script.written.node_path.clone())
                .fix_opt(fix);
            return;
        }
    };
    let range = if script.first_line.is_empty() {
        TextRange::empty(TextSize::ZERO)
    } else {
        script.first_line
    };
    let fix = edit.map(|e| make_fix(cx, script, format!("Use `{canonical}`"), e));
    let _ = cx
        .report_in(script.file, range, message)
        .node_path(script.written.node_path.clone())
        .fix_opt(fix);
}

fn make_fix(
    cx: &Checker<'_, '_>,
    script: &ScriptFile<'_>,
    message: impl Into<String>,
    edit: TextEdit,
) -> Fix {
    Fix {
        message: message.into(),
        applicability: Applicability::Unsafe,
        edits: cx.file_edits(script.file, vec![edit]),
        suite: None,
        unsafe_reason: Some(crate::diagnostic::UnsafeReason::new(
            "the fix changes the code Ignition runs: it repairs the header the gateway replaces",
        )),
    }
}

/// `def f(): body` becomes `canonical` and `body` on the next line, when
/// the following code (if any) is indented: then the body joins it.
fn move_body_edit(
    text: &str,
    line1: &str,
    canonical: &str,
    body: &str,
    newline: &str,
) -> Option<TextEdit> {
    let rest = text.get(line1.len()..).unwrap_or_default();
    let next = rest
        .lines()
        .skip(1)
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'));
    let indent = match next {
        None => "\t",
        Some(l) => {
            let indent = l
                .strip_suffix(l.trim_start_matches([' ', '\t']))
                .unwrap_or_default();
            if indent.is_empty() {
                return None;
            }
            indent
        }
    };
    Some(TextEdit::replace(
        TextRange::new(TextSize::ZERO, TextSize::of(line1)),
        format!("{canonical}{newline}{indent}{body}"),
    ))
}

/// Inserts `canonical` and indents every non-blank line with a tab, when
/// the file parses and has no string literal spanning lines.
fn insert_header_edit(text: &str, canonical: &str, newline: &str) -> Option<TextEdit> {
    let parsed = iglint_python::parse_module(text);
    if !parsed.errors.is_empty() {
        return None;
    }
    let mut multiline = false;
    walk_exprs(&parsed.module.body, &mut |e| {
        if let Expr::Str(s) = e
            && s.range.slice(text).contains('\n')
        {
            multiline = true;
        }
    });
    if multiline {
        return None;
    }
    let mut out = String::with_capacity(text.len() + canonical.len() + 64);
    out.push_str(canonical);
    out.push_str(newline);
    for line in text.split_inclusive('\n') {
        if !line.trim().is_empty() {
            out.push('\t');
        }
        out.push_str(line);
    }
    Some(TextEdit::replace(
        TextRange::new(TextSize::ZERO, TextSize::of(text)),
        out,
    ))
}

#[cfg(test)]
mod tests {
    use crate::rules::script::testing::*;
    use crate::rules::test_support::{Fixture, PERSPECTIVE};

    fn timer_path(f: &Fixture) -> std::path::PathBuf {
        f.path("projects/p/ignition/timer/t/handleTimerEvent.py")
    }

    fn message_path(f: &Fixture) -> std::path::PathBuf {
        f.path("projects/p/ignition/message/m/handleMessage.py")
    }

    #[test]
    fn canonical_and_whitespace_only_headers_are_fine() {
        for code in [
            "def handleMessage(payload):\n\tpass",
            "def handleMessage( payload ) :\n\tpass\n",
            "def handleMessage(payload):  # handles it\r\n\tpass\r\n",
            "def handleMessage(payload=None):\n\tpass\n",
        ] {
            assert!(gateway_message(code).lint(&["SCR301"]).is_empty(), "{code}");
        }
    }

    #[test]
    fn written_parameters_and_defaults() {
        let f = gateway_message(
            "def handleMessage(payload=None, extra=5):  # note\n\treturn payload\n",
        );
        let d = f.lint(&["SCR301"]);
        assert_eq!(
            flagged(&d),
            ["def handleMessage(payload=None, extra=5):  # note"]
        );
        assert_eq!(
            messages(&d),
            [
                "Ignition replaces this header with `def handleMessage(payload):`: the parameters written here have no effect, defaults are ignored, and `extra` is undefined at run time"
            ]
        );
        let fixed = f.fixed(&[("SCR301", "")], true, &message_path(&f));
        assert_eq!(
            fixed,
            "def handleMessage(payload):  # note\n\treturn payload\n"
        );
        // Unsafe: not applied without unsafe fixes.
        let d = gateway_timer("def handleTimerEvent(x):\n\tpass").lint(&["SCR301"]);
        assert_eq!(
            d[0].fix.as_ref().map(|f| f.applicability),
            Some(crate::diagnostic::Applicability::Unsafe)
        );
    }

    #[test]
    fn one_line_functions() {
        let f = gateway_timer("def handleTimerEvent(): doWork()\n");
        let d = f.lint(&["SCR301"]);
        assert_eq!(
            messages(&d),
            ["One-line function: Ignition drops the whole of line 1, so `doWork()` never runs"]
        );
        let fixed = f.fixed(&[("SCR301", "")], true, &timer_path(&f));
        assert_eq!(fixed, "def handleTimerEvent():\n\tdoWork()\n");
        // The body joins indented code below, with its indentation.
        let f = gateway_timer("def handleTimerEvent(): a()  \r\n    b()\r\n");
        let fixed = f.fixed(&[("SCR301", "")], true, &timer_path(&f));
        assert_eq!(fixed, "def handleTimerEvent():\r\n    a()\r\n    b()\r\n");
        // Top-level code below: no fix.
        let d = gateway_timer("def handleTimerEvent(): a()\nb()\n").lint(&["SCR301"]);
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn wrapped_headers() {
        let d =
            gateway_message("def handleMessage(payload,\n\t\textra):\n\tpass\n").lint(&["SCR301"]);
        assert_eq!(flagged(&d), ["def handleMessage(payload,"]);
        assert!(messages(&d)[0].contains("syntax error"));
        assert!(d[0].fix.is_none());
        // Parentheses inside default strings do not close the list.
        let d = gateway_message("def handleMessage(p=')'):\n\tpass\n").lint(&["SCR301"]);
        assert!(d[0].fix.is_some());
    }

    #[test]
    fn header_less_files() {
        let f = gateway_timer("# tick\nx = 1\nif x:\n    y = 2\n\nz = '''a'''\n");
        let d = f.lint(&["SCR301"]);
        assert_eq!(flagged(&d), ["x = 1"]);
        assert!(messages(&d)[0].starts_with("The file has no `def handleTimerEvent():` header"));
        let fixed = f.fixed(&[("SCR301", "")], true, &timer_path(&f));
        assert_eq!(
            fixed,
            "def handleTimerEvent():\n\t# tick\n\tx = 1\n\tif x:\n\t    y = 2\n\n\tz = '''a'''\n"
        );
        // A string spanning lines would change: no fix.
        let d = gateway_timer("x = '''a\nb'''\n").lint(&["SCR301"]);
        assert_eq!(d.len(), 1);
        assert!(d[0].fix.is_none());
        // Legacy body-only files compile.
        assert!(gateway_timer("\tx = 1\n").lint(&["SCR301"]).is_empty());
        // Empty files are not about the header.
        assert!(gateway_timer("\n# nothing\n").lint(&["SCR301"]).is_empty());
    }

    #[test]
    fn misplaced_or_misnamed_headers() {
        let d = gateway_timer("# Timer\ndef handleTimerEvent():\n\tpass\n").lint(&["SCR301"]);
        assert_eq!(flagged(&d), ["def handleTimerEvent():"]);
        assert!(messages(&d)[0].starts_with("The header must be the first line"));
        assert!(d[0].fix.is_none());
        let d = gateway_timer("def onTimer():\n\tpass\n").lint(&["SCR301"]);
        assert!(messages(&d)[0].starts_with("Line 1 does not define `handleTimerEvent`"));
        assert!(d[0].fix.is_none());
    }

    #[test]
    fn session_events_and_webdev() {
        let d = session_event("def onStartup(session, extra):\n\tpass\n").lint(&["SCR301"]);
        assert_eq!(flagged(&d), ["def onStartup(session, extra):"]);
        let f = webdev("def doGet(request):\n\treturn {'json': 1}\n");
        let d = f.lint(&["SCR301"]);
        assert_eq!(flagged(&d), ["def doGet(request):"]);
        let fixed = f.fixed(
            &[("SCR301", "")],
            true,
            &f.path("projects/p/com.inductiveautomation.webdev/resources/api/doGet.py"),
        );
        assert_eq!(
            fixed,
            "def doGet(request, session):\n\treturn {'json': 1}\n"
        );
        let f = Fixture::new().resource(
            &format!("projects/p/{PERSPECTIVE}/message/m"),
            &[("handleMessage.py", "def handleMessage(payload):\n\tpass\n")],
        );
        assert_eq!(
            messages(&f.lint(&["SCR301"])),
            [
                "Ignition replaces this header with `def handleMessage(session, payload):`: the parameters written here have no effect"
            ]
        );
    }

    #[test]
    fn vision_client_events_are_not_checked() {
        let f = Fixture::new().resource(
            "projects/p/com.inductiveautomation.vision/timer/t",
            &[("handleTimerEvent.py", "def handleTimerEvent(x):\n\tpass\n")],
        );
        assert!(f.lint(&["SCR301"]).is_empty());
    }
}
