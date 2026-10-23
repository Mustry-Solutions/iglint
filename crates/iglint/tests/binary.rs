//! Tests of the installed binary: process exit codes, argv[0] dispatch.

use assert_cmd::Command;
use predicates::prelude::*;

fn iglint(dir: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("iglint").unwrap();
    command
        .current_dir(dir)
        .env_remove("CLICOLOR_FORCE")
        .env("NO_COLOR", "1");
    command
}

#[test]
fn clean_project_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("views/A")).unwrap();
    std::fs::write(dir.path().join("views/A/view.json"), "{}").unwrap();
    iglint(dir.path())
        .arg("--no-cache")
        .assert()
        .success()
        .stdout("All checks passed!\n");
    iglint(dir.path())
        .args(["check", "--show-files"])
        .assert()
        .success()
        .stdout("views/A/view.json\n");
}

#[test]
fn files_that_are_not_utf8_are_findings_not_errors() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("p/ignition/script-python/m");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(
        dir.path().join("p/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    std::fs::write(
        module.join("resource.json"),
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    )
    .unwrap();
    std::fs::write(module.join("code.py"), b"x = \"caf\xe9\"\n").unwrap();
    iglint(dir.path())
        .args(["--no-cache", "--output-format", "concise"])
        .assert()
        .code(1)
        .stderr("")
        .stdout(predicate::str::contains(
            "p/ignition/script-python/m/code.py:1:9: VIEW007 [error] File is not valid UTF-8",
        ))
        .stdout(predicate::str::contains("All checks passed").not());
}

#[test]
fn usage_and_configuration_errors_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    iglint(dir.path())
        .args(["--select", "NOPE"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown rule selector `NOPE`"));
    iglint(dir.path()).arg("--bogus").assert().code(2);
    std::fs::write(dir.path().join("iglint.toml"), "selct = []\n").unwrap();
    iglint(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "iglint.toml:1:1: unknown key `selct`; did you mean `select`?",
        ));
}

#[test]
fn help_and_version() {
    let dir = tempfile::tempdir().unwrap();
    iglint(dir.path())
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("iglint {}\n", env!("IGLINT_VERSION")));
    iglint(dir.path())
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ign-lint"));
    iglint(dir.path()).arg("rules").assert().success();
}

#[cfg(unix)]
#[test]
fn ign_lint_symlink_dispatches_to_compat_mode() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("ign-lint");
    std::os::unix::fs::symlink(assert_cmd::cargo::cargo_bin("iglint"), &link).unwrap();
    std::process::Command::new(&link)
        .arg("--version")
        .current_dir(dir.path())
        .output()
        .map(|output| {
            assert!(output.status.success());
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.starts_with("ign-lint "), "{stdout}");
        })
        .unwrap();
    // argparse-style usage errors exit 2.
    let output = std::process::Command::new(&link)
        .arg("--no-such-option")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// `iglint server` speaks LSP over the standard streams: the whole session
/// (initialize, shutdown, exit) arrives at once and every answer comes back.
#[test]
fn server_speaks_lsp_over_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let frame = |body: &str| format!("Content-Length: {}\r\n\r\n{body}", body.len());
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ]
    .map(frame)
    .concat();
    iglint(dir.path())
        .arg("server")
        .write_stdin(input)
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .success()
        .stdout(
            predicate::str::contains(r#""serverInfo":{"name":"iglint""#)
                .and(predicate::str::contains(r#""id":2,"result":null"#)),
        );
}

/// Every file `--show-files` lists is linted: given a syntax error, each
/// one gets a finding (or the run names it as skipped, with a reason). No
/// discovered file is silently dropped, whatever its place in the tree.
#[test]
fn every_discovered_file_is_linted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let bad_json = "{\"a\": 1,}";
    let bad_py = "def f(:\n    pass\n";
    let manifest = r#"{"scope": "G", "version": 1}"#;
    let perspective = "projects/p/com.inductiveautomation.perspective";
    write(
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true}"#,
    );
    write("projects/broken/project.json", bad_json);
    // Resources with their manifest.
    write(&format!("{perspective}/views/A/resource.json"), manifest);
    write(&format!("{perspective}/views/A/view.json"), bad_json);
    write(&format!("{perspective}/views/Bad/resource.json"), bad_json);
    write(&format!("{perspective}/views/Bad/view.json"), "{}");
    write(
        &format!("{perspective}/views/Unlisted/resource.json"),
        r#"{"scope": "G", "version": 1, "files": ["thumbnail.png"]}"#,
    );
    write(&format!("{perspective}/views/Unlisted/view.json"), bad_json);
    write(
        &format!("{perspective}/style-classes/S/resource.json"),
        manifest,
    );
    write(
        &format!("{perspective}/style-classes/S/style.json"),
        bad_json,
    );
    write(
        "projects/p/ignition/script-python/a/resource.json",
        manifest,
    );
    write("projects/p/ignition/script-python/a/code.py", bad_py);
    write("projects/p/ignition/timer/t/resource.json", manifest);
    write("projects/p/ignition/timer/t/handleTimerEvent.py", bad_py);
    write(
        "projects/p/com.inductiveautomation.webdev/resources/w/resource.json",
        manifest,
    );
    write(
        "projects/p/com.inductiveautomation.webdev/resources/w/doGet.py",
        bad_py,
    );
    write(
        "projects/p/com.inductiveautomation.reporting/reports/r/resource.json",
        manifest,
    );
    write(
        "projects/p/com.inductiveautomation.reporting/reports/r/data.json",
        bad_json,
    );
    write("projects/p/unknown.module/things/x/resource.json", manifest);
    write("projects/p/unknown.module/things/x/data.json", bad_json);
    write("projects/p/unknown.module/things/x/code.py", bad_py);
    // Resources without their manifest.
    write(
        &format!("{perspective}/views/NoManifest/view.json"),
        bad_json,
    );
    write("projects/p/ignition/script-python/b/code.py", bad_py);
    write("projects/p/ignition/timer/u/handleTimerEvent.py", bad_py);
    // Files outside every resource.
    write("projects/p/views/Stray/view.json", bad_json);
    write("projects/p/ignition/script-python/a/helper.py", bad_py);
    write("projects/p/loose.py", bad_py);
    // A gateway configuration collection.
    write("config/core/config-mode.json", "{}");
    write(
        "config/core/ignition/database-connection/db/resource.json",
        manifest,
    );
    write(
        "config/core/ignition/database-connection/db/config.json",
        bad_json,
    );
    write(
        "config/core/ignition/tag-definition/default/unary-resource.json",
        manifest,
    );
    write(
        "config/core/ignition/tag-definition/default/tags.json",
        bad_json,
    );
    // Outside any project.
    write("views/Loose/view.json", bad_json);

    let listed = iglint(root)
        .args(["check", "--show-files"])
        .output()
        .unwrap();
    let listed = String::from_utf8(listed.stdout).unwrap();
    let files: Vec<&str> = listed.lines().collect();
    assert!(files.len() >= 25, "{listed}");
    let checked = iglint(root)
        .args([
            "check",
            "--no-cache",
            "--output-format",
            "concise",
            "--select",
            "VIEW001,SCR001",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8(checked.stdout).unwrap();
    let stderr = String::from_utf8(checked.stderr).unwrap();
    let mut missing = Vec::new();
    for file in &files {
        let has_finding = stdout.lines().any(|l| l.starts_with(&format!("{file}:")));
        let skipped = stderr.lines().any(|l| l.contains(file));
        // Valid files of the tree: they have nothing to report.
        let valid = file.ends_with("project.json") && !file.contains("broken")
            || file.ends_with("config-mode.json")
            || file.ends_with("Bad/view.json")
            || (file.ends_with("resource.json") && !file.contains("/Bad/"));
        if !valid && !has_finding && !skipped {
            missing.push(*file);
        }
    }
    assert!(
        missing.is_empty(),
        "not linted: {missing:#?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

/// Reviewer round 8 (Codex) blocker, through the command line: a function
/// defined before `import re` reaches `eval` through `re.sys`, so `--fix`
/// must keep `import math` (Jython prints `3.14159265359` before and raises
/// `NameError` after removing it). Without the capture the safe fix runs.
#[test]
fn fix_keeps_imports_read_by_functions_defined_before_an_unvetted_import() {
    let exploit = "def capture():\n    return re.sys.modules['__builtin__']\nimport re\ndef target():\n    import math\n    print capture().eval('math.pi')\ntarget()\n";
    let benign = "import re\ndef target():\n    import math\n    print re.escape('a')\ntarget()\n";
    for (code, removed) in [(exploit, false), (benign, true)] {
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("p/ignition/script-python/m");
        std::fs::create_dir_all(&module).unwrap();
        std::fs::write(
            dir.path().join("p/project.json"),
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        )
        .unwrap();
        std::fs::write(
            module.join("resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        )
        .unwrap();
        std::fs::write(module.join("code.py"), code).unwrap();
        iglint(dir.path())
            .args(["--no-cache", "--fix", "--select", "SCR102"])
            .assert()
            .stderr("");
        let after = std::fs::read_to_string(module.join("code.py")).unwrap();
        assert_eq!(!after.contains("import math"), removed, "{after}");
        assert_eq!(after.lines().count(), code.lines().count(), "{after}");
    }
}

/// Reviewer round 8, finding 2: an unsafe fix says why and where, in
/// every output format, and the closed-world site stays current when the
/// finding comes from the cache.
#[test]
fn unsafe_fixes_name_the_code_that_makes_them_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("p/ignition/script-python");
    std::fs::create_dir_all(dir.path().join("p")).unwrap();
    std::fs::write(
        dir.path().join("p/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    for (name, code) in [
        ("c", "def g():\n    import os\n    return 1\n"),
        ("m", "import re\nx = re.sys\n"),
    ] {
        std::fs::create_dir_all(lib.join(name)).unwrap();
        std::fs::write(
            lib.join(name).join("resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        )
        .unwrap();
        std::fs::write(lib.join(name).join("code.py"), code).unwrap();
    }
    let run = |format: &str| -> String {
        let out = iglint(dir.path())
            .args([
                "--select",
                "SCR102",
                "--cache-dir",
                "cache",
                "--output-format",
                format,
                "p/ignition/script-python/c/code.py",
            ])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let reason = |json: &str| -> serde_json::Value {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        v[0]["fix"]["unsafe_reason"].clone()
    };
    let first = run("json");
    let r = reason(&first);
    assert_eq!(
        r["message"],
        "closed world: code in the workspace is not vetted, reads frames or reaches other views, first"
    );
    assert_eq!(r["what"], "re.sys");
    assert_eq!(r["filename"], "p/ignition/script-python/m/code.py");
    assert_eq!(r["location"]["row"], 2);
    // Deterministic, also from the cache.
    let second = run("json");
    assert_eq!(second, first, "{first}\n----\n{second}");
    let text = run("text");
    assert!(
        text.contains(
            "note: the fix is unsafe because closed world: code in the workspace is not vetted, \
             reads frames or reaches other views, first: `re.sys` at \
             p/ignition/script-python/m/code.py:2:5"
        ),
        "{text}"
    );
    let sarif: serde_json::Value = serde_json::from_str(&run("sarif")).unwrap();
    let related = &sarif["runs"][0]["results"][0]["relatedLocations"][0];
    assert_eq!(
        related["physicalLocation"]["artifactLocation"]["uri"],
        "p/ignition/script-python/m/code.py"
    );
    assert_eq!(related["physicalLocation"]["region"]["startLine"], 2);
    // The site moves without changing any summary: the cached finding of
    // `c` names the new line.
    std::fs::write(lib.join("m/code.py"), "\nimport re\nx = re.sys\n").unwrap();
    assert_eq!(reason(&run("json"))["location"]["row"], 3);
}

/// Reviewer round 9 (Opus) blocker, through the binary: view `B` reads
/// view `A`'s custom property `foo` through `self.page.views`; `--fix`
/// must not delete it, `--fix --unsafe-fixes` may, and the JSON output
/// names the site in `B`.
#[test]
fn fix_keeps_properties_read_from_another_view() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("p")).unwrap();
    std::fs::write(
        dir.path().join("p/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    let view = |custom: &str, script: &str| {
        format!(
            r#"{{"custom": {custom}, "params": {{}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{{"meta": {{"name": "Btn"}}, "type": "ia.input.button", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}}}]}}}}"#
        )
    };
    let views = dir
        .path()
        .join("p/com.inductiveautomation.perspective/views");
    for (name, json) in [
        (
            "A",
            view(
                r#"{"foo": 1, "bar": 2}"#,
                r"\tsystem.perspective.print(self.view.custom.bar)",
            ),
        ),
        (
            "B",
            view(
                "{}",
                r"\tfor v in self.page.views:\n\t\tif v.id.startswith('A@'):\n\t\t\tsystem.perspective.print(v.custom.foo)",
            ),
        ),
    ] {
        std::fs::create_dir_all(views.join(name)).unwrap();
        std::fs::write(
            views.join(name).join("resource.json"),
            r#"{"scope": "G", "version": 1, "files": ["view.json"]}"#,
        )
        .unwrap();
        std::fs::write(views.join(name).join("view.json"), json).unwrap();
    }
    let a = views.join("A/view.json");
    let out = iglint(dir.path())
        .args([
            "--no-cache",
            "--select",
            "VIEW015",
            "--output-format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let foo = v
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("`foo`"))
        .unwrap();
    assert_eq!(foo["fix"]["applicability"], "unsafe", "{foo}");
    let reason = &foo["fix"]["unsafe_reason"];
    assert_eq!(
        reason["filename"], "p/com.inductiveautomation.perspective/views/B/view.json",
        "{reason}"
    );
    assert_eq!(reason["what"], "self.page.views", "{reason}");
    iglint(dir.path())
        .args(["--no-cache", "--fix", "--select", "VIEW015"])
        .output()
        .unwrap();
    assert!(std::fs::read_to_string(&a).unwrap().contains("\"foo\""));
    iglint(dir.path())
        .args([
            "--no-cache",
            "--fix",
            "--unsafe-fixes",
            "--select",
            "VIEW015",
        ])
        .output()
        .unwrap();
    assert!(!std::fs::read_to_string(&a).unwrap().contains("\"foo\""));
}
