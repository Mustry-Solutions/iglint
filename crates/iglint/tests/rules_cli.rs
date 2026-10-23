//! `iglint check` and `iglint ign-lint` with the real rules (no fake
//! analyzer), on hand-written projects and on the ign-lint fixtures.

use std::path::Path;

use iglint::{Environment, ExitStatus, Io};
use iglint_linter::rules::REGISTRY;

struct Run {
    status: ExitStatus,
    stdout: String,
}

fn environment(cwd: &Path) -> Environment {
    Environment {
        registry: &REGISTRY,
        analyzer: None,
        cwd: cwd.to_path_buf(),
        vars: Vec::new(),
    }
}

fn run_with(cwd: &Path, args: &[&str], ign_lint: bool) -> Run {
    let environment = environment(cwd);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdin: &[u8] = &[];
    let mut io = Io {
        stdout: &mut stdout,
        stderr: &mut stderr,
        stdin: &mut stdin,
        stdout_is_terminal: false,
    };
    let mut argv = vec![std::ffi::OsString::from(if ign_lint {
        "ign-lint"
    } else {
        "iglint"
    })];
    argv.extend(args.iter().map(std::ffi::OsString::from));
    let status = if ign_lint {
        iglint::ign_lint::run(argv, &environment, &mut io)
    } else {
        iglint::run(argv, &environment, &mut io)
    };
    let stderr = String::from_utf8(stderr).unwrap();
    assert!(!stderr.contains("panicked"), "{stderr}");
    Run {
        status,
        stdout: String::from_utf8(stdout).unwrap(),
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

const MANIFEST: &str = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;

/// A project with one view that references a missing view, a missing
/// property and has an unused custom property.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(
        d,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    let views = "projects/p/com.inductiveautomation.perspective/views";
    write(d, &format!("{views}/Main/resource.json"), MANIFEST);
    write(
        d,
        &format!("{views}/Main/view.json"),
        r#"{
  "custom": {
    "unused": 1
  },
  "root": {
    "children": [
      {
        "meta": {
          "name": "Embed"
        },
        "props": {
          "path": "Missing/View"
        },
        "type": "ia.display.view"
      },
      {
        "meta": {
          "name": "Label"
        },
        "propConfig": {
          "props.text": {
            "binding": {
              "config": {
                "expression": "{view.params.nope}"
              },
              "type": "expr"
            }
          }
        },
        "type": "ia.display.label"
      }
    ],
    "meta": {
      "name": "root"
    },
    "type": "ia.container.flex"
  }
}"#,
    );
    dir
}

#[test]
fn check_reports_real_findings() {
    let dir = project();
    let run = run_with(
        dir.path(),
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--output-format",
            "concise",
        ],
        false,
    );
    assert_eq!(run.status, ExitStatus::Failure, "{}", run.stdout);
    for code in ["REF001", "BIND005", "VIEW015"] {
        assert!(run.stdout.contains(code), "{code} missing:\n{}", run.stdout);
    }
    // Every finding points into the view file with a position.
    for line in run.stdout.lines().filter(|l| l.contains("view.json:")) {
        assert!(
            line.split(':')
                .nth(1)
                .is_some_and(|n| n.parse::<u32>().is_ok()),
            "{line}"
        );
    }
}

#[test]
fn check_fix_removes_unused_custom_property() {
    let dir = project();
    let run = run_with(
        dir.path(),
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--select",
            "VIEW015",
            "--fix",
        ],
        false,
    );
    assert_eq!(run.status, ExitStatus::Success, "{}", run.stdout);
    let text = std::fs::read_to_string(
        dir.path()
            .join("projects/p/com.inductiveautomation.perspective/views/Main/view.json"),
    )
    .unwrap();
    assert!(!text.contains("unused"), "{text}");
    assert!(
        text.starts_with("{\n  \"custom\": {},\n  \"root\": {"),
        "{text}"
    );
    // Re-running finds nothing.
    let again = run_with(
        dir.path(),
        &["check", "--isolated", "--no-cache", "--select", "VIEW015"],
        false,
    );
    assert_eq!(again.status, ExitStatus::Success, "{}", again.stdout);
}

/// A view whose button hands itself to `lib.util.handle`, which `util`
/// defines as `def handle(p): pass`, plus `rest` appended to the module and
/// `other` (a second library module, `lib.other`) when not empty.
fn handed_to_library(rest: &str, other: &str) -> tempfile::TempDir {
    library_view(
        "\\tlib.util.handle(self)\\n",
        &format!("def handle(p):\n    pass\n{rest}"),
        other,
    )
}

/// A view whose root button (custom property `used`) runs `script` (JSON
/// escaped), in a project whose library has `lib.util` with `util` and
/// `lib.other` with `other` when not empty.
fn library_view(script: &str, util: &str, other: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    write(
        d,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    let views = "projects/p/com.inductiveautomation.perspective/views";
    write(d, &format!("{views}/Main/resource.json"), MANIFEST);
    write(
        d,
        &format!("{views}/Main/view.json"),
        &r#"{
  "root": {
    "custom": {
      "used": 42
    },
    "events": {
      "component": {
        "onActionPerformed": {
          "config": {
            "script": "SCRIPT"
          },
          "scope": "G",
          "type": "script"
        }
      }
    },
    "meta": {
      "name": "root"
    },
    "type": "ia.input.button"
  }
}"#
        .replace("SCRIPT", script),
    );
    let library = "projects/p/ignition/script-python/lib";
    write(d, &format!("{library}/util/resource.json"), "{}\n");
    write(d, &format!("{library}/util/code.py"), util);
    if !other.is_empty() {
        write(d, &format!("{library}/other/resource.json"), "{}\n");
        write(d, &format!("{library}/other/code.py"), other);
    }
    dir
}

/// Every way of giving the module another `handle` after its `def` keeps
/// the property: the safe fix must not remove what the function that
/// actually runs reads. Each variant prints 42 on Jython (`p.custom.used`
/// of a stand-in object).
#[test]
fn safe_fixes_keep_properties_of_rebound_library_functions() {
    let new_handle = "def _h(p):\n    print p.custom.used\n";
    let rebinding = [
        ("exec 'def handle(p): print p.custom.used'\n".to_owned(), String::new()),
        (
            "def _setup():\n    exec 'global handle\\ndef handle(p): print p.custom.used'\n_setup()\n"
                .to_owned(),
            String::new(),
        ),
        (format!("{new_handle}globals()['handle'] = _h\n"), String::new()),
        (
            format!("import sys\n{new_handle}setattr(sys.modules[__name__], 'handle', _h)\n"),
            String::new(),
        ),
        (
            format!("import sys\n{new_handle}sys.modules[__name__].handle = _h\n"),
            String::new(),
        ),
        (format!("{new_handle}handle = _h\n"), String::new()),
        ("def handle(p):\n    print p.custom.used\n".to_owned(), String::new()),
        (format!("{new_handle}handle.func_code = _h.func_code\n"), String::new()),
        ("eval(\"globals().update(handle=lambda p: p.custom.used)\")\n".to_owned(), String::new()),
        // Another module rebinds it.
        (String::new(), format!("{new_handle}lib.util.handle = _h\n")),
        (String::new(), format!("{new_handle}setattr(lib.util, 'handle', _h)\n")),
        (String::new(), format!("{new_handle}lib.util.__dict__['handle'] = _h\n")),
        (
            String::new(),
            format!("import sys\n{new_handle}sys.modules['lib.util'].handle = _h\n"),
        ),
        (String::new(), format!("{new_handle}exec 'lib.util.handle = _h'\n")),
        (String::new(), format!("{new_handle}m = lib.util\nm.handle = _h\n")),
        // Through `importlib` and function globals.
        (
            String::new(),
            format!("import importlib\n{new_handle}importlib.import_module('lib.util').handle = _h\n"),
        ),
        (
            "def _x():\n    pass\n".to_owned(),
            format!("{new_handle}lib.util._x.func_globals['handle'] = _h\n"),
        ),
        (
            "def _x():\n    pass\n".to_owned(),
            format!("{new_handle}lib.util._x.__globals__['handle'] = _h\n"),
        ),
        // The function object changed through an alias or a helper.
        (format!("{new_handle}alias = handle\nalias.func_code = _h.func_code\n"), String::new()),
        (
            format!("{new_handle}alias = handle\nsetattr(alias, 'func_code', _h.func_code)\n"),
            String::new(),
        ),
        (
            format!("{new_handle}def _patch(f):\n    f.func_code = _h.func_code\n_patch(handle)\n"),
            String::new(),
        ),
        (
            String::new(),
            format!("{new_handle}def _patch(f):\n    f.func_code = _h.func_code\n_patch(lib.util.handle)\n"),
        ),
    ];
    let view = "projects/p/com.inductiveautomation.perspective/views/Main/view.json";
    for (rest, other) in &rebinding {
        let dir = handed_to_library(rest, other);
        assert_fix_keeps_used(dir.path(), &format!("{rest}{other}"));
    }
    // A function used as a value elsewhere may be changed by whoever
    // receives it: it is not followed, so the property counts as read.
    let dir = handed_to_library("", "def g():\n    return lib.util.handle\n");
    assert_fix_keeps_used(dir.path(), "returned");
    // The function that is not rebound is followed: it reads nothing. The
    // finding relies on following the call into the library, so its fix
    // is unsafe: `--fix` keeps the property, `--unsafe-fixes` removes it.
    let dir = handed_to_library("", "");
    let args = ["check", "--isolated", "--no-cache", "--select", "VIEW015"];
    run_with(dir.path(), &[&args[..], &["--fix"]].concat(), false);
    let text = std::fs::read_to_string(dir.path().join(view)).unwrap();
    assert!(text.contains("\"used\": 42"), "{text}");
    run_with(
        dir.path(),
        &[&args[..], &["--fix", "--unsafe-fixes"]].concat(),
        false,
    );
    let text = std::fs::read_to_string(dir.path().join(view)).unwrap();
    assert!(!text.contains("used"), "{text}");
}

/// `iglint check --fix --unsafe-fixes` on `dir` (a [`library_view`]) keeps
/// `used`, and `--diff` offers no fix for it, not even an unsafe one: the
/// property counts as read.
fn assert_fix_keeps_used(dir: &Path, case: &str) {
    let args = [
        "check",
        "--isolated",
        "--no-cache",
        "--select",
        "VIEW015",
        "--unsafe-fixes",
    ];
    let diff = run_with(dir, &[&args[..], &["--diff"]].concat(), false);
    assert!(!diff.stdout.contains("used"), "{case}\n{}", diff.stdout);
    let run = run_with(dir, &[&args[..], &["--fix"]].concat(), false);
    let view = "projects/p/com.inductiveautomation.perspective/views/Main/view.json";
    let text = std::fs::read_to_string(dir.join(view)).unwrap();
    assert!(
        text.contains("\"used\": 42"),
        "{case}\n{}\n{text}",
        run.stdout
    );
}

/// A library function that stores the component in a module global (or
/// anywhere else that outlives the call) lets later code read it: the safe
/// fix keeps the property. Each variant prints 42 on Jython.
#[test]
fn safe_fixes_keep_properties_stored_by_library_functions() {
    let later = "def later():\n    print _c.custom.used\n";
    for util in [
        format!("_c = None\ndef handle(c):\n    global _c\n    _c = c\n{later}"),
        format!("def handle(c):\n    global _c\n    v = c\n    _c = v\n{later}"),
        "_s = []\ndef handle(c):\n    _s.append(c)\ndef later():\n    print _s[0].custom.used\n"
            .to_owned(),
        "import lib.other as m\ndef handle(c):\n    m._c = c\ndef later():\n    print m._c.custom.used\n"
            .to_owned(),
        "def handle(c, cache={}):\n    cache['c'] = c\ndef later():\n    print handle.func_defaults[0]['c'].custom.used\n"
            .to_owned(),
    ] {
        let dir = library_view(
            "\\tlib.util.handle(self)\\n\\tlib.util.later()\\n",
            &util,
            "_c = None\n",
        );
        assert_fix_keeps_used(dir.path(), &util);
    }
}

/// A library function the view script itself, or another module through an
/// alias, replaces before the call: the function that runs reads the
/// property. Each variant prints 42 on Jython.
#[test]
fn safe_fixes_keep_properties_of_functions_replaced_by_callers() {
    let util = "def handle(c):\n    pass\n";
    let real = "def real(c):\n    print c.custom.used\n";
    let cases = [
        (
            "\\tdef real(c):\\n\\t\\tprint c.custom.used\\n\\tlib.util.handle = real\\n\\tlib.util.handle(self)\\n",
            util.to_owned(),
            String::new(),
        ),
        (
            "\\timport lib.util as u\\n\\tdef real(c):\\n\\t\\tprint c.custom.used\\n\\tu.handle = real\\n\\tlib.util.handle(self)\\n",
            util.to_owned(),
            String::new(),
        ),
        (
            "\\tdef real(c):\\n\\t\\tprint c.custom.used\\n\\tsetattr(lib.util, 'handle', real)\\n\\tlib.util.handle(self)\\n",
            util.to_owned(),
            String::new(),
        ),
        (
            "\\tlib.util.handle(self)\\n",
            util.to_owned(),
            format!("import lib.util as u\n{real}u.handle = real\n"),
        ),
        (
            "\\tlib.util.handle(self)\\n",
            util.to_owned(),
            format!("import lib.util as u\n{real}setattr(u, 'handle', real)\n"),
        ),
        (
            "\\tlib.util.handle(self)\\n",
            format!("{util}from lib.other import *\n"),
            "def handle(c):\n    print c.custom.used\n".to_owned(),
        ),
    ];
    for (script, util, other) in &cases {
        let dir = library_view(script, util, other);
        assert_fix_keeps_used(dir.path(), &format!("{script}\n{util}\n{other}"));
    }
}

/// Reads of the property that the library function makes in forms the
/// analysis must not lose: a property read into a local that is never
/// used (also where the read only checks that it exists), a property
/// called, read through an alias of the container, and the wrapper's
/// methods used as values. Each variant reads `used` on Jython.
#[test]
fn library_reads_through_locals_and_methods_keep_the_property() {
    for util in [
        "def handle(c):\n    x = c.custom.used\n",
        "def handle(c):\n    try:\n        x = c.custom.used\n    except AttributeError:\n        return False\n    return True\n",
        "def handle(c):\n    f = c.custom.used\n    f()\n",
        "def handle(c):\n    cu = c.custom\n    x = cu.used\n",
        "def handle(c):\n    print map(c.custom.get, ['used'])\n",
        "def handle(c):\n    g = c.custom.get\n    print g('used')\n",
        "def handle(c):\n    system.util.invokeAsynchronous(c.custom.keys)\n",
        "def handle(c):\n    print 'used' in c.custom\n",
        "def handle(c):\n    print c.custom.get('used')\n",
    ] {
        let dir = library_view("\\tlib.util.handle(self)\\n", util, "");
        assert_fix_keeps_used(dir.path(), util);
    }
}

/// The same forms in the view script itself.
#[test]
fn view_reads_through_methods_keep_the_property() {
    for script in [
        "\\tprint map(self.custom.get, ['used'])\\n",
        "\\tg = self.custom.get\\n\\tprint g('used')\\n",
        "\\tsystem.util.invokeAsynchronous(self.custom.keys)\\n",
        "\\tprint self.custom.get('used')\\n",
        "\\tf = self.getSibling\\n",
    ] {
        let dir = library_view(script, "", "");
        assert_fix_keeps_used(dir.path(), script);
    }
}

/// Frame builtins under other names read the property through the frame:
/// qualified (`__builtin__.eval`), imported under another name, or
/// aliased in another library module. Each variant reads `used` on
/// Jython.
#[test]
fn frame_builtins_under_other_names_keep_the_property() {
    let cases = [
        (
            "\\tlib.util.handle(self)\\n",
            "import __builtin__\ndef handle(p):\n    print __builtin__.eval('p.custom.used')\n",
            "",
        ),
        (
            "\\tlib.util.handle(self)\\n",
            "def handle(p):\n    from __builtin__ import eval as ev\n    print ev('p.custom.used')\n",
            "",
        ),
        (
            "\\tlib.util.handle(self)\\n",
            "import lib.other\ndef handle(p):\n    print lib.other.ev('p.custom.used')\n",
            "ev = eval\n",
        ),
        (
            "\\tprint lib.other.ev('self.custom.used')\\n",
            "",
            "ev = eval\n",
        ),
        (
            "\\tfrom lib.other import ev\\n\\tprint ev('self.custom.used')\\n",
            "",
            "ev = eval\n",
        ),
        (
            "\\tf = lib.other.loc\\n\\tprint f()['self'].custom.used\\n",
            "",
            "loc = locals\n",
        ),
    ];
    for (script, util, other) in cases {
        let dir = library_view(script, util, other);
        assert_fix_keeps_used(dir.path(), &format!("{script}\n{util}\n{other}"));
    }
}

/// A property the view's own scripts prove unused keeps its safe fix,
/// also when the script calls library functions (checked to be plain
/// functions), and `--fix` removes it.
#[test]
fn view_local_findings_keep_safe_fixes() {
    let dir = library_view(
        "\\tx = self.custom.get('other')\\n\\tlib.util.log('clicked')\\n\\timport json\\n\\tjson.dumps(1)\\n",
        "def log(m):\n    print m\n",
        "",
    );
    let args = ["check", "--isolated", "--no-cache", "--select", "VIEW015"];
    let run = run_with(dir.path(), &[&args[..], &["--fix"]].concat(), false);
    let view = "projects/p/com.inductiveautomation.perspective/views/Main/view.json";
    let text = std::fs::read_to_string(dir.path().join(view)).unwrap();
    assert!(!text.contains("used"), "{}\n{text}", run.stdout);
    // The library name that is not a plain function makes the view keep it.
    let dir = library_view(
        "\\tlib.util.log('clicked')\\n",
        "log = lib.other.make()\n",
        "def make():\n    return eval\n",
    );
    assert_fix_keeps_used(dir.path(), "variable");
}

#[test]
fn ign_lint_mode_prints_ign_lint_paths_and_wording() {
    let dir = tempfile::tempdir().unwrap();
    let case = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iglint_linter/tests/fixtures/ign-lint/cases/PascalCase/view.json");
    write(
        dir.path(),
        "view.json",
        &std::fs::read_to_string(case).unwrap(),
    );
    write(dir.path(), "rule_config.json", "{}");
    let run = run_with(dir.path(), &["view.json"], true);
    assert_eq!(run.status, ExitStatus::Failure);
    assert!(
        run.stdout.contains(
            "custom.CustomViewParam: custom property 'CustomViewParam' is defined but never referenced"
        ),
        "{}",
        run.stdout
    );
}

/// Every finding of `iglint ign-lint` carries an ign-lint node path
/// (`root.root.children[0].Name...`, `propConfig.custom.x`, `custom.x`),
/// never iglint's own (`root/Name.props.x@binding...`), whatever binding
/// or expression location it comes from: every rule with an ign-lint
/// equivalent, on every ign-lint fixture view and on a view with an
/// unresolved reference, a traversal and a fast poll in each expression
/// location iglint models.
#[test]
fn ign_lint_mode_uses_ign_lint_paths_everywhere() {
    let bad = r#"toStr({../Missing.props.text}) + dateFormat(now(100), \"HH\")"#;
    let view = r#"{
  "custom": {"used": 1},
  "params": {},
  "propConfig": {
    "custom.used": {"binding": {"type": "expr", "config": {"expression": "BAD"}}}
  },
  "props": {},
  "root": {
    "type": "ia.container.flex",
    "meta": {"name": "root"},
    "children": [
      {"type": "ia.display.label", "meta": {"name": "Target"}, "props": {"text": "x"}},
      {
        "type": "ia.display.label",
        "meta": {"name": "Host"},
        "propConfig": {
          "props.text": {"binding": {"type": "query", "config": {"queryPath": "Q", "parameters": {"p": "BAD"}, "polling": {"enabled": true, "rate": "BAD"}}}},
          "meta.visible": {"binding": {"type": "expr-struct", "config": {"struct": {"a": "BAD", "b": {"c": "BAD"}}, "waitOnAll": true}}},
          "props.style.color": {"binding": {"type": "tag", "config": {"mode": "expression", "tagPath": "BAD", "fallbackDelay": 2.5}}},
          "props.style.opacity": {"binding": {"type": "tag-history", "config": {"tags": "BAD", "dateRange": {"mostRecent": "1", "mostRecentUnits": "HOUR"}, "returnFormat": "Wide", "polling": {"enabled": true, "rate": "BAD"}}}},
          "props.style.width": {"binding": {"type": "http", "config": {"request": {"url": "BAD", "method": "GET"}, "polling": {"enabled": true, "rate": "BAD"}}}},
          "props.style.height": {"binding": {"type": "property", "config": {"path": "../Missing.props.text"}, "transforms": [
            {"type": "expression", "expression": "BAD"},
            {"type": "map", "inputType": "expression", "outputType": "expression", "mappings": [{"input": "BAD", "output": "BAD"}], "fallback": ""},
            {"type": "script", "code": "\tself.getSibling(\"Missing\")\n\treturn value"}
          ]}}
        },
        "events": {"dom": {"onClick": {"type": "script", "scope": "G", "config": {"script": "\tself.getSibling(\"Missing\").props.text = 1"}}}},
        "scripts": {"customMethods": [{"name": "m", "params": [], "script": "\tself.getSibling(\"Missing\")"}], "messageHandlers": [{"messageType": "t", "pageScope": true, "sessionScope": false, "viewScope": false, "script": "\tself.getSibling(\"Missing\")"}], "extensionFunctions": null}
      }
    ]
  }
}"#
    .replace("BAD", bad);
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "probe/view.json", &view);
    let cases =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../iglint_linter/tests/fixtures/ign-lint");
    for sub in ["cases", "probes"] {
        for entry in std::fs::read_dir(cases.join(sub)).unwrap() {
            let entry = entry.unwrap();
            let file = entry.path().join("view.json");
            if file.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                write(
                    dir.path(),
                    &format!("{sub}-{name}/view.json"),
                    &std::fs::read_to_string(file).unwrap(),
                );
            }
        }
    }
    write(
        dir.path(),
        "rule_config.json",
        r#"{"PylintScriptRule": {"enabled": false}}"#,
    );
    let run = run_with(dir.path(), &[], true);
    let mut findings = 0;
    for line in run.stdout.lines() {
        let Some(message) = line.strip_prefix("    • ") else {
            continue;
        };
        findings += 1;
        let path = message.split(": ").next().unwrap();
        assert!(
            !path.contains('/') && !path.contains('@'),
            "not an ign-lint path: {line}"
        );
    }
    assert!(findings > 100, "{}", run.stdout);
    // The probe view's query parameter is reported with the binding's
    // ign-lint path and the parameter name.
    assert!(
        run.stdout.contains(
            "root.root.children[1].Host.propConfig.props.text.p: Expression references non-existent component 'Missing'"
        ),
        "{}",
        run.stdout
    );
}

/// A legacy PylintScriptRule configuration disables what ign-lint's
/// combined-file header disables, also with a pylintrc of its own, and
/// `iglint migrate` writes that into iglint.toml.
#[test]
fn legacy_pylint_configuration_keeps_ign_lints_header_disables() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "rule_config.json",
        r#"{"PylintScriptRule": {"kwargs": {"pylintrc": "rc"}}}"#,
    );
    write(dir.path(), "rc", "[MAIN]\n");
    let run = run_with(dir.path(), &["migrate", "--dry-run"], false);
    assert!(
        run.stdout.contains(
            r#"disable = ["unused-argument", "missing-docstring", "redefined-outer-name", "function-redefined"]"#
        ),
        "{}",
        run.stdout
    );
}
