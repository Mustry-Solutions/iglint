//! Stress tests: generated inputs at and beyond the depth limits, through
//! the real binary (`check`, `ign-lint` and the language server).
//!
//! Every input must either be linted normally or produce a "nested too
//! deeply" finding, never crash the process. The limits are explained in
//! `iglint_python` (`MAX_DEPTH`, the expression nesting limit),
//! `iglint_expression::MAX_DEPTH` and `iglint_project::MAX_JSON_DEPTH`
//! (JSON of any depth is parsed; deeper than that is a `VIEW001` finding).
//!
//! The inputs are large, so these tests run in release mode only:
//!
//! ```sh
//! cargo test --release -p iglint --test stress
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Chain length that stays within `iglint_python::MAX_DEPTH`.
const CHAIN: usize = 16_000;
/// Nesting that stays within the expression nesting limit (2,000).
const NESTING: usize = 1_990;

fn iglint(dir: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_iglint"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "iglint {args:?} failed with {:?}: {}",
        output.status,
        &stderr[..stderr.len().min(2000)]
    );
    assert!(!stderr.contains("overflow"), "{stderr}");
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const MANIFEST: &str = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": [], "attributes": {}}"#;
const MODULE: &str = "projects/p/ignition/script-python/m/code.py";
const VIEW: &str = "projects/p/com.inductiveautomation.perspective/views/V/view.json";

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    dir
}

/// A project whose script module is `source`.
fn module(source: &str) -> tempfile::TempDir {
    let dir = project();
    write(dir.path(), MODULE, source);
    write(
        dir.path(),
        "projects/p/ignition/script-python/m/resource.json",
        MANIFEST,
    );
    dir
}

/// A project with one view whose JSON is `json`.
fn view_json(json: &str) -> tempfile::TempDir {
    let dir = project();
    write(dir.path(), VIEW, json);
    write(
        dir.path(),
        "projects/p/com.inductiveautomation.perspective/views/V/resource.json",
        MANIFEST,
    );
    dir
}

/// A view that runs `source` as an event script, a script transform, a
/// message handler and a custom method.
fn view_script(source: &str) -> tempfile::TempDir {
    let body: String = source
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| format!("\t{l}\n"))
        .collect();
    let view = json!({
        "custom": {"c": 1},
        "params": {},
        "props": {},
        "root": {
            "type": "ia.container.flex",
            "meta": {"name": "root"},
            "scripts": {
                "customMethods": [{"name": "m", "params": [], "script": body}],
                "messageHandlers": [{
                    "messageType": "t",
                    "pageScope": true,
                    "script": body,
                    "sessionScope": false,
                    "viewScope": false
                }]
            },
            "children": [{
                "type": "ia.input.button",
                "meta": {"name": "Button"},
                "events": {"component": {"onActionPerformed": {
                    "type": "script", "scope": "G", "config": {"script": body}
                }}},
                "propConfig": {"props.text": {"binding": {
                    "type": "property",
                    "config": {"path": "view.custom.c"},
                    "transforms": [{"type": "script", "code": body}]
                }}}
            }]
        }
    });
    view_json(&view.to_string())
}

/// A view that evaluates `expression` in an expression binding and an
/// expression transform.
fn view_expression(expression: &str) -> tempfile::TempDir {
    let view = json!({
        "custom": {"c": 1},
        "params": {},
        "props": {},
        "root": {
            "type": "ia.container.flex",
            "meta": {"name": "root"},
            "children": [{
                "type": "ia.display.label",
                "meta": {"name": "Label"},
                "propConfig": {
                    "props.text": {"binding": {
                        "type": "expr", "config": {"expression": expression}
                    }},
                    "props.style.color": {"binding": {
                        "type": "property",
                        "config": {"path": "view.custom.c"},
                        "transforms": [{"type": "expression", "expression": expression}]
                    }}
                }
            }]
        }
    });
    view_json(&view.to_string())
}

/// The inputs from the review that used to abort the process.
fn too_deep_python() -> Vec<(&'static str, String)> {
    vec![
        (
            "elif",
            "if a: pass\n".to_owned() + &"elif a: pass\n".repeat(30_000),
        ),
        ("subscript", format!("x = a{}\n", "[0]".repeat(50_000))),
        ("attribute", format!("x = a{}\n", ".b".repeat(100_000))),
        ("unary", format!("x = {}1\n", "-".repeat(100_000))),
        ("binary", format!("x = {}\n", vec!["1"; 200_000].join("+"))),
        ("call", format!("x = a{}\n", "()".repeat(50_000))),
        (
            "parentheses",
            format!("x = {}1{}\n", "(".repeat(2_010), ")".repeat(2_010)),
        ),
        ("power", format!("x = {}\n", vec!["a"; 2_010].join("**"))),
    ]
}

/// Inputs within the limits (Ignition itself fails on most of them, but
/// they must be linted like any other code).
fn deep_python() -> Vec<(&'static str, String)> {
    let n = NESTING;
    vec![
        (
            "elif",
            "a = 1\nif a: pass\n".to_owned()
                + &"elif a:\n    b = [a.b(c=a) for c in a if not c]\n".repeat(CHAIN),
        ),
        (
            "subscript",
            format!("a = {{}}\nx = (a{})\n", "\n[0]".repeat(CHAIN)),
        ),
        (
            "attribute",
            format!("import system\nx = (system{})\n", "\n.b".repeat(CHAIN)),
        ),
        (
            "call",
            format!("a = len\nx = (a{})\n", "\n()".repeat(CHAIN)),
        ),
        (
            "binary",
            format!("a = 1\nx = ({})\n", vec!["a"; CHAIN].join(" +\n")),
        ),
        ("unary", format!("a = 1\nx = {}a\n", "-".repeat(CHAIN))),
        ("not", format!("a = 1\nx = {}a\n", "not ".repeat(CHAIN))),
        (
            "parentheses",
            format!("a = 1\nx = {}a{}\n", "(".repeat(n), ")".repeat(n)),
        ),
        ("list", format!("x = {}{}\n", "[".repeat(n), "]".repeat(n))),
        (
            "nested call",
            format!(
                "def f(*a, **k): return a\nx = {}1{}\n",
                "f(k=".repeat(n),
                ")".repeat(n)
            ),
        ),
        ("lambda", format!("x = {}1\n", "lambda: ".repeat(n))),
        ("power", format!("a = 2\nx = {}\n", vec!["a"; n].join("**"))),
        (
            "conditional",
            format!("a = 1\nx = ({} a)\n", vec!["a if a else"; n].join("\n")),
        ),
        (
            "dict",
            format!("x = {}1{}\n", "{1: ".repeat(n), "}".repeat(n)),
        ),
        (
            "comprehension",
            format!(
                "a = []\nx = {}a{}\n",
                "[c for c in ".repeat(n),
                "]".repeat(n)
            ),
        ),
    ]
}

const ALL: &[&str] = &["--no-cache", "--select", "ALL", "--ignore", "SCR900"];

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn too_deep_scripts_are_findings() {
    for (name, source) in too_deep_python() {
        for dir in [module(&source), view_script(&source)] {
            for args in [
                &["--no-cache"][..],
                ALL,
                &["--no-cache", "--select", "VIEW001"],
                &["--no-cache", "--output-format", "json"],
            ] {
                let output = iglint(dir.path(), args);
                let shown = stdout(&output);
                if args.contains(&"VIEW001") {
                    continue;
                }
                assert!(
                    shown.contains("nested too deeply"),
                    "{name} {args:?}: {shown}"
                );
            }
        }
    }
}

/// A deep chain is one finding, not one per level (reviewer note R6-D).
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn deep_chains_do_not_flood() {
    let mut inputs = too_deep_python();
    inputs.extend(deep_python());
    inputs.extend([
        ("not 200000", format!("x = {}1\n", "not ".repeat(200_000))),
        ("unary target", format!("({}a) = 1\n", "-".repeat(CHAIN))),
        ("not target", format!("({}a) = 1\n", "not ".repeat(CHAIN))),
        (
            "for target",
            format!("for ({}a) in x:\n    pass\n", "~".repeat(CHAIN)),
        ),
    ]);
    for (name, source) in inputs {
        // The view holds the body in four scripts.
        for (dir, scripts) in [(module(&source), 1), (view_script(&source), 4)] {
            let args = [
                "--no-cache",
                "--select",
                "SCR001,SCR002",
                "--output-format",
                "concise",
            ];
            let shown = stdout(&iglint(dir.path(), &args));
            let findings = shown.lines().filter(|l| l.contains(": SCR00")).count();
            // Distinct problems of one script (too deep, too long a line)
            // are separate findings; a flood is one per level.
            assert!(findings <= 3 * scripts, "{name}: {findings} findings");
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn deep_scripts_are_linted() {
    for (name, source) in deep_python() {
        for dir in [module(&source), view_script(&source)] {
            for args in [ALL, &["--no-cache", "--unsafe-fixes", "--diff"][..]] {
                let output = iglint(dir.path(), args);
                let shown = stdout(&output);
                assert!(!shown.contains("too deeply"), "{name} {args:?}: {shown}");
                assert!(!shown.contains("SCR001"), "{name} {args:?}: {shown}");
            }
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn deep_expressions() {
    let value = "{view.custom.c}";
    let within = [
        format!("{value}{}", " + 1".repeat(990)),
        format!("{}{value}{}", "(".repeat(990), ")".repeat(990)),
        format!("{}{value}{}", "if(true, ".repeat(990), ", 0)".repeat(990)),
        format!("{}{value}", "-".repeat(990)),
        format!("{value}{}", "[0]".repeat(990)),
    ];
    for expression in within {
        let dir = view_expression(&expression);
        let shown = stdout(&iglint(dir.path(), ALL));
        assert!(!shown.contains("too deeply"), "{shown}");
    }
    let beyond = [
        format!("{value}{}", " + 1".repeat(200_000)),
        format!("{}{value}{}", "(".repeat(200_000), ")".repeat(200_000)),
        format!(
            "{}{value}{}",
            "if(true, ".repeat(100_000),
            ", 0)".repeat(100_000)
        ),
        format!("{}{value}", "!".repeat(200_000)),
        format!("{value}{}", "[0]".repeat(100_000)),
    ];
    for expression in beyond {
        let dir = view_expression(&expression);
        let shown = stdout(&iglint(dir.path(), ALL));
        assert!(shown.contains("nested too deeply"), "{shown}");
    }
}

/// JSON levels iglint analyses (`iglint_project::MAX_JSON_DEPTH`).
const JSON_DEPTH: usize = 8_192;

/// Views nested `n` levels in different places, the JSON depth each
/// reaches, as (name, view.json, JSON depth).
fn deep_views(n: usize) -> Vec<(&'static str, String, usize)> {
    let array = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
    let object = |n: usize| format!("{}1{}", r#"{"a": "#.repeat(n), "}".repeat(n));
    let components = |n: usize| {
        let mut open = String::new();
        for i in 0..n {
            open.push_str(&format!(
                r#"{{"type": "ia.container.flex", "meta": {{"name": "C{i}"}}, "children": ["#
            ));
        }
        format!(
            r#"{open}{{"type": "ia.display.label", "meta": {{"name": "L"}}, "props": {{"text": "x"}}}}{}"#,
            "]}".repeat(n)
        )
    };
    let view = |custom: &str, root: &str| {
        format!(r#"{{"custom": {{"c": {custom}}}, "params": {{}}, "props": {{}}, "root": {root}}}"#)
    };
    let flex = r#"{"type": "ia.container.flex", "meta": {"name": "root"}}"#;
    let props = |value: &str| {
        format!(
            r#"{{"type": "ia.container.flex", "meta": {{"name": "root"}}, "props": {{"style": {{"x": {value}}}}}, "custom": {{"c": {value}}}}}"#
        )
    };
    let binding = |value: &str| {
        format!(
            r#"{{"type": "ia.container.flex", "meta": {{"name": "root"}}, "propConfig": {{"props.x": {{"binding": {{"type": "property", "config": {{"path": "view.custom.c"}}, "transforms": [{{"type": "map", "inputType": "scalar", "outputType": "scalar", "mappings": [{{"input": {value}, "output": {value}}}], "fallback": {value}}}]}}}}}}}}"#
        )
    };
    vec![
        ("custom array", view(&array(n), flex), n + 2),
        ("custom object", view(&object(n), flex), n + 2),
        ("props", view("1", &props(&object(n))), n + 5),
        ("binding", view("1", &binding(&array(n))), n + 11),
        ("components", view("1", &components(n / 2)), n + 3),
    ]
}

/// A rule configuration that makes `iglint ign-lint` build its node model
/// and run rules that walk properties.
const RULE_CONFIG: &str = r#"{"UnusedCustomPropertiesRule": {"enabled": true}, "PylintScriptRule": {"enabled": true}, "NamePatternRule": {"enabled": true}, "ExcessiveContextDataRule": {"enabled": true}, "PropertyAccessRule": {"enabled": true}}"#;

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn deep_json() {
    // Gson reads any depth: iglint lints views up to its limit, in check
    // and ign-lint mode (ign-lint itself crashes from about 500 levels).
    for n in [600, JSON_DEPTH - 16] {
        for (name, json, depth) in deep_views(n) {
            assert!(depth <= JSON_DEPTH, "{name}");
            let dir = view_json(&json);
            let shown = stdout(&iglint(dir.path(), ALL));
            assert!(!shown.contains("VIEW001"), "{name} {n}: {shown}");
            assert!(!shown.contains("too deep"), "{name} {n}: {shown}");
            write(dir.path(), "rule_config.json", RULE_CONFIG);
            let output = iglint(dir.path(), &["ign-lint", "--files", "**/view.json"]);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!stderr.contains("Invalid JSON"), "{name} {n}: {stderr}");
            assert!(stdout(&output).contains("Files processed: 1"), "{name} {n}");
        }
    }
    // Deeper is one VIEW001 finding (Perspective cannot open such a view),
    // and invalid JSON in ign-lint mode.
    for n in [JSON_DEPTH + 16, 200_000] {
        for (name, json, _) in deep_views(n) {
            let dir = view_json(&json);
            let shown = stdout(&iglint(dir.path(), ALL));
            assert_eq!(
                shown.matches("nesting too deep").count(),
                1,
                "{name} {n}: {shown}"
            );
            write(dir.path(), "rule_config.json", RULE_CONFIG);
            let output = iglint(dir.path(), &["ign-lint", "--files", "**/view.json"]);
            assert_eq!(output.status.code(), Some(1));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("Invalid JSON"), "{name} {n}: {stderr}");
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn deep_tags() {
    for (n, too_deep) in [(JSON_DEPTH / 2 - 8, false), (100_000, true)] {
        let mut tags = r#"{"name": "T", "tagType": "AtomicTag", "valueSource": "expr", "expression": "now()"}"#
            .to_owned();
        for i in 0..n {
            tags = format!(r#"{{"name": "F{i}", "tagType": "Folder", "tags": [{tags}]}}"#);
        }
        let dir = project();
        write(
            dir.path(),
            "gw/core/config-mode.json",
            r#"{"title": "core"}"#,
        );
        let provider = "gw/core/ignition/tag-provider/default";
        write(
            dir.path(),
            &format!("{provider}/config.json"),
            r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
        );
        write(
            dir.path(),
            &format!("{provider}/resource.json"),
            &MANIFEST.replace("[]", r#"["config.json"]"#),
        );
        let folder = "gw/core/ignition/tag-definition/default";
        write(
            dir.path(),
            &format!("{folder}/tags.json"),
            &format!(r#"{{"tags": [{tags}]}}"#),
        );
        write(
            dir.path(),
            &format!("{folder}/unary-resource.json"),
            &MANIFEST.replace("[]", r#"["tags.json"]"#),
        );
        let shown = stdout(&iglint(dir.path(), ALL));
        assert_eq!(shown.contains("nesting too deep"), too_deep, "{n}: {shown}");
    }
}

/// Scripts whose first two lines Jython cannot read (the 100,000
/// character `Mark invalid` limit), and long lines it can.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn long_lines() {
    // How many scripts Jython cannot read.
    let mark = |source: &str, expected: usize, view: bool| {
        let dir = if view {
            view_script(source)
        } else {
            module(source)
        };
        let shown = stdout(&iglint(dir.path(), ALL));
        assert_eq!(
            shown.matches("Mark invalid").count(),
            expected,
            "{}: {}",
            &source[..40.min(source.len())],
            &shown[..shown.len().min(2000)]
        );
    };
    // A module: line 1 plus line 2 (`x = 1`, 5) and line 1's newline.
    let comment = |n: usize| format!("x = 1 # {}\nx = 1\n", "a".repeat(n));
    mark(&comment(99_985), 0, false);
    mark(&comment(99_986), 1, false);
    let list = |n: usize| format!("x = [{}1]\n", "1, ".repeat(n));
    mark(&list(33_330), 0, false);
    mark(&list(33_331), 1, false);
    // Lines from 3 on have no limit.
    let third = |text: &str| format!("x = 1\ny = 2\nz = u'{text}'\n");
    mark(&third(&"\u{e9}a".repeat(200_000)), 0, false);
    // View scripts: the generated header is line 1, so the limit for the
    // first body line (`\tx = 1 # ...`, 9 + n) depends on the header:
    // `def m(self):` (12), `def runAction(self, event):` (27),
    // `def onMessageReceived(self, payload):` (37) and
    // `def transform(self, value, quality, timestamp):` (47) fail from
    // n = 99,990 - header.
    let body = |n: usize| format!("x = 1 # {}\npass\n", "b".repeat(n));
    mark(&body(99_942), 0, true);
    mark(&body(99_943), 1, true);
    mark(&body(99_962), 2, true);
    mark(&body(99_963), 3, true);
    mark(&body(99_978), 4, true);
    mark(&third(&"a".repeat(500_000)), 0, true);
    // A body too large for one Java method.
    let dir = module(&format!("def f():\n{}", "    pass\n".repeat(11_000)));
    let shown = stdout(&iglint(dir.path(), ALL));
    assert!(shown.contains("Module or method too large"), "{shown}");
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn ign_lint_mode_survives_deep_scripts() {
    for (_, source) in too_deep_python().into_iter().chain(deep_python()) {
        let dir = view_script(&source);
        write(
            dir.path(),
            "rule_config.json",
            r#"{"UnusedCustomPropertiesRule": {"enabled": true}, "PylintScriptRule": {"enabled": true}, "PropertyAccessRule": {"enabled": true}}"#,
        );
        iglint(dir.path(), &["ign-lint", "--files", "**/view.json"]);
    }
}

/// A language server process and the messages it sent.
struct Server {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    messages: mpsc::Receiver<Value>,
    next_id: i64,
    open: std::collections::HashSet<String>,
}

impl Server {
    fn start(root: &Path) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_iglint"))
            .arg("server")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(n) = line.strip_prefix("Content-Length: ") {
                        length = n.parse::<usize>().ok();
                    }
                }
                let mut body = vec![0; length.unwrap()];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if send.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut server = Server {
            child,
            stdin,
            messages,
            next_id: 0,
            open: std::collections::HashSet::new(),
        };
        let uri = file_uri(root);
        server.request(
            "initialize",
            json!({"capabilities": {}, "workspaceFolders": [{"uri": uri, "name": "stress"}]}),
        );
        server.notify("initialized", json!({}));
        server
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.wait(|m| (m["id"] == id).then(|| m["result"].clone()))
    }

    fn wait<T>(&mut self, mut pick: impl FnMut(&Value) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = self
                .messages
                .recv_timeout(left)
                .expect("the server answers (and is alive)");
            if let Some(found) = pick(&message) {
                return found;
            }
        }
    }

    /// Sets the text of `path` (opening it the first time) and returns
    /// the diagnostics published for that version.
    fn edit(&mut self, path: &Path, text: &str) -> Vec<Value> {
        self.next_id += 1;
        let version = self.next_id;
        let uri = file_uri(path);
        if self.open.insert(uri.clone()) {
            let language = if path.extension().is_some_and(|e| e == "py") {
                "python"
            } else {
                "json"
            };
            self.notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": uri, "languageId": language, "version": version, "text": text}}),
            );
        } else {
            self.notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
            );
        }
        self.wait(|m| {
            let params = &m["params"];
            (m["method"] == "textDocument/publishDiagnostics"
                && params["uri"] == uri
                && params["version"] == version)
                .then(|| {
                    params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
        })
    }

    fn stop(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        if let Some(mut err) = self.child.stderr.take() {
            let _ = err.read_to_string(&mut stderr);
        }
        assert!(status.success(), "{status:?}: {stderr}");
    }
}

fn file_uri(path: &Path) -> String {
    let path = path.canonicalize().unwrap();
    let text = path.to_string_lossy().replace('\\', "/");
    // Windows: `\\?\C:\...`.
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn language_server_survives_deep_documents() {
    let dir = module("x = 1\n");
    write(
        dir.path(),
        VIEW,
        r#"{"custom": {}, "params": {}, "props": {}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#,
    );
    write(
        dir.path(),
        "projects/p/com.inductiveautomation.perspective/views/V/resource.json",
        MANIFEST,
    );
    let mut server = Server::start(dir.path());
    let module_path = dir.path().join(MODULE);
    let view_path = dir.path().join(VIEW);
    for (name, source) in too_deep_python() {
        let diagnostics = server.edit(&module_path, &source);
        assert!(
            diagnostics.iter().any(|d| d["message"]
                .as_str()
                .unwrap_or("")
                .contains("nested too deeply")),
            "{name}: {diagnostics:?}"
        );
    }
    for (name, source) in deep_python() {
        let diagnostics = server.edit(&module_path, &source);
        assert!(
            !diagnostics
                .iter()
                .any(|d| d["message"].as_str().unwrap_or("").contains("too deeply")),
            "{name}: {diagnostics:?}"
        );
        let view = std::fs::read_to_string(view_script(&source).path().join(VIEW)).unwrap();
        server.edit(&view_path, &view);
    }
    let deep_view = std::fs::read_to_string(
        view_expression(&format!("{{view.custom.c}}{}", " + 1".repeat(200_000)))
            .path()
            .join(VIEW),
    )
    .unwrap();
    let diagnostics = server.edit(&view_path, &deep_view);
    assert!(
        diagnostics.iter().any(|d| d["message"]
            .as_str()
            .unwrap_or("")
            .contains("nested too deeply")),
        "{diagnostics:?}"
    );
    for (n, too_deep) in [(JSON_DEPTH - 16, false), (200_000, true)] {
        for (name, json, _) in deep_views(n) {
            let diagnostics = server.edit(&view_path, &json);
            let found = diagnostics.iter().any(|d| {
                d["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("nesting too deep")
            });
            assert_eq!(found, too_deep, "{name} {n}: {diagnostics:?}");
        }
    }
    server.stop();
}

/// A view with `n` labels in the root container, each with a binding, an
/// event script and a name that `NAME001` would rename.
fn wide_view(n: usize) -> String {
    let components: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"type": "ia.display.label", "meta": {{"name": "Label_{i}"}}, "position": {{"x": {x}, "y": {y}}}, "props": {{"text": "L{i}"}}, "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": "{{view.custom.v{m}}} + 1"}}}}}}}}, "events": {{"dom": {{"onClick": {{"type": "script", "scope": "G", "config": {{"script": "\tself.props.text = 'x{i}'"}}}}}}}}}}"#,
                x = i % 100,
                y = i / 100,
                m = i % 50
            )
        })
        .collect();
    let custom: Vec<String> = (0..50).map(|i| format!(r#""v{i}": {i}"#)).collect();
    format!(
        r#"{{"custom": {{{}}}, "params": {{}}, "props": {{}}, "root": {{"type": "ia.container.coord", "meta": {{"name": "root"}}, "children": [{}]}}}}"#,
        custom.join(", "),
        components.join(", ")
    )
}

/// The best of two wall times of `iglint check` on `dir`.
fn check_time(dir: &Path, args: &[&str]) -> Duration {
    (0..2)
        .map(|_| {
            let start = Instant::now();
            iglint(dir, args);
            start.elapsed()
        })
        .min()
        .unwrap()
}

/// Review round 14: a view of 80,000 components took 11.5 s, 46 times as
/// long as one of 10,000 (`propConfig` entries looked up per component,
/// sibling names collected per component, every string of the view
/// searched for every renamed component). Time must grow about linearly:
/// four times the components may take at most six times as long (a
/// quadratic part takes sixteen), within a generous budget.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn large_views_and_modules_take_linear_time() {
    let args = [
        "--no-cache",
        "--select",
        "ALL",
        "--ignore",
        "SCR900",
        "--exit-zero",
    ];
    let (small, large) = (view_json(&wide_view(20_000)), view_json(&wide_view(80_000)));
    let (t_small, t_large) = (
        check_time(small.path(), &args),
        check_time(large.path(), &args),
    );
    eprintln!("views: 20,000 components {t_small:?}, 80,000 {t_large:?}");
    assert!(
        t_large < t_small * 6 + Duration::from_millis(300),
        "20,000 components: {t_small:?}, 80,000: {t_large:?}"
    );
    assert!(t_large < Duration::from_secs(30), "{t_large:?}");
    let (small, large) = (
        module(&"x = 1\n".repeat(250_000)),
        module(&"x = 1\n".repeat(1_000_000)),
    );
    let (t_small, t_large) = (
        check_time(small.path(), &args),
        check_time(large.path(), &args),
    );
    eprintln!("modules: 250,000 lines {t_small:?}, 1,000,000 {t_large:?}");
    assert!(
        t_large < t_small * 6 + Duration::from_millis(300),
        "250,000 lines: {t_small:?}, 1,000,000: {t_large:?}"
    );
    assert!(t_large < Duration::from_secs(60), "{t_large:?}");
}
