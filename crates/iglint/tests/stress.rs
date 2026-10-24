//! Stress tests: generated inputs at and beyond the depth limits, through
//! the real binary (`check`, `ign-lint` and the language server).
//!
//! Every input must either be linted normally or produce a "nested too
//! deeply" finding, never crash the process. The limits are explained in
//! `iglint_python` (`MAX_DEPTH`, the expression nesting limit),
//! `iglint_expression::MAX_DEPTH` and `iglint_project::MAX_JSON_DEPTH`
//! (JSON of any depth is parsed; deeper than that is a `VIEW001` finding).
//!
//! Generated workspaces with multi-byte characters in every name and
//! string position must not crash any command either.
//!
//! The deep inputs are large, so those tests run in release mode only:
//!
//! ```sh
//! cargo test --release -p iglint --test stress
//! ```

use std::collections::HashMap;
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
        stderr.chars().take(2000).collect::<String>()
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
/// Reviewer round 19 (Codex) major: nested `try`/`finally` chains made
/// the size bounds overflow (a debug panic; in release a wrapped bound
/// that let the fix of a 64-level chain through as safe). In a release
/// build too, no fix that changes the tokens of such a script is safe and
/// nothing panics; since review round 24 one that leaves every token where
/// it was (trailing whitespace) compiles to the same code and is safe.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn nested_finally_chains_are_never_safe() {
    for depth in [50, 64, 200] {
        let mut source = String::from("def g():\n    import math\n    return 1\ndef f():\n");
        for d in 0..depth {
            source.push_str(&format!("{}try:\n", "    ".repeat(d + 1)));
        }
        source.push_str(&format!("{}return 1\n", "    ".repeat(depth + 1)));
        for d in (0..depth).rev() {
            let pad = "    ".repeat(d + 1);
            source.push_str(&format!("{pad}finally:\n{pad}    return 2\n"));
        }
        source.push_str("x = 1  \n");
        let dir = module(&source);
        let output = iglint(
            dir.path(),
            &[
                "--no-cache",
                "--select",
                "SCR102,SCR124,SCR002,SCR001",
                "--output-format",
                "json",
            ],
        );
        let shown = stdout(&output);
        let findings: Value = serde_json::from_str(&shown).unwrap();
        for finding in findings.as_array().unwrap() {
            let safe = finding["fix"]["applicability"] == "safe";
            assert_eq!(safe, finding["code"] == "SCR124", "{depth}: {finding}");
        }
        if depth < 100 {
            assert!(
                shown.contains("compile status is not known"),
                "{depth}: {shown}"
            );
        }
    }
}

/// Review round 24, item 2: when a script's compile status is not known,
/// the net used to reject the combined safe fixes and then check each
/// alone, quadratic in the size of the file (1,400 lines with trailing
/// whitespace took 3.2 s, 12,000 lines 95 s, 16,000 more than 120 s).
/// Edits that leave every token where it was are now proven neutral by
/// lexing, fixes that may change a token of such a script are dropped
/// without evaluating the net, and the rest is bisected. `check` and
/// `--fix` stay within a budget on the reviewer's shapes and on a module
/// whose every fix fails.
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "large inputs: run with `cargo test --release -p iglint --test stress`"
)]
fn unknown_compile_status_is_not_quadratic() {
    const BUDGET: Duration = Duration::from_secs(20);
    // 1,400 statements make the module's size bounds lie on both sides of
    // Jython's method limit; comment lines pad it to `lines`. Every line
    // ends in whitespace, and `g` has an unused import whose removal
    // changes tokens.
    let shape = |lines: usize| {
        let mut source = String::from("def g():\n    import math\n    return 1\n");
        for i in 0..lines {
            if i < 1_400 {
                source.push_str(&format!("x{i} = ({i}, {i})   \n"));
            } else {
                source.push_str(&format!("# note {i}   \n"));
            }
        }
        source
    };
    for lines in [1_400, 12_000, 16_000] {
        let dir = module(&shape(lines));
        let args = [
            "--no-cache",
            "--select",
            "SCR102,SCR124",
            "--output-format",
            "json",
            "--exit-zero",
        ];
        let start = Instant::now();
        let findings: Value = serde_json::from_str(&stdout(&iglint(dir.path(), &args))).unwrap();
        let checked = start.elapsed();
        let findings = findings.as_array().unwrap();
        assert_eq!(findings.len(), lines + 1, "{lines}");
        for finding in findings {
            let safe = finding["fix"]["applicability"] == "safe";
            assert_eq!(safe, finding["code"] == "SCR124", "{lines}: {finding}");
            if !safe {
                let reason = finding["fix"]["unsafe_reason"]["message"].as_str().unwrap();
                assert!(reason.contains("compile status is not known"), "{reason}");
            }
        }
        let start = Instant::now();
        let output = iglint(
            dir.path(),
            &[
                "--no-cache",
                "--select",
                "SCR102,SCR124",
                "--fix",
                "--exit-zero",
            ],
        );
        let fixed = start.elapsed();
        assert!(output.status.success(), "{lines}");
        let text = std::fs::read_to_string(dir.path().join(MODULE)).unwrap();
        assert!(!text.lines().any(|l| l.ends_with(' ')), "{lines}");
        assert!(text.contains("import math"), "{lines}");
        assert!(
            checked < BUDGET && fixed < BUDGET,
            "{lines} lines: check {checked:?}, --fix {fixed:?}"
        );
    }
    // Every safe fix fails on its own: 2,000 functions with an unused
    // import each, in a module whose status is not known.
    let source: String = (0..2_000)
        .map(|i| format!("def f{i}():\n    import math\n    return {i}\n"))
        .collect();
    let dir = module(&source);
    for args in [
        &[
            "--no-cache",
            "--select",
            "SCR102",
            "--output-format",
            "json",
            "--exit-zero",
        ][..],
        &["--no-cache", "--select", "SCR102", "--fix", "--exit-zero"][..],
    ] {
        let start = Instant::now();
        let output = iglint(dir.path(), args);
        let took = start.elapsed();
        assert!(output.status.success(), "{args:?}");
        assert!(took < BUDGET, "{args:?}: {took:?}");
    }
    let shown = stdout(&iglint(
        dir.path(),
        &[
            "--no-cache",
            "--select",
            "SCR102",
            "--output-format",
            "json",
            "--exit-zero",
        ],
    ));
    assert_eq!(shown.matches("compile status is not known").count(), 2_000);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(MODULE)).unwrap(),
        source
    );
}

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
            source.chars().take(40).collect::<String>(),
            shown.chars().take(2000).collect::<String>()
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
            // The server survives a panicking handler (it logs the panic
            // and answers with an internal error); that is still a bug.
            let logged = message["method"] == "window/logMessage"
                || message["method"] == "window/showMessage";
            assert!(
                !(logged
                    && message["params"]["message"]
                        .as_str()
                        .is_some_and(|m| m.contains(" failed: "))),
                "{message}"
            );
            assert_ne!(message["error"]["code"], -32603, "{message}");
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
                && params["uri"].as_str().map(percent_decoded) == Some(percent_decoded(&uri))
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
        assert!(!stderr.contains("panicked"), "{stderr}");
    }
}

/// `uri` with its percent escapes decoded, to compare URIs that encode
/// different characters.
fn percent_decoded(uri: &str) -> Vec<u8> {
    let bytes = uri.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

fn file_uri(path: &Path) -> String {
    let path = path.canonicalize().unwrap();
    let raw = path.to_string_lossy().replace('\\', "/");
    // Windows: `\\?\C:\...`.
    let raw = raw.strip_prefix("//?/").unwrap_or(&raw);
    let mut text = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            text.push(char::from(byte));
        } else {
            text.push_str(&format!("%{byte:02X}"));
        }
    }
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

/// A change of a file's text.
type Change = dyn Fn(&str) -> String;

/// Copies the public fixture projects (`iglint_project`'s fixtures, and
/// the ign-lint parity views of at most 64 KiB as views of a project)
/// into `to`, every text file changed by `change`.
fn multi_byte_fixture(to: &Path, change: &Change) {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut copied = 0;
    let mut copy = |from: &Path, rel: &Path| {
        let target = to.join(rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        match std::fs::read_to_string(from) {
            Ok(text) => std::fs::write(&target, change(&text)).unwrap(),
            Err(_) => {
                std::fs::copy(from, &target).unwrap();
            }
        }
        copied += 1;
    };
    let mut stack = vec![crates.join("iglint_project/tests/fixtures")];
    let base = stack[0].clone();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                copy(&path, path.strip_prefix(&base).unwrap());
            }
        }
    }
    let cases = crates.join("iglint_linter/tests/fixtures/ign-lint/cases");
    for entry in std::fs::read_dir(&cases).unwrap().flatten() {
        let view = entry.path().join("view.json");
        if std::fs::metadata(&view).is_ok_and(|m| m.len() <= 64 * 1024) {
            let name = entry.file_name();
            let dir = Path::new("cases/projects/cases/com.inductiveautomation.perspective/views")
                .join(&name);
            copy(&view, &dir.join("view.json"));
            let manifest = entry.path().join("resource.json");
            if manifest.is_file() {
                copy(&manifest, &dir.join("resource.json"));
            }
        }
    }
    write(
        to,
        "cases/projects/cases/project.json",
        &change(r#"{"title": "cases", "enabled": true, "inheritable": false}"#),
    );
    assert!(copied > 100, "{copied}");
}

/// Review round 17 (Opus): a file whose first character is longer than
/// a byte (a byte order mark, `€`, an emoji) made rules that point at
/// "the first byte" panic. Every rule (`--select ALL`, with fixes applied),
/// the ign-lint mode and the language server must survive files that start
/// with such a character and files that are only one. Debug builds also
/// assert that no rule reports a range that splits a character.
#[test]
fn multi_byte_first_characters_do_not_panic() {
    let changes: [(&str, &Change); 5] = [
        ("byte order mark first", &|t: &str| format!("\u{feff}{t}")),
        ("euro sign first", &|t: &str| format!("\u{20ac}{t}")),
        ("emoji first", &|t: &str| format!("\u{1f600}{t}")),
        ("only a byte order mark", &|_: &str| "\u{feff}".to_owned()),
        ("only an emoji", &|_: &str| "\u{1f600}".to_owned()),
    ];
    let ign_lint_rules: serde_json::Map<String, Value> = [
        "BadComponentReferenceRule",
        "ComponentReferenceValidationRule",
        "ExcessiveContextDataRule",
        "NamePatternRule",
        "PollingIntervalRule",
        "PropertyAccessRule",
        "PropertyPersistenceRule",
        "UnusedCustomPropertiesRule",
    ]
    .into_iter()
    .map(|r| (r.to_owned(), json!({"enabled": true})))
    .collect();
    for (name, change) in changes {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        multi_byte_fixture(root, change);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        // pylint is not needed (and may not be installed).
        let all = ["--select", "ALL", "--ignore", "SCR900"];
        for format in ["json", "text", "github", "sarif"] {
            let mut args = vec!["check", "--no-cache", "--output-format", format];
            args.extend(all);
            iglint(root, &args);
        }
        // Every file on its own, as pre-commit and editors lint it.
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap().flatten() {
                let path = entry.path();
                if entry.file_name() == ".git" {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files.sort();
        let mut args: Vec<&str> = vec!["check", "--no-cache", "--output-format", "json"];
        args.extend(all);
        let mut each = args.clone();
        each.extend(files.iter().map(|f| f.to_str().unwrap()));
        iglint(root, &each);
        let mut diff = args.clone();
        diff.extend(["--diff", "--unsafe-fixes"]);
        iglint(root, &diff);
        let mut fix = args.clone();
        fix.extend(["--fix", "--unsafe-fixes"]);
        iglint(root, &fix);
        // The ign-lint mode, with its defaults and with every rule on.
        iglint(root, &["ign-lint", "--files", "**/view.json"]);
        write(
            root,
            "rule_config.json",
            &Value::Object(ign_lint_rules.clone()).to_string(),
        );
        iglint(root, &["ign-lint", "--files", "**/view.json"]);
        std::fs::remove_file(root.join("rule_config.json")).unwrap();
        // The language server, with every rule: open every file, ask for
        // its code actions and hover over its first character.
        write(
            root,
            "iglint.toml",
            "select = [\"ALL\"]\nignore = [\"SCR900\"]\n",
        );
        let mut server = Server::start(root);
        for file in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "json" || e == "py"))
        {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            let diagnostics = server.edit(file, &text);
            let uri = file_uri(file);
            server.request(
                "textDocument/codeAction",
                json!({
                    "textDocument": {"uri": uri},
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 10000, "character": 0}},
                    "context": {"diagnostics": diagnostics}
                }),
            );
            server.request(
                "textDocument/hover",
                json!({"textDocument": {"uri": uri}, "position": {"line": 0, "character": 1}}),
            );
        }
        server.stop();
        eprintln!("{name}: {} files survived", files.len());
    }
}

/// Review round 18 (Codex): `outside_workspace` sliced a path component
/// at byte 1, so a literal path whose component starts with a multi-byte
/// character (`u"/é/report.txt"`) aborted `check` and the language
/// server's initialization with exit status 101.
#[test]
fn multi_byte_path_components_do_not_panic() {
    let dir = module("import os\nprint os.path.exists(u\"/\u{e9}/report.txt\")\n");
    let root = dir.path();
    for args in [
        &["check", "--isolated", "--no-cache"][..],
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--select",
            "ALL",
            "--ignore",
            "SCR900",
        ],
    ] {
        let mut args = args.to_vec();
        let path = root.to_str().unwrap();
        args.push(path);
        iglint(root, &args);
    }
    let mut server = Server::start(root);
    let module_path = root.join(MODULE);
    let text = std::fs::read_to_string(&module_path).unwrap();
    server.edit(&module_path, &text);
    server.stop();
}

/// A deterministic random number generator (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// True with probability `percent` / 100.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
}

/// Stand-ins replaced after JSON serialization: a surrogate pair escape
/// and lone surrogate escapes, which a Rust string cannot hold.
const SURROGATE_PAIR: char = '\u{e000}';
const LONE_HIGH: char = '\u{e001}';
const LONE_LOW: char = '\u{e002}';

/// Characters longer than a byte, to start and fill strings with:
/// two-, three- and four-byte characters, combining marks (alone and
/// after a base), a byte order mark, zero-width and bidirectional
/// controls, right-to-left scripts, emoji sequences, characters whose case
/// mapping changes their length, the last code point and line separators.
const WIDE: &[&str] = &[
    "\u{e9}",
    "\u{20ac}",
    "\u{1f600}",
    "\u{301}",
    "e\u{301}",
    "\u{feff}",
    "\u{200b}",
    "\u{200d}",
    "\u{202e}",
    "\u{200f}",
    "\u{5e9}\u{5dc}\u{5d5}\u{5dd}",
    "\u{645}\u{631}\u{62d}\u{628}\u{627}",
    "\u{6f22}\u{5b57}",
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
    "\u{2764}\u{fe0f}",
    "\u{1f1e7}\u{1f1ea}",
    "\u{131}",
    "\u{17f}",
    "\u{df}",
    "\u{130}",
    "\u{1e9e}",
    "\u{fb03}",
    "\u{10ffff}",
    "\u{fffd}",
    "\u{2028}",
    "\u{85}",
    "\u{a0}",
    "\u{3000}",
    "\u{1d400}",
    "\u{ff21}",
];

/// ASCII that means something to one of the parsers.
const SYNTAX: &[&str] = &[
    "{", "}", ".", "/", "[", "]", "%", "$", ":", "#", "*", "?", " ", "-", "_", "a", "Z", "0", "..",
    "{0}", "%s", "%(", "{x}", "${", "::", ",",
];

/// What a generated string is for, which decides what it may contain.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    /// A file or directory name (no `/`, no NUL).
    Name,
    /// A JSON string (anything, including NUL and surrogate escapes).
    Json,
    /// The content of a Python string literal (no quote, backslash or
    /// line break; escapes are added separately).
    Python,
    /// A Python comment (no line break).
    Comment,
    /// A TOML string value, and regular expressions built from it (no
    /// ASCII syntax).
    Plain,
}

fn unicode(rng: &mut Rng, slot: Slot) -> String {
    let mut out = String::new();
    let pieces = 1 + rng.below(4);
    for i in 0..pieces {
        // Most strings start with a multi-byte character.
        let wide = (i == 0 && rng.chance(80)) || slot == Slot::Plain || rng.chance(60);
        if wide {
            out.push_str(rng.pick(WIDE));
            continue;
        }
        match slot {
            Slot::Json => match rng.below(8) {
                0 => out.push('\0'),
                1 => out.push(rng.pick(&[SURROGATE_PAIR, LONE_HIGH, LONE_LOW])),
                2 => out.push_str(rng.pick(&["\"", "\\", "\n", "\t", "'", "\r\n"])),
                _ => out.push_str(rng.pick(SYNTAX)),
            },
            Slot::Python => match rng.below(6) {
                0 => out.push_str(rng.pick(&["\\\\", "\\x00", "\\u00e9", "\\n", "\\0", "\\N"])),
                _ => out.push_str(rng.pick(SYNTAX)),
            },
            Slot::Comment => out.push_str(rng.pick(SYNTAX)),
            Slot::Name => out.push_str(rng.pick(SYNTAX).trim_matches('/')),
            Slot::Plain => unreachable!(),
        }
    }
    if slot == Slot::Name {
        out = out.replace('/', "");
        if out.is_empty() || out == "." || out == ".." {
            out.insert(0, '\u{e9}');
        }
    }
    if slot == Slot::Comment {
        out = out.replace(['\u{2028}', '\u{85}'], "\u{e9}");
    }
    out
}

/// `text` with a generated string inserted at a random character boundary
/// (most often right after a character that starts a name or reference).
fn insert_somewhere(rng: &mut Rng, text: &str, slot: Slot) -> String {
    let boundaries: Vec<usize> = (0..=text.len())
        .filter(|&i| text.is_char_boundary(i))
        .collect();
    let preferred: Vec<usize> = boundaries
        .iter()
        .copied()
        .filter(|&i| {
            i > 0
                && text
                    .as_bytes()
                    .get(i - 1)
                    .is_some_and(|b| b"{[./'\":\\".contains(b))
        })
        .collect();
    let at = if !preferred.is_empty() && rng.chance(70) {
        rng.pick(&preferred)
    } else {
        rng.pick(&boundaries)
    };
    let (before, after) = text.split_at(at);
    format!("{before}{}{after}", unicode(rng, slot))
}

/// A changed copy of a JSON string or key.
fn mutate_string(
    rng: &mut Rng,
    key: &str,
    text: &str,
    renamed: &HashMap<String, String>,
) -> String {
    if let Some(new) = renamed.get(text) {
        return new.clone();
    }
    if matches!(key, "script" | "code") || text.contains("\n\t") || text.starts_with('\t') {
        return mutate_python(rng, text);
    }
    match rng.below(10) {
        0..=3 => text.to_owned(),
        4 => format!("{}{text}", unicode(rng, Slot::Json)),
        5 => format!("{text}{}", unicode(rng, Slot::Json)),
        6 => unicode(rng, Slot::Json),
        _ => insert_somewhere(rng, text, Slot::Json),
    }
}

fn mutate_json_value(
    rng: &mut Rng,
    key: &str,
    value: &Value,
    renamed: &HashMap<String, String>,
) -> Value {
    match value {
        Value::String(s) => Value::String(mutate_string(rng, key, s, renamed)),
        // File lists of manifests name the files next to them.
        Value::Array(items) if key == "files" => Value::Array(items.clone()),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| mutate_json_value(rng, key, v, renamed))
                .collect(),
        ),
        Value::Object(members) => {
            let mut out = serde_json::Map::new();
            for (k, v) in members {
                let new_key = if rng.chance(20) {
                    mutate_string(rng, "", k, renamed)
                } else {
                    k.clone()
                };
                let new_value = mutate_json_value(rng, k, v, renamed);
                out.insert(new_key, new_value);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// JSON text with the surrogate stand-ins replaced by escapes.
fn json_text(value: &Value) -> String {
    serde_json::to_string_pretty(value)
        .unwrap()
        .replace(SURROGATE_PAIR, "\\ud83d\\ude00")
        .replace(LONE_HIGH, "\\ud800")
        .replace(LONE_LOW, "\\udfff")
}

fn mutate_json(rng: &mut Rng, text: &str, renamed: &HashMap<String, String>) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => json_text(&mutate_json_value(rng, "", &value, renamed)),
        Err(_) => insert_somewhere(rng, text, Slot::Json),
    }
}

/// `source` with generated text in its string literals and comments
/// (Jython only accepts ASCII identifiers, so code stays ASCII).
fn mutate_python(rng: &mut Rng, source: &str) -> String {
    let mut out = String::with_capacity(source.len() * 2);
    let mut chars = source.chars().peekable();
    let mut quote: Option<(char, bool)> = None;
    let mut comment = false;
    while let Some(c) = chars.next() {
        out.push(c);
        if comment {
            comment = c != '\n';
            continue;
        }
        match quote {
            None if c == '#' => {
                comment = true;
                if rng.chance(60) {
                    out.push_str(&unicode(rng, Slot::Comment));
                }
            }
            None if c == '\'' || c == '"' => {
                let triple = chars.peek() == Some(&c) && {
                    let mut ahead = chars.clone();
                    ahead.next();
                    ahead.next() == Some(c)
                };
                if triple {
                    out.push(chars.next().unwrap());
                    out.push(chars.next().unwrap());
                }
                quote = Some((c, triple));
                if rng.chance(60) {
                    out.push_str(&unicode(rng, Slot::Python));
                }
            }
            None => {}
            Some(_) if c == '\\' => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            Some((q, triple)) if c == q => {
                if !triple {
                    quote = None;
                } else if chars.peek() == Some(&q) {
                    let mut ahead = chars.clone();
                    ahead.next();
                    if ahead.next() == Some(q) {
                        out.push(chars.next().unwrap());
                        out.push(chars.next().unwrap());
                        quote = None;
                    }
                }
            }
            Some((_, false)) if c == '\n' => quote = None,
            Some(_) => {}
        }
    }
    if rng.chance(30) {
        out.push_str(&format!("\n# {}\n", unicode(rng, Slot::Comment)));
    }
    out
}

/// Directory names that Ignition gives a meaning to; the others (projects,
/// resource folders, packages) get generated names.
const STRUCTURAL: &[&str] = &[
    "projects",
    "services",
    "config",
    "resources",
    "ignition",
    "com.inductiveautomation.perspective",
    "com.inductiveautomation.webdev",
    "views",
    "script-python",
    "named-query",
    "timer",
    "message",
    "tag-change",
    "scheduled",
    "startup",
    "shutdown",
    "update",
    "key-event",
    "session-props",
    "page-config",
    "stylesheet",
    "tag-definition",
    "tag-type-definition",
    "tag-provider",
    "cases",
];

/// Writes the public fixture projects (see [`multi_byte_fixture`]) to `to`
/// with generated names for every resource directory and generated text in
/// every JSON string and key, script string literal and comment, and the
/// other text files. Returns how many files were written.
fn unicode_fixture(rng: &mut Rng, to: &Path) -> usize {
    let source = tempfile::tempdir().unwrap();
    multi_byte_fixture(source.path(), &|t: &str| t.to_owned());
    let mut files = Vec::new();
    let mut stack = vec![source.path().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut targets = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(source.path()).unwrap();
        let parts: Vec<String> = rel
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let mut target = to.to_path_buf();
        for (i, part) in parts.iter().enumerate() {
            let is_dir = i + 1 < parts.len();
            let inside = parts[..i].iter().any(|p| STRUCTURAL.contains(&p.as_str()));
            if is_dir && inside && !STRUCTURAL.contains(&part.as_str()) {
                let new = renamed.entry(part.clone()).or_insert_with(|| {
                    if rng.chance(70) {
                        unicode(rng, Slot::Name)
                    } else {
                        part.clone()
                    }
                });
                target.push(new.as_str());
            } else {
                target.push(part);
            }
        }
        targets.push((file.clone(), target));
    }
    for (from, target) in &targets {
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let Ok(text) = std::fs::read_to_string(from) else {
            std::fs::copy(from, target).unwrap();
            continue;
        };
        let extension = from.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut changed = match extension {
            "json" => mutate_json(rng, &text, &renamed),
            "py" => mutate_python(rng, &text),
            _ => insert_somewhere(rng, &text, Slot::Comment),
        };
        // Files that start with a multi-byte character (review round 17).
        if rng.chance(10) {
            changed.insert_str(0, rng.pick(&["\u{feff}", "\u{e9}", "\u{1f600}"]));
        }
        std::fs::write(target, changed).unwrap();
    }
    targets.len()
}

/// A TOML basic string.
fn toml_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A script library module that passes generated strings to every API
/// whose string arguments iglint reads: file paths (POSIX, Windows drive
/// and UNC paths), URLs, format strings, `getattr` names, component
/// paths, tag paths, named query paths, message types and credentials.
fn unicode_module(rng: &mut Rng) -> String {
    let mut u = || unicode(rng, Slot::Python);
    let lines = [
        "import os".to_owned(),
        "import os.path".to_owned(),
        "def f(self, event):".to_owned(),
        format!("    p = u\"{}\"", u()),
        format!("    print os.path.exists(u\"/{}/report.txt\")", u()),
        format!("    print os.path.exists(\"/{}/{}\")", u(), u()),
        format!("    print open(u\"C:\\\\{}\\\\x\").read()", u()),
        format!("    print open(r\"C:\\{}\\x\", \"w\")", u()),
        format!("    print os.listdir(u\"\\\\\\\\server\\\\{}\")", u()),
        format!("    print os.listdir(\"\\\\\\\\{}\\\\share\")", u()),
        format!("    print open(u\"{}\" + p)", u()),
        format!("    print open(os.path.join(u\"/{}\", u\"{}\"))", u(), u()),
        format!("    print u\"{}\".format(1, x=2)", u()),
        format!("    print \"{}\" % (1,)", u()),
        format!("    print u\"%({})s\" % {{\"{}\": 1}}", u(), u()),
        format!("    print getattr(self, \"{}\")", u()),
        format!("    print getattr(self, u\"{}\", None)", u()),
        format!("    setattr(self, \"{}\", 1)", u()),
        format!("    print self.getChild(\"{}\").props.text", u()),
        format!(
            "    print self.getSibling(u\"{}\").custom[\"{}\"]",
            u(),
            u()
        ),
        format!("    print self.parent.getChild(\"{}/{}\")", u(), u()),
        format!("    print self.view.getChild(\"root/{}\")", u()),
        format!(
            "    print system.tag.readBlocking([\"[default]{}/{}\", u\"{}\"])",
            u(),
            u(),
            u()
        ),
        format!("    system.tag.writeBlocking([u\"[{}]{}\"], [1])", u(), u()),
        format!(
            "    print system.db.runNamedQuery(u\"{}/{}\", {{\"{}\": 1}})",
            u(),
            u(),
            u()
        ),
        format!("    system.perspective.navigate(u\"/{}\")", u()),
        format!(
            "    system.perspective.sendMessage(u\"{}\", {{}}, scope=\"page\")",
            u()
        ),
        format!("    system.util.getLogger(u\"{}\").info(u\"{}\")", u(), u()),
        format!("    print system.net.httpGet(u\"http://{}/x\")", u()),
        format!("    print system.net.httpGet(u\"file:///{}/x\")", u()),
        format!("    print system.net.httpGet(\"file://{}/{}\")", u(), u()),
        format!("    system.util.execute([u\"{}\", \"{}\"])", u(), u()),
        format!("    url = \"https://user:{}@{}/\"", u(), u()),
        format!("    password = u\"{}\"", u()),
        format!("    print \"{}\" is \"{}\"", u(), u()),
        format!(
            "    print system.{}",
            ["tag.read", "util.jsonDecode", "x"][0]
        ),
        format!("    return p  # {}", unicode(rng, Slot::Comment)),
    ];
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// A view whose names, keys, values, bindings, expressions, paths and
/// scripts are generated strings.
fn unicode_view(rng: &mut Rng) -> String {
    let mut j = || unicode(rng, Slot::Json);
    let (custom, param, child) = (j(), j(), j());
    let script = format!(
        "\tprint u\"{}\"\n\tself.getSibling(\"{}\").props.text = self.view.custom[\"{}\"]\n",
        unicode(rng, Slot::Python),
        unicode(rng, Slot::Python),
        unicode(rng, Slot::Python)
    );
    let mut j = || unicode(rng, Slot::Json);
    let view = json!({
        "custom": {custom.clone(): j(), j(): {j(): [j(), 1]}},
        "params": {param.clone(): j()},
        "propConfig": {
            format!("custom.{custom}"): {"persistent": true, "binding": {
                "type": "expr", "config": {"expression": format!("{{view.params.{param}}} + \"{}\"", j())}
            }},
            format!("params.{param}"): {"paramDirection": "input", "persistent": true}
        },
        "props": {"defaultSize": {"width": 100, "height": 100}},
        "root": {
            "type": "ia.container.flex",
            "meta": {"name": "root"},
            "scripts": {
                "customMethods": [{"name": j(), "params": ["value"], "script": script}],
                "messageHandlers": [{
                    "messageType": j(), "pageScope": true, "script": script,
                    "sessionScope": false, "viewScope": false
                }]
            },
            "children": [
                {
                    "type": "ia.display.label",
                    "meta": {"name": child.clone()},
                    "custom": {j(): j()},
                    "props": {"text": j(), "style": {"classes": format!("{} {}", j(), j())}},
                    "propConfig": {
                        "props.text": {"binding": {
                            "type": "expr",
                            "config": {"expression": format!(
                                "{{view.custom.{custom}}} + {{../{child}.custom.{}}} + {{this.custom.{}}} + \"{}\" + if(1, '{}', {{session.custom.{}}})",
                                j(), j(), j(), j(), j()
                            )},
                            "transforms": [
                                {"type": "script", "code": script},
                                {"type": "expression", "expression": format!("{{value}} + '{}'", j())},
                                {"type": "map", "inputType": "scalar", "outputType": "scalar",
                                 "mappings": [{"input": j(), "output": j()}], "fallback": j()},
                                {"type": "format", "formatType": "numeric", "formatValue": j()}
                            ]
                        }},
                        format!("custom.{}", j()): {"binding": {
                            "type": "tag", "config": {
                                "mode": "indirect",
                                "tagPath": format!("[default]{}/{{{}}}/{}", j(), j(), j()),
                                "references": {j(): format!("{{view.params.{param}}}")},
                                "fallbackDelay": 2.5
                            }
                        }},
                        format!("custom.{}", j()): {"binding": {
                            "type": "tag", "config": {"mode": "direct", "tagPath": format!("[{}]{}", j(), j())}
                        }},
                        "props.style.color": {"binding": {
                            "type": "property", "config": {"path": format!("view.custom.{custom}")}
                        }},
                        "position.display": {"binding": {
                            "type": "query", "config": {
                                "queryPath": format!("{}/{}", j(), j()),
                                "parameters": {j(): format!("{{view.params.{param}}}")},
                                "polling": {"enabled": true, "rate": "1"}
                            }
                        }}
                    },
                    "events": {
                        "dom": {"onClick": {"type": "script", "scope": "G", "config": {"script": script}}},
                        "component": {"onActionPerformed": [
                            {"type": "nav", "config": {"url": j(), "page": j()}},
                            {"type": "popup", "config": {"viewPath": j(), "id": j(), "viewParams": {j(): j()}}}
                        ]}
                    }
                },
                {
                    "type": "ia.display.view",
                    "meta": {"name": j()},
                    "props": {"path": format!("{}/{}", j(), j()), "params": {j(): j()}}
                },
                {
                    "type": "ia.display.flex-repeater",
                    "meta": {"name": j()},
                    "props": {"path": j(), "instances": [{j(): j()}, {"instanceStyle": {"classes": j()}}]}
                },
                {"type": j(), "meta": {"name": j()}, "props": {j(): j()}}
            ]
        }
    });
    json_text(&view)
}

/// Writes generated resources of every kind into the project at
/// `project`, the gateway collection `gateway`, and the configuration
/// files at `root`.
fn unicode_resources(rng: &mut Rng, root: &Path, project: &Path, gateway: &Path) {
    let manifest = |files: &[&str]| {
        let files: Vec<Value> = files.iter().map(|f| json!(f)).collect();
        json!({"scope": "G", "version": 1, "restricted": false, "overridable": true,
               "files": files, "attributes": {}})
        .to_string()
    };
    let put = |path: &Path, text: &str| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let project_json = json!({"title": unicode(rng, Slot::Json), "description": unicode(rng, Slot::Json),
                              "enabled": true, "inheritable": true});
    put(&project.join("project.json"), &json_text(&project_json));
    for _ in 0..2 {
        let dir = project
            .join("ignition/script-python")
            .join(unicode(rng, Slot::Name))
            .join(unicode(rng, Slot::Name));
        put(&dir.join("code.py"), &unicode_module(rng));
        put(&dir.join("resource.json"), &manifest(&["code.py"]));
        // A file Ignition does not know, with a generated name.
        put(
            &dir.join(format!("{}.py", unicode(rng, Slot::Name))),
            &unicode_module(rng),
        );
    }
    for _ in 0..2 {
        let dir = project
            .join("com.inductiveautomation.perspective/views")
            .join(unicode(rng, Slot::Name))
            .join(unicode(rng, Slot::Name));
        put(&dir.join("view.json"), &unicode_view(rng));
        put(&dir.join("resource.json"), &manifest(&["view.json"]));
        put(
            &dir.join(format!("{}.json", unicode(rng, Slot::Name))),
            &unicode_view(rng),
        );
    }
    let body = unicode_module(rng).replace("def f(self, event):", "if True:");
    for (kind, file, header) in [
        ("timer", "handleTimerEvent.py", "def handleTimerEvent():"),
        ("message", "handleMessage.py", "def handleMessage(payload):"),
        (
            "tag-change",
            "onTagChange.py",
            "def onTagChange(initialChange, newValue, previousValue, event, executionCount):",
        ),
        (
            "scheduled",
            "handleScheduleEvent.py",
            "def handleScheduleEvent():",
        ),
    ] {
        let dir = project
            .join("ignition")
            .join(kind)
            .join(unicode(rng, Slot::Name));
        let indented: String = body.lines().map(|l| format!("\t{l}\n")).collect();
        put(&dir.join(file), &format!("{header}\n{indented}"));
        put(&dir.join("resource.json"), &manifest(&[file]));
    }
    let webdev = project
        .join("com.inductiveautomation.webdev/resources")
        .join(unicode(rng, Slot::Name));
    put(
        &webdev.join("doGet.py"),
        &format!(
            "def doGet(request, session):\n\treturn {{\"json\": {{u\"{}\": request[\"params\"].get(\"{}\")}}}}\n",
            unicode(rng, Slot::Python),
            unicode(rng, Slot::Python)
        ),
    );
    put(
        &webdev.join("config.json"),
        &json_text(
            &json!({"doGet": {"enabled": true, "require-auth": false}, "description": unicode(rng, Slot::Json)}),
        ),
    );
    put(
        &webdev.join("resource.json"),
        &manifest(&["config.json", "doGet.py"]),
    );
    let query = project
        .join("ignition/named-query")
        .join(unicode(rng, Slot::Name))
        .join(unicode(rng, Slot::Name));
    put(
        &query.join("query.sql"),
        &format!(
            "SELECT '{}' FROM t WHERE a = :{} AND b = {{{}}} -- {}\n",
            unicode(rng, Slot::Python),
            unicode(rng, Slot::Plain),
            unicode(rng, Slot::Plain),
            unicode(rng, Slot::Comment)
        ),
    );
    put(&query.join("resource.json"), &manifest(&["query.sql"]));
    // Tags and UDTs.
    put(
        &gateway.join("config-mode.json"),
        &json_text(&json!({"title": unicode(rng, Slot::Json)})),
    );
    let provider = gateway
        .join("ignition/tag-provider")
        .join(unicode(rng, Slot::Name));
    put(
        &provider.join("config.json"),
        r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
    );
    put(&provider.join("resource.json"), &manifest(&["config.json"]));
    let tag_script = format!("\tprint u\"{}\"\n", unicode(rng, Slot::Python));
    let mut j2 = || unicode(rng, Slot::Json);
    let tags = json!({"tags": [
        {"name": j2(), "tagType": "AtomicTag", "valueSource": "expr",
         "expression": format!("{{[.]{}}} + {{[~]{}/{}}} + '{}'", j2(), j2(), j2(), j2()),
         "eventScripts": [{"eventid": "valueChanged", "script": tag_script}]},
        {"name": j2(), "tagType": "AtomicTag", "valueSource": "reference", "sourceTagPath": format!("[{}]{}", j2(), j2())},
        {"name": j2(), "tagType": "Folder", "tags": [
            {"name": j2(), "tagType": "UdtInstance", "typeId": j2(),
             "parameters": {j2(): {"dataType": "String", "value": j2()}}, "tags": []}
        ]}
    ]});
    let mut j = || unicode(rng, Slot::Json);
    let udts = json!({"tags": [
        {"name": j(), "tagType": "UdtType",
         "parameters": {j(): {"dataType": "String", "value": j()}},
         "tags": [{"name": j(), "tagType": "AtomicTag", "valueSource": "opc",
                   "opcItemPath": format!("ns=1;s={{{}}}.{}", j(), j())}]}
    ]});
    let folder = gateway
        .join("ignition/tag-definition")
        .join(unicode(rng, Slot::Name));
    put(&folder.join("tags.json"), &json_text(&tags));
    put(
        &folder.join("unary-resource.json"),
        &manifest(&["tags.json"]),
    );
    let types = gateway
        .join("ignition/tag-type-definition")
        .join(unicode(rng, Slot::Name));
    put(&types.join("udts.json"), &json_text(&udts));
    put(
        &types.join("unary-resource.json"),
        &manifest(&["udts.json"]),
    );
    // Configuration: iglint.toml, an ign-lint rule_config.json and
    // whitelist with generated values.
    let mut p = || toml_string(&unicode(rng, Slot::Plain));
    let toml = format!(
        "select = [\"ALL\"]\nignore = [\"SCR900\"]\nexclude = [{}]\n\
         [rules.NAME001]\nconvention = \"camelCase\"\nforbidden-names = [{}]\nskip-names = [\"root\", {}]\n\
         allowed-abbreviations = [{}, \"\\u0131\", \"\\u017F\", \"\\u00DF\"]\n\
         target-node-types = [\"component\", \"property\", \"message_handler\", \"custom_method\"]\n\
         [rules.VIEW018]\nforbidden-patterns = [{}, \"../\"]\n\
         [rules.BIND002]\nextra-functions = [{}]\n\
         [rules.SCR201]\nallow = [{}]\n\
         [rules.SCR117]\nallowed = [{}]\n\
         [rules.VIEW003]\nallowed-types = [{}]\n\
         [rules.VIEW012]\nexempt-props = [{}]\n\
         [rules.SCR103]\ndummy-pattern = {}\n\
         [[overrides]]\nfiles = [{}]\nignore = [\"NAME001\"]\n",
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p(),
        p()
    );
    put(&root.join("iglint.toml"), &toml);
    let mut j = || unicode(rng, Slot::Json);
    let rule_config = json!({
        "NamePatternRule": {"kwargs": {"convention": "PascalCase", "forbidden_names": [j()],
            "skip_names": ["root", j()], "allowed_abbreviations": [j(), "\u{131}", "\u{17f}"],
            "target_node_types": ["component", "property", "message_handler", "custom_method"]}},
        "BadComponentReferenceRule": {"kwargs": {"forbidden_patterns": [j(), "../"]}},
        "PropertyAccessRule": {"kwargs": {"expected_access": "private", "exempt_props": [j()]}},
        "PropertyPersistenceRule": {"enabled": true},
        "ExcessiveContextDataRule": {"enabled": true},
        "PollingIntervalRule": {"enabled": true},
        "UnusedCustomPropertiesRule": {"enabled": true},
        "ComponentReferenceValidationRule": {"enabled": true},
        "PylintScriptRule": {"enabled": false}
    });
    // Without surrogate escapes: ign-lint rejects them in its configuration.
    put(
        &root.join("rule_config.json"),
        &serde_json::to_string_pretty(&rule_config).unwrap(),
    );
    let whitelist: String = (0..3)
        .map(|_| {
            format!(
                "{}/{}\n",
                unicode(rng, Slot::Name),
                unicode(rng, Slot::Name)
            )
        })
        .collect();
    put(&root.join(".ignition-lint-whitelist.txt"), &whitelist);
}

/// The files under `root`, sorted, without `.git`.
fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if entry.file_name() == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// `iglint check --stdin-filename` with `text` on standard input.
fn iglint_stdin(dir: &Path, file: &Path, text: &str) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_iglint"))
        .args([
            "check",
            "--no-cache",
            "--output-format",
            "json",
            "--stdin-filename",
        ])
        .arg(file)
        .arg("-")
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "stdin {file:?} failed with {:?}: {stderr}",
        output.status
    );
}

/// Every command over one generated workspace.
fn unicode_workspace(seed: u64) {
    let mut rng = Rng(seed);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    unicode_fixture(&mut rng, &root.join("fixtures"));
    let project = root.join("projects").join(unicode(&mut rng, Slot::Name));
    let gateway = root
        .join("services/gw/config/resources")
        .join(unicode(&mut rng, Slot::Name));
    unicode_resources(&mut rng, root, &project, &gateway);
    let files = files_under(root);
    let context = format!("seed {seed:#x}");
    eprintln!("{context}: {} files", files.len());
    if let Some(keep) = std::env::var_os("IGLINT_UNICODE_KEEP") {
        let keep = Path::new(&keep).join(format!("{seed:x}"));
        for file in &files {
            let target = keep.join(file.strip_prefix(root).unwrap());
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::copy(file, target).unwrap();
        }
    }

    for format in [
        "text",
        "concise",
        "grouped",
        "json",
        "json-lines",
        "sarif",
        "github",
        "azure",
        "junit",
        "pylint",
        "gitlab",
    ] {
        iglint(root, &["check", "--no-cache", "--output-format", format]);
    }
    iglint(
        root,
        &[
            "check",
            "--no-cache",
            "--isolated",
            "--select",
            "ALL",
            "--ignore",
            "SCR900",
            "--statistics",
        ],
    );
    let mut each = vec!["check", "--no-cache", "--output-format", "json"];
    each.extend(files.iter().map(|f| f.to_str().unwrap()));
    iglint(root, &each);
    for file in files.iter().step_by(7) {
        if let Ok(text) = std::fs::read_to_string(file) {
            iglint_stdin(root, file, &text);
        }
    }
    iglint(root, &["check", "--no-cache", "--diff", "--unsafe-fixes"]);
    iglint(root, &["baseline", "--no-cache"]);
    iglint(root, &["check", "--no-cache", "--output-format", "json"]);
    iglint(root, &["migrate", "--dry-run"]);
    iglint(root, &["ign-lint", "--files", "**/view.json"]);
    iglint(
        root,
        &[
            "ign-lint",
            "--files",
            "**/*.json",
            "--config",
            "rule_config.json",
        ],
    );

    // The language server: open every script and JSON file, ask for code
    // actions, hover at a few positions, edit inside multi-byte
    // characters and fix everything.
    let mut server = Server::start(root);
    for file in files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "json" || e == "py"))
        .step_by(2)
    {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let diagnostics = server.edit(file, &text);
        let uri = file_uri(file);
        let lines = text.lines().count().max(1);
        server.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": uri},
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": lines + 1, "character": 0}},
                "context": {"diagnostics": diagnostics}
            }),
        );
        for _ in 0..3 {
            let position = json!({"line": rng.below(lines), "character": rng.below(40)});
            server.request(
                "textDocument/hover",
                json!({"textDocument": {"uri": uri}, "position": position}),
            );
        }
        let at = json!({"line": rng.below(lines), "character": rng.below(40)});
        server.notify(
            "textDocument/didChange",
            json!({"textDocument": {"uri": uri, "version": 1_000_000},
                   "contentChanges": [{"range": {"start": at, "end": at}, "text": unicode(&mut rng, Slot::Json)}]}),
        );
        server.request(
            "workspace/executeCommand",
            json!({"command": "iglint.applyFixAll", "arguments": [{"uri": uri}]}),
        );
    }
    server.request(
        "workspace/executeCommand",
        json!({"command": "iglint.ruleDocs", "arguments": [unicode(&mut rng, Slot::Json)]}),
    );
    server.stop();

    iglint(root, &["check", "--no-cache", "--fix", "--unsafe-fixes"]);
    eprintln!("{context}: survived");
}

/// Review round 18 (Codex): the second character boundary panic in two
/// rounds. Generated workspaces put random Unicode (multi-byte first
/// characters, combining marks, emoji, bidirectional controls, NUL and
/// surrogate escapes where JSON allows them) in every name and string
/// position: directory and file names, JSON keys and values, component
/// and property names, bindings, expressions, tag paths, script string
/// literals and comments, POSIX, Windows and UNC paths in scripts, and
/// configuration values. Every command and the language server must
/// survive them. `IGLINT_UNICODE_SEED` replays one seed (hexadecimal);
/// `IGLINT_UNICODE_KEEP=<dir>` keeps a copy of each generated workspace.
#[test]
fn unicode_everywhere_does_not_panic() {
    match std::env::var("IGLINT_UNICODE_SEED") {
        Ok(seed) => {
            let seed = seed.trim_start_matches("0x");
            unicode_workspace(u64::from_str_radix(seed, 16).unwrap());
        }
        Err(_) => {
            for seed in 0..4 {
                unicode_workspace(0x18_0000 + seed);
            }
        }
    }
}

/// The long version: `IGLINT_UNICODE_CASES` workspaces (default 100).
#[test]
#[ignore = "long: cargo test --release -p iglint --test stress unicode_everywhere_long -- --ignored"]
fn unicode_everywhere_long() {
    let cases: u64 = std::env::var("IGLINT_UNICODE_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(100);
    for seed in 0..cases {
        unicode_workspace(0x18_1000 + seed);
    }
}
