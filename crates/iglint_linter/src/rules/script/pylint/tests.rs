use iglint_text::{TextRange, TextSize};

use super::run::{Script, map_messages, parse_report, source_range};
use super::*;
#[cfg(unix)]
use crate::diagnostic::Diagnostic;
use crate::engine::FixMode;
use crate::rules::script::testing::*;
#[cfg(unix)]
use crate::rules::test_support::Fixture;

/// `(line, byte column, character column)` of `needle` in `source`, as
/// pylint counts them.
fn position(source: &str, needle: &str) -> (i64, i64, i64) {
    let at = source.find(needle).expect("needle");
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let line = source[..at].matches('\n').count() as i64 + 1;
    let bytes = (at - line_start) as i64;
    let chars = source[line_start..at].chars().count() as i64;
    (line, bytes, chars)
}

#[test]
fn parses_pylint_json() {
    let text = r#"[
      {"type": "error", "module": "iglint_0", "obj": "f", "line": 2, "column": 4,
       "endLine": 2, "endColumn": 15, "path": "iglint_0.py",
       "symbol": "undefined-variable", "message": "Undefined variable 'xé'",
       "message-id": "E0602"},
      {"type": "convention", "module": "iglint_1", "obj": "", "line": 1, "column": 0,
       "endLine": null, "endColumn": null, "path": "./iglint_1.py",
       "symbol": "line-too-long", "message": "Line too long (150/140)",
       "message-id": "C0301"}
    ]"#;
    let raw = parse_report(text).unwrap();
    assert_eq!(raw.len(), 2);
    assert_eq!(raw[0].message, "Undefined variable 'x\u{e9}'");
    assert_eq!(raw[1].end_line, None);
    let sources = ["def f():\n    xé = 1\n", "x = 1\n"];
    let scripts: Vec<Script<'_>> = sources
        .iter()
        .map(|s| Script {
            source: s,
            embedded: false,
        })
        .collect();
    let (messages, notices) = map_messages(&raw, &scripts);
    assert!(notices.is_empty());
    assert_eq!(messages[0][0].symbol, "undefined-variable");
    assert_eq!(messages[0][0].range.slice(sources[0]), "xé = 1");
    assert_eq!(messages[1][0].id, "C0301");
    assert_eq!(messages[1][0].range, TextRange::empty(TextSize::ZERO));
    assert!(parse_report("Traceback (most recent call last):").is_err());
}

#[test]
fn messages_outside_scripts_are_notices() {
    let raw = parse_report(
        r#"[{"line": 1, "column": 0, "path": "/tmp/x/pylintrc", "symbol": "unrecognized-option",
            "message": "Unrecognized option found: bogus", "message-id": "E0015"}]"#,
    )
    .unwrap();
    let (messages, notices) = map_messages(
        &raw,
        &[Script {
            source: "",
            embedded: false,
        }],
    );
    assert!(messages[0].is_empty());
    assert_eq!(
        notices,
        ["SCR900: pylint: Unrecognized option found: bogus (unrecognized-option, E0015)"]
    );
}

#[test]
fn columns_are_bytes_for_nodes_and_characters_for_tokens() {
    let source = "a = 'éé'; b = 'x\\d'\r\nc = 1\rd = undefined\n";
    // Node message (with an end): byte columns.
    let (line, bytes, _) = position(source, "b = ");
    let r = source_range(source, line, bytes, Some((line, bytes + 5)));
    assert_eq!(r.slice(source), "b = '");
    // Token message (no end): character columns, empty range.
    let (line, bytes, chars) = position(source, "\\d");
    assert_ne!(bytes, chars);
    let r = source_range(source, line, chars, None);
    assert_eq!(
        r,
        TextRange::empty(TextSize::from(source.find("\\d").unwrap()))
    );
    // `\r\n` and a lone `\r` end lines.
    let (line, _, _) = position(source, "undefined");
    assert_eq!(line, 2, "position() counts only \\n, pylint counts 3");
    let r = source_range(source, 3, 4, Some((3, 13)));
    assert_eq!(r.slice(source), "undefined");
    // Out of range positions clamp.
    assert_eq!(
        source_range(source, 99, 3, None),
        TextRange::empty(TextSize::of(source))
    );
    assert_eq!(
        source_range(source, 2, 99, Some((2, 120))).slice(source),
        ""
    );
    assert_eq!(
        source_range(source, 0, 0, None),
        TextRange::empty(TextSize::ZERO)
    );
}

#[test]
fn options() {
    let meta = &META;
    let empty = Default::default();
    let parsed = parse_options(&OptionReader::new(meta, &empty)).unwrap();
    let o = parsed.downcast_ref::<PylintOptions>().unwrap();
    assert_eq!(o.command, ["pylint"]);
    assert_eq!(o.jobs, 0);
    assert_eq!(o.pylintrc, None);
    assert_eq!(o.severity_of("E0602"), Some(Severity::Error));
    assert_eq!(o.severity_of("F0002"), Some(Severity::Error));
    assert_eq!(o.severity_of("C0103"), Some(Severity::Warning));
    assert_eq!(o.severity_of("I1101"), None);

    let values = crate::rules::test_support::options_from_toml(
        "pylint = \"python3  -m pylint\"\njobs = 2\ncategory-mapping = { c = \"info\", E = \"warning\" }",
    );
    let parsed = parse_options(&OptionReader::new(meta, &values)).unwrap();
    let o = parsed.downcast_ref::<PylintOptions>().unwrap();
    assert_eq!(o.command, ["python3", "-m", "pylint"]);
    assert_eq!(o.jobs, 2);
    // The mapping replaces the default.
    assert_eq!(o.severity_of("C0103"), Some(Severity::Info));
    assert_eq!(o.severity_of("E0602"), Some(Severity::Warning));
    assert_eq!(o.severity_of("W0611"), None);

    for bad in [
        "jobs = -1",
        "pylint = \" \"",
        "category-mapping = { X = \"error\" }",
        "category-mapping = { EW = \"error\" }",
        "category-mapping = { E = \"fatal\" }",
    ] {
        let values = crate::rules::test_support::options_from_toml(bad);
        assert!(
            parse_options(&OptionReader::new(meta, &values)).is_err(),
            "{bad}"
        );
    }
}

/// A fake pylint: a shell script that prints `messages` for `iglint_0.py`
/// and counts its runs in `runs.txt` next to it.
#[cfg(unix)]
fn fake_pylint(f: Fixture, messages: &serde_json::Value) -> (Fixture, String) {
    let script = f.path("fake-pylint.sh");
    let runs = f.path("runs.txt");
    let body = format!(
        "#!/bin/sh\necho run >> '{}'\ntest -f iglint_0.py || exit 32\ncat <<'IGLINT_EOF'\n{}\nIGLINT_EOF\nexit 0\n",
        runs.display(),
        serde_json::to_string_pretty(messages).unwrap()
    );
    let f = f.file("fake-pylint.sh", &body);
    (f, format!("pylint = \"/bin/sh {}\"", script.display()))
}

#[cfg(unix)]
fn message(
    symbol: &str,
    id: &str,
    text: &str,
    (line, column): (i64, i64),
    end: Option<(i64, i64)>,
) -> serde_json::Value {
    serde_json::json!({
        "type": "x", "module": "iglint_0", "obj": "",
        "line": line, "column": column,
        "endLine": end.map(|e| e.0), "endColumn": end.map(|e| e.1),
        "path": "iglint_0.py", "symbol": symbol, "message": text, "message-id": id,
    })
}

#[cfg(unix)]
fn runs(f: &Fixture) -> usize {
    std::fs::read_to_string(f.path("runs.txt"))
        .map(|t| t.lines().count())
        .unwrap_or(0)
}

#[cfg(unix)]
#[test]
fn maps_view_script_messages_to_exact_bytes() {
    let body = "\tx = \"é\\t\"; y = undefined_a\n\tz = system.tag\n\tw = xrange(2) + util.f()\n\tif x == None:\n\t\tpass";
    let source = format!("def runAction(self, event):\n{body}");
    let (l, b, c) = position(&source, "undefined_a");
    let (l2, b2, _) = position(&source, "system");
    let (l3, b3, _) = position(&source, "xrange");
    let (l4, b4, _) = position(&source, "util");
    let (l5, b5, _) = position(&source, "x == None");
    let messages = serde_json::json!([
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'undefined_a'",
            (l, b),
            Some((l, b + 11))
        ),
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'system'",
            (l2, b2),
            Some((l2, b2 + 6))
        ),
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'xrange'",
            (l3, b3),
            Some((l3, b3 + 6))
        ),
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'util'",
            (l4, b4),
            Some((l4, b4 + 4))
        ),
        message(
            "invalid-name",
            "C0103",
            "Function name \"runAction\" doesn't conform to snake_case naming style",
            (1, 4),
            Some((1, 13))
        ),
        message(
            "too-many-branches",
            "R0912",
            "Too many branches (13/12)",
            (1, 0),
            Some((1, 13))
        ),
        message(
            "anomalous-backslash-in-string",
            "W1401",
            "Anomalous backslash",
            (l, c),
            None
        ),
        message(
            "singleton-comparison",
            "C0121",
            "Comparison 'x == None' should be 'x is None'",
            (l5, b5),
            Some((l5, b5 + 9))
        ),
        message("syntax-error", "E0001", "Parsing failed", (2, 0), None),
    ]);
    let f = view(ViewScript::Action, body);
    let f = with_library(f, "util", "def f():\n\treturn 1\n");
    let (f, command) = fake_pylint(f, &messages);
    let d: Vec<Diagnostic> = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(runs(&f), 1);
    let found: Vec<(String, String, Severity)> = decoded_flagged(&d)
        .into_iter()
        .zip(&d)
        .map(|(text, d)| (text, d.message.clone(), d.severity))
        .collect();
    assert_eq!(
        found,
        [
            (
                "\"script\"".to_owned(),
                "Too many branches (13/12) (too-many-branches, R0912)".to_owned(),
                Severity::Warning
            ),
            (
                String::new(),
                "Anomalous backslash (anomalous-backslash-in-string, W1401)".to_owned(),
                Severity::Warning
            ),
            (
                "undefined_a".to_owned(),
                "Undefined variable 'undefined_a' (undefined-variable, E0602)".to_owned(),
                Severity::Error
            ),
            (
                "x == None".to_owned(),
                "Comparison 'x == None' should be 'x is None' (singleton-comparison, C0121)"
                    .to_owned(),
                Severity::Warning
            ),
        ]
    );
    // The token message's empty range sits right before `undefined_a`.
    assert_eq!(d[1].range.start(), d[2].range.start());
    assert_eq!(
        d[2].ign_lint_message.as_deref(),
        Some("Line 1: Undefined variable 'undefined_a' (undefined-variable) (E0602)")
    );
    assert_eq!(
        d[2].ign_lint_path.as_deref(),
        Some("root.root.events.component.onActionPerformed")
    );
}

#[cfg(unix)]
#[test]
fn event_files_and_library_modules_map_to_file_lines() {
    let code = "def handleMessage(payload):\n\tx = undefined_b\n";
    let (l, b, _) = position(code, "undefined_b");
    let messages = serde_json::json!([message(
        "undefined-variable",
        "E0602",
        "Undefined variable 'undefined_b'",
        (l, b),
        Some((l, b + 11))
    )]);
    let (f, command) = fake_pylint(gateway_message(code), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(lines(&d), [(2, "undefined_b".to_owned())]);

    // A replaced header: header messages go to the dropped first line.
    let code = "def handleMessage(payload, extra=1):\n\tpass\n";
    let messages = serde_json::json!([
        message(
            "too-many-statements",
            "R0915",
            "Too many statements",
            (1, 0),
            Some((1, 17))
        ),
        message(
            "missing-function-docstring",
            "C0116",
            "Missing function or method docstring",
            (1, 0),
            Some((1, 17))
        ),
    ]);
    let (f, command) = fake_pylint(gateway_message(code), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(
        lines(&d),
        [(1, "def handleMessage(payload, extra=1):".to_owned())]
    );

    let code = "import os\n\ndef f():\n    return os\n";
    let messages = serde_json::json!([message(
        "bad-indentation",
        "W0311",
        "Bad indentation. Found 4 spaces, expected 1",
        (4, 0),
        None
    )]);
    let (f, command) = fake_pylint(library("m", code), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(lines(&d), [(4, String::new())]);
    assert_eq!(
        d[0].range.start(),
        TextSize::from(code.find("    return").unwrap())
    );
}

#[cfg(unix)]
#[test]
fn custom_method_headers_are_the_users() {
    let messages = serde_json::json!([message(
        "invalid-name",
        "C0103",
        "Argument name \"a\" doesn't conform",
        (1, 12),
        Some((1, 13))
    )]);
    let (f, command) = fake_pylint(view(ViewScript::Method, "\treturn a + b"), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(decoded_flagged(&d), ["\"script\""]);
}

#[cfg(unix)]
#[test]
fn category_mapping_and_rule_severity() {
    let messages = serde_json::json!([
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'q'",
            (2, 1),
            Some((2, 2))
        ),
        message(
            "invalid-name",
            "C0103",
            "Variable name \"Q\" doesn't conform",
            (3, 1),
            Some((3, 2))
        ),
    ]);
    let (f, command) = fake_pylint(library("m", "def f():\n\tq\n\tQ = 1\n"), &messages);
    let options = format!("{command}\ncategory-mapping = {{ C = \"info\" }}");
    let d = f.lint_with(&[("SCR900", &options)]);
    let severities: Vec<Severity> = d.iter().map(|d| d.severity).collect();
    // `E` is not mapped any more: the rule severity applies.
    assert_eq!(severities, [Severity::Warning, Severity::Info]);
}

#[cfg(unix)]
#[test]
fn python_2_syntax_errors_are_dropped() {
    let messages = serde_json::json!([message(
        "syntax-error",
        "E0001",
        "Parsing failed: 'Missing parentheses in call to 'print'.'",
        (2, 1),
        None
    )]);
    // Valid Jython: never reported.
    let (f, command) = fake_pylint(library("m", "def f():\n\tprint 'x'\n"), &messages);
    assert!(f.lint_with(&[("SCR900", &command)]).is_empty());
    // Invalid for Jython too: SCR001 reports it, or pylint when SCR001 is off.
    let (f, command) = fake_pylint(library("m", "def f(:\n\tpass\n"), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(d.len(), 1);
    let d = f.lint_with(&[("SCR900", &command), ("SCR001", "")]);
    assert_eq!(
        d.iter().map(|d| d.rule.code()).collect::<Vec<_>>(),
        ["SCR001"]
    );
}

#[cfg(unix)]
#[test]
fn injected_names_are_never_undefined() {
    let code =
        "x = system.tag\ny = fpmi\nz = sys.argv\nw = util.f\nv = basestring\nu = __package__\n";
    let messages: Vec<serde_json::Value> =
        ["system", "fpmi", "sys", "util", "basestring", "__package__"]
            .iter()
            .map(|name| {
                let (l, b, _) = position(code, name);
                message(
                    "undefined-variable",
                    "E0602",
                    &format!("Undefined variable '{name}'"),
                    (l, b),
                    Some((l, b + name.len() as i64)),
                )
            })
            .chain(std::iter::once(message(
                "import-error",
                "E0401",
                "Unable to import 'util.sub'",
                (1, 0),
                Some((1, 3)),
            )))
            .collect();
    let f = with_library(library("m", code), "util", "def f():\n\treturn 1\n");
    let (f, command) = fake_pylint(f, &serde_json::Value::Array(messages));
    assert!(f.lint_with(&[("SCR900", &command)]).is_empty());
}

#[cfg(unix)]
#[test]
fn suppressions_apply_and_count_as_used() {
    let code = "def f():\n\tx = undefined_c  # iglint: ignore[SCR900]\n\treturn x\n";
    let (l, b, _) = position(code, "undefined_c");
    let messages = serde_json::json!([message(
        "undefined-variable",
        "E0602",
        "Undefined variable 'undefined_c'",
        (l, b),
        Some((l, b + 11))
    )]);
    let (f, command) = fake_pylint(library("m", code), &messages);
    assert!(
        f.lint_with(&[("SCR900", &command), ("SCR123", "")])
            .is_empty()
    );
    // Without pylint results the suppression is not judged.
    let d = f.lint_with(&[
        ("SCR900", "pylint = \"iglint-test-no-such-pylint\""),
        ("SCR123", ""),
    ]);
    assert!(d.is_empty(), "{d:?}");
}

#[test]
fn missing_pylint_is_one_notice() {
    let f = library("m", "def f():\n\treturn undefined_d\n");
    let f = with_library(f, "other", "x = 1\n");
    let result = f.run(
        &[
            ("SCR900", "pylint = \"iglint-test-no-such-pylint --flag\""),
            ("SCR101", ""),
        ],
        FixMode::None,
    );
    assert_eq!(result.notices.len(), 1, "{:?}", result.notices);
    assert!(
        result.notices[0].contains("`iglint-test-no-such-pylint --flag` was not found"),
        "{}",
        result.notices[0]
    );
    assert!(result.notices[0].contains("pip install pylint"));
    assert!(result.errors.is_empty());
    let codes: Vec<&str> = result.diagnostics.iter().map(|d| d.rule.code()).collect();
    assert_eq!(codes, ["SCR101"]);
}

#[cfg(unix)]
#[test]
fn failing_pylint_is_one_notice() {
    let f = library("m", "x = 1\n").file("fail.sh", "#!/bin/sh\necho 'bad option' >&2\nexit 32\n");
    let command = format!("pylint = \"/bin/sh {}\"", f.path("fail.sh").display());
    let result = f.run(&[("SCR900", &command)], FixMode::None);
    assert_eq!(result.notices.len(), 1);
    assert!(
        result.notices[0].contains("failed (exit status 32): bad option"),
        "{}",
        result.notices[0]
    );
}

#[test]
fn missing_pylintrc_falls_back_with_a_notice() {
    let f = library("m", "x = 1\n");
    let result = f.run(
        &[(
            "SCR900",
            "pylint = \"iglint-test-no-such-pylint\"\npylintrc = \"missing-rc\"",
        )],
        FixMode::None,
    );
    assert!(
        result.notices[0].contains("pylintrc") && result.notices[0].contains("not found"),
        "{:?}",
        result.notices
    );
}

#[cfg(unix)]
#[test]
fn fixes_move_pylint_findings_without_running_pylint_again() {
    let code = "import os\nx = undefined_e\n";
    let (l, b, _) = position(code, "undefined_e");
    let messages = serde_json::json!([message(
        "undefined-variable",
        "E0602",
        "Undefined variable 'undefined_e'",
        (l, b),
        Some((l, b + 11))
    )]);
    let (f, command) = fake_pylint(library("m", code), &messages);
    let result = f.run(
        &[("SCR900", &command), ("SCR102", "")],
        FixMode::DiffOnly {
            unsafe_fixes: false,
        },
    );
    assert_eq!(runs(&f), 1);
    assert_eq!(result.fixed.len(), 1);
    assert_eq!(result.fixed[0].fixed, "x = undefined_e\n");
    let d: Vec<&Diagnostic> = result.diagnostics.iter().collect();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].rule.code(), "SCR900");
    assert_eq!(d[0].range.slice(d[0].file.text()), "undefined_e");
}

/// Runs the real pylint named by `IGLINT_TEST_PYLINT` (for example
/// `/path/to/venv/bin/pylint`) on a hand-written project.
#[test]
fn real_pylint() {
    let Ok(pylint) = std::env::var("IGLINT_TEST_PYLINT") else {
        eprintln!("note: IGLINT_TEST_PYLINT is not set; skipping the real pylint test");
        return;
    };
    let body =
        "\tif self.props.text == None:\n\t\tsystem.perspective.print(util.f())\n\tprint \"py2\"";
    let f = view(ViewScript::Action, body);
    let f = with_library(f, "util", "import os\n\ndef f():\n\treturn xrange(3)\n");
    let f = f.resource(
        "projects/p/ignition/message/m",
        &[("handleMessage.py", "def handleMessage(payload):\n\tvalue = payload['x']\n\tif value == None:\n\t\treturn undefined_f\n")],
    );
    let options = format!("pylint = \"{pylint}\"\njobs = 1");
    let result = f.run(&[("SCR900", &options)], FixMode::None);
    assert!(result.notices.is_empty(), "{:?}", result.notices);
    let found: Vec<(String, String)> = decoded_flagged(&result.diagnostics)
        .into_iter()
        .zip(&result.diagnostics)
        .map(|(text, d)| (text, d.message.clone()))
        .collect();
    let has = |text: &str, symbol: &str| {
        found
            .iter()
            .any(|(t, m)| t == text && m.contains(&format!("({symbol}, ")))
    };
    // The view script with Python 2 syntax gives only E0001 in pylint,
    // which is dropped: Jython accepts it.
    assert!(
        !found.iter().any(|(_, m)| m.contains("syntax-error")),
        "{found:?}"
    );
    assert!(has("import os", "unused-import"), "{found:?}");
    assert!(has("value == None", "singleton-comparison"), "{found:?}");
    assert!(has("undefined_f", "undefined-variable"), "{found:?}");
    for name in ["system", "util", "xrange", "payload"] {
        assert!(
            !found.iter().any(|(_, m)| m.contains(&format!("'{name}'"))),
            "{name}: {found:?}"
        );
    }
    #[cfg(unix)]
    driver_and_command_agree(&pylint);
    // No temporary directory is left behind.
    let leftovers = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(&format!("iglint-pylint-{}-", std::process::id()))
        })
        .count();
    assert_eq!(leftovers, 0);
}

/// The forking driver (used for a `pylint` script or `python -m pylint`)
/// and the plain pylint command (`env pylint`, run with pylint's own
/// `--jobs`) find the same messages, with any number of processes. Part
/// of [`real_pylint`], which checks for leftover temporary directories.
#[cfg(unix)]
fn driver_and_command_agree(pylint: &str) {
    let mut f = with_library(
        view(
            ViewScript::Action,
            "\tif self.props.text == None:\n\t\tx = undefined_a",
        ),
        "util",
        "import os\n\ndef f():\n\treturn xrange(3)\n",
    );
    for i in 0..60 {
        f = with_library(
            f,
            &format!("m{i}"),
            &format!("def g{i}(a):\n\tif a == None:\n\t\treturn b{i}\n"),
        );
    }
    let messages = |command: &str, jobs: u32| -> Vec<String> {
        let options = format!("pylint = \"{command}\"\njobs = {jobs}");
        let result = f.run(&[("SCR900", &options)], FixMode::None);
        assert!(result.notices.is_empty(), "{:?}", result.notices);
        let mut out: Vec<String> = result
            .diagnostics
            .iter()
            .map(|d| format!("{} {:?} {}", d.file.path().display(), d.range, d.message))
            .collect();
        out.sort();
        out
    };
    let reference = messages(&format!("env {pylint}"), 1);
    assert!(reference.len() > 60, "{reference:?}");
    for jobs in [1, 3, 0] {
        assert_eq!(messages(pylint, jobs), reference, "jobs = {jobs}");
    }
}

#[test]
fn bundled_configuration_is_ign_lints() {
    let rc = run::BUNDLED_PYLINTRC;
    assert!(rc.contains("MIT License"));
    assert!(rc.contains("Copyright (c) 2024 Eric Knorr"));
    for disabled in [
        "unused-argument",
        "missing-docstring",
        "redefined-outer-name",
        "function-redefined",
    ] {
        assert!(rc.contains(&format!("        {disabled},\n")), "{disabled}");
    }
}

#[test]
fn quoted_names() {
    assert_eq!(quoted("Undefined variable 'abc'"), Some("abc"));
    assert_eq!(quoted("Unable to import 'a.b'"), Some("a.b"));
    assert_eq!(quoted("no quotes"), None);
}

/// ign-lint's trailing-whitespace fix: the whole body `rstrip()`ed per
/// line, attached to the script's first `C0303`; every `C0303` of the
/// script is resolved by it.
#[cfg(unix)]
#[test]
fn trailing_whitespace_fix_strips_every_line() {
    let body = "\treturn 1   \n\tx \u{3d} 2\t";
    let source = format!("def runAction(self, event):\n{body}");
    let messages = serde_json::json!([
        message(
            "trailing-whitespace",
            "C0303",
            "Trailing whitespace",
            (2, 9),
            None
        ),
        message(
            "trailing-whitespace",
            "C0303",
            "Trailing whitespace",
            (3, 7),
            None
        ),
    ]);
    assert_eq!(&source[source.find("1   ").unwrap() + 1..][..3], "   ");
    let (f, command) = fake_pylint(view(ViewScript::Action, body), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(d.len(), 2);
    assert_eq!(decoded_flagged(&d), ["   ", "\t"]);
    let fixes: Vec<_> = d.iter().map(|d| d.fix.is_some()).collect();
    assert_eq!(fixes, [true, false]);
    let result = f.run(&[("SCR900", &command)], FixMode::Safe);
    assert_eq!(result.fixed[0].applied.len(), 1);
    assert_eq!(
        result.fixed[0].applied[0].message,
        "Remove trailing whitespace from script at root.root.events.component.onActionPerformed"
    );
    assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    let fixed = std::fs::read_to_string(view_path(&f)).unwrap();
    assert!(
        fixed.contains(r#""script": "\treturn 1\n\tx \u003d 2""#),
        "{fixed}"
    );
}

/// Whitespace inside a multi-line string is data: the fix is unsafe.
#[cfg(unix)]
#[test]
fn trailing_whitespace_fix_in_strings_is_unsafe() {
    let code = "x = '''a  \nb'''\n";
    let messages = serde_json::json!([message(
        "trailing-whitespace",
        "C0303",
        "Trailing whitespace",
        (1, 8),
        None
    )]);
    let (f, command) = fake_pylint(library("m", code), &messages);
    let d = f.lint_with(&[("SCR900", &command)]);
    assert_eq!(
        d[0].fix.as_ref().map(|f| f.applicability),
        Some(crate::diagnostic::Applicability::Unsafe)
    );
}

/// A pylint message that a native rule reports on the same range (and
/// name) is dropped; others stay.
#[cfg(unix)]
#[test]
fn duplicates_of_native_findings_are_dropped() {
    let code = "def f():\n    return undefined_e\n";
    let (l, b, _) = position(code, "undefined_e");
    let messages = serde_json::json!([
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'undefined_e'",
            (l, b),
            Some((l, b + 11))
        ),
        message(
            "undefined-variable",
            "E0602",
            "Undefined variable 'other'",
            (l, b),
            Some((l, b + 11))
        ),
        message("line-too-long", "C0301", "Line too long", (l, 0), None),
    ]);
    let (f, command) = fake_pylint(library("m", code), &messages);
    let d = f.lint_with(&[("SCR900", &command), ("SCR101", "")]);
    let found: Vec<(&str, &str)> = d
        .iter()
        .map(|d| (d.rule.code(), d.message.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            ("SCR900", "Line too long (line-too-long, C0301)"),
            ("SCR101", "Undefined name `undefined_e`"),
            (
                "SCR900",
                "Undefined variable 'other' (undefined-variable, E0602)"
            ),
        ]
    );
}
