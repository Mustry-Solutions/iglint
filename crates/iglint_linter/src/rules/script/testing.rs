//! Fixtures for script rule tests: library modules, event files and view
//! scripts in a temporary project.

use std::path::PathBuf;

use iglint_json::{EscapeStyle, to_json_string};

use crate::diagnostic::Diagnostic;
use crate::rules::test_support::{Fixture, PERSPECTIVE};

/// Where a view script lives.
#[derive(Clone, Copy, Debug)]
pub enum ViewScript {
    /// `onActionPerformed` script action: `def runAction(self, event):`.
    Action,
    /// Script transform: `def transform(self, value, quality, timestamp):`.
    Transform,
    /// Property change script: `def valueChanged(self, previousValue, ...)`.
    Change,
    /// Message handler: `def onMessageReceived(self, payload):`.
    Handler,
    /// Custom method `go(self,a,b)`.
    Method,
}

/// A JSON string literal the way the Designer writes it.
pub fn json(code: &str) -> String {
    to_json_string(code, EscapeStyle::Ignition)
}

/// A view whose root component has one script of `kind` with body `code`.
pub fn view_json(kind: ViewScript, code: &str) -> String {
    let s = json(code);
    let (props, events, scripts) = match kind {
        ViewScript::Action => (
            String::new(),
            format!(
                r#""events": {{"component": {{"onActionPerformed": {{"config": {{"script": {s}}}, "scope": "G", "type": "script"}}}}}},"#
            ),
            String::new(),
        ),
        ViewScript::Transform => (
            format!(
                r#""propConfig": {{"props.text": {{"binding": {{"config": {{"expression": "1"}}, "transforms": [{{"code": {s}, "type": "script"}}], "type": "expr"}}}}}},"#
            ),
            String::new(),
            String::new(),
        ),
        ViewScript::Change => (
            format!(
                r#""custom": {{"x": 1}}, "propConfig": {{"custom.x": {{"onChange": {{"enabled": true, "script": {s}}}}}}},"#
            ),
            String::new(),
            String::new(),
        ),
        ViewScript::Handler => (
            String::new(),
            String::new(),
            format!(
                r#""scripts": {{"customMethods": [], "extensionFunctions": null, "messageHandlers": [{{"messageType": "m", "pageScope": false, "script": {s}, "sessionScope": true, "viewScope": true}}]}},"#
            ),
        ),
        ViewScript::Method => (
            String::new(),
            String::new(),
            format!(
                r#""scripts": {{"customMethods": [{{"name": "go", "params": ["a", "b"], "script": {s}}}], "extensionFunctions": null, "messageHandlers": []}},"#
            ),
        ),
    };
    format!(
        r#"{{
  "custom": {{}},
  "params": {{}},
  "props": {{}},
  "root": {{
    {events}
    "meta": {{"name": "root"}},
    {props}
    {scripts}
    "type": "ia.display.label"
  }}
}}"#
    )
}

/// A project with a library module `path` (`core/util`) holding `code`.
pub fn library(path: &str, code: &str) -> Fixture {
    Fixture::new().resource(
        &format!("projects/p/ignition/script-python/{path}"),
        &[("code.py", code)],
    )
}

/// Adds a library module to a fixture.
pub fn with_library(f: Fixture, path: &str, code: &str) -> Fixture {
    f.resource(
        &format!("projects/p/ignition/script-python/{path}"),
        &[("code.py", code)],
    )
}

/// A project with a gateway message handler `handleMessage.py`.
pub fn gateway_message(code: &str) -> Fixture {
    Fixture::new().resource(
        "projects/p/ignition/message/m",
        &[("handleMessage.py", code)],
    )
}

/// A project with a Perspective session startup script `onStartup.py`.
pub fn session_event(code: &str) -> Fixture {
    Fixture::new().resource(
        &format!("projects/p/{PERSPECTIVE}/startup"),
        &[("onStartup.py", code)],
    )
}

/// A project with a gateway timer script `handleTimerEvent.py`.
pub fn gateway_timer(code: &str) -> Fixture {
    Fixture::new().resource(
        "projects/p/ignition/timer/t",
        &[("handleTimerEvent.py", code)],
    )
}

/// A project with a WebDev python resource `api` holding `doGet.py`.
pub fn webdev(code: &str) -> Fixture {
    Fixture::new().resource(
        "projects/p/com.inductiveautomation.webdev/resources/api",
        &[
            (
                "config.json",
                r#"{"resource-type": "python-resource", "doGet": {"enabled": true}}"#,
            ),
            ("doGet.py", code),
        ],
    )
}

/// A gateway with a memory tag whose `valueChanged` event script has body
/// `code`.
pub fn tag_event(code: &str) -> Fixture {
    let tags = format!(
        r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "eventScripts": [{{"eventid": "valueChanged", "script": {}}}]}}]"#,
        json(code)
    );
    crate::rules::tag::fixtures::tags(crate::rules::tag::fixtures::gateway(), &tags, "[]")
}

/// A project with a view `Main` whose root has one script.
pub fn view(kind: ViewScript, code: &str) -> Fixture {
    Fixture::new().with_view("Main", &view_json(kind, code))
}

pub fn library_path(f: &Fixture, path: &str) -> PathBuf {
    f.path(&format!("projects/p/ignition/script-python/{path}/code.py"))
}

pub fn view_path(f: &Fixture) -> PathBuf {
    f.path(&format!("projects/p/{PERSPECTIVE}/views/Main/view.json"))
}

/// Lints a library module `m` holding `code` with `rule` (default options).
pub fn lint_module(rule: &str, code: &str) -> Vec<Diagnostic> {
    library("m", code).lint(&[rule])
}

/// `(line, flagged text)` of each finding, 1-based lines in the file.
pub fn lines(diagnostics: &[Diagnostic]) -> Vec<(usize, String)> {
    diagnostics
        .iter()
        .map(|d| {
            let text = d.file.text();
            let line = text[..d.range.start().to_usize()].matches('\n').count() + 1;
            (line, d.range.slice(text).to_owned())
        })
        .collect()
}

/// The flagged texts.
pub fn flagged(diagnostics: &[Diagnostic]) -> Vec<String> {
    lines(diagnostics).into_iter().map(|(_, t)| t).collect()
}

/// The messages.
pub fn messages(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

/// Lints `code` as each kind of view script and as a library function
/// body, returning the flagged texts per location (they must agree).
pub fn flagged_everywhere(rule: &str, body: &str) -> Vec<String> {
    let mut results: Vec<(String, Vec<String>)> = Vec::new();
    for kind in [
        ViewScript::Action,
        ViewScript::Transform,
        ViewScript::Change,
        ViewScript::Handler,
        ViewScript::Method,
    ] {
        let f = view(kind, body);
        let d = f.lint(&[rule]);
        results.push((format!("{kind:?}"), decoded_flagged(&d)));
    }
    let indented: String = body.lines().map(|l| format!("{l}\n")).collect();
    let f = library("m", &format!("def f(self, event, value):\n{indented}"));
    results.push(("library".into(), flagged(&f.lint(&[rule]))));
    let first = results[0].1.clone();
    for (where_, r) in &results {
        assert_eq!(*r, first, "{where_} differs from {}", results[0].0);
    }
    first
}

/// Flagged texts, JSON-decoded when they come from a JSON file.
pub fn decoded_flagged(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| {
            let raw = d.range.slice(d.file.text());
            if d.file.path().extension().is_some_and(|e| e == "json") {
                let quoted = format!("\"{raw}\"");
                match iglint_json::parse(&quoted) {
                    Ok(v) => v
                        .as_json_string()
                        .map(|s| s.value().to_string())
                        .unwrap_or_else(|| raw.to_owned()),
                    Err(_) => raw.to_owned(),
                }
            } else {
                raw.to_owned()
            }
        })
        .collect()
}

/// The text of `rel` after `--fix` (safe fixes only) with `rules`, or the
/// original text when nothing changes.
pub fn safe_fixed(f: &Fixture, rules: &[&str], rel: &std::path::Path) -> String {
    let rules: Vec<(&str, &str)> = rules.iter().map(|r| (*r, "")).collect();
    let result = f.run(
        &rules,
        crate::engine::FixMode::DiffOnly {
            unsafe_fixes: false,
        },
    );
    match result.fixed.iter().find(|s| s.path == rel) {
        Some(s) => s.fixed.clone(),
        None => std::fs::read_to_string(rel).expect("read"),
    }
}

/// Whether `--fix` (safe fixes only) changes library module `module`.
pub fn safe_fix_applies(f: &Fixture, rule: &str, module: &str) -> bool {
    let path = library_path(f, module);
    f.run(
        &[(rule, "")],
        crate::FixMode::DiffOnly {
            unsafe_fixes: false,
        },
    )
    .fixed
    .iter()
    .any(|s| s.path == path && s.changed())
}
