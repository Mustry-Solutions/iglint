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

/// A workspace whose scripts read their own files (reviewer round 10,
/// Codex blocker): `m` reads its own source after `import math`, `r` reads
/// the last line of `m` (a comment with trailing whitespace), and view
/// `Main` reads its own `view.json`.
fn file_reading_workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    let library = |name: &str, code: &str| {
        write(
            &format!("p/ignition/script-python/{name}/resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        );
        write(&format!("p/ignition/script-python/{name}/code.py"), code);
    };
    library(
        "m",
        "import math\nprint open('/w/projects/p/ignition/script-python/m/code.py').read().startswith('import math\\n')\nx = 1  # note  \n",
    );
    library(
        "r",
        "def last(p):\n    return open(p).read().splitlines()[-1]\n",
    );
    write(
        "p/com.inductiveautomation.perspective/views/Main/resource.json",
        r#"{"scope": "G", "version": 1, "files": ["view.json"]}"#,
    );
    write(
        "p/com.inductiveautomation.perspective/views/Main/view.json",
        r#"{"custom": {"unused": 1}, "params": {}, "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [{"meta": {"name": "Btn"}, "type": "ia.input.button", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\timport json\n\td = json.loads(open('/usr/local/bin/ignition/data/projects/p/com.inductiveautomation.perspective/views/Main/view.json').read())\n\tsystem.perspective.print('unused' in d['custom'])"}}}}}]}}"#,
    );
    dir
}

#[test]
fn fixes_are_unsafe_when_scripts_read_workspace_files() {
    let dir = file_reading_workspace();
    let m = dir.path().join("p/ignition/script-python/m/code.py");
    let view = dir
        .path()
        .join("p/com.inductiveautomation.perspective/views/Main/view.json");
    let before = (
        std::fs::read_to_string(&m).unwrap(),
        std::fs::read_to_string(&view).unwrap(),
    );
    let select = ["--select", "SCR102,SCR124,VIEW015"];
    // With and without the cache (the second cached run restores findings).
    for cache in ["--no-cache", "--cache-dir=.cache", "--cache-dir=.cache"] {
        let out = iglint(dir.path())
            .args([cache, "--output-format", "json"])
            .args(select)
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let found = v.as_array().unwrap();
        let mut rules: Vec<&str> = found.iter().map(|d| d["code"].as_str().unwrap()).collect();
        rules.sort_unstable();
        assert_eq!(rules, ["SCR102", "SCR124", "VIEW015"], "{v}");
        for d in found {
            assert_eq!(d["fix"]["applicability"], "unsafe", "{d}");
            let reason = &d["fix"]["unsafe_reason"];
            assert!(
                reason["message"]
                    .as_str()
                    .unwrap()
                    .contains("assume-scripts-do-not-read-workspace-files"),
                "{reason}"
            );
            assert!(
                reason["what"].as_str().unwrap().starts_with("open("),
                "{reason}"
            );
        }
    }
    iglint(dir.path())
        .args(["--no-cache", "--fix"])
        .args(select)
        .output()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&m).unwrap(), before.0);
    assert_eq!(std::fs::read_to_string(&view).unwrap(), before.1);
    // The text output names the read and the setting.
    let out = iglint(dir.path())
        .args(["--no-cache", "--select", "SCR124"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "note: the fix is unsafe because code in the workspace may read the files a fix edits"
        ),
        "{text}"
    );
    // The project declares that no script reads workspace files: safe
    // fixes apply again (the closed world still decides SCR102 and VIEW015).
    std::fs::write(
        dir.path().join("iglint.toml"),
        "[fix]\nassume-scripts-do-not-read-workspace-files = true\n",
    )
    .unwrap();
    iglint(dir.path())
        .args(["--no-cache", "--fix"])
        .args(select)
        .output()
        .unwrap();
    let fixed = std::fs::read_to_string(&m).unwrap();
    assert!(fixed.ends_with("x = 1  # note\n"), "{fixed}");
    assert!(fixed.starts_with("\nprint open("), "{fixed}");
    assert!(
        !std::fs::read_to_string(&view)
            .unwrap()
            .contains("\"unused\"")
    );
}

/// A gateway collection with tag `T`, whose `valueChanged` event script
/// has an unused suppression of `SCR124` (`SCR123`) and trailing whitespace
/// (`SCR124`), the extra tags `extra` (JSON objects), and a project library
/// module `m` with `code`.
fn tag_configuration_workspace(code: &str, extra: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write("config/core/config-mode.json", "{}");
    write(
        "config/core/ignition/tag-provider/default/resource.json",
        r#"{"scope": "G", "version": 1, "files": ["config.json"]}"#,
    );
    write(
        "config/core/ignition/tag-provider/default/config.json",
        r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
    );
    write(
        "config/core/ignition/tag-definition/default/unary-resource.json",
        r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["tags.json"], "attributes": {"config": {}}}"#,
    );
    write(
        "config/core/ignition/tag-definition/default/tags.json",
        &format!(
            r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Int4", "eventScripts": [{{"eventid": "valueChanged", "script": "\tx = 1  # iglint: ignore[SCR124]\n\ty = 2   \n"}}]}}{extra}]"#
        ),
    );
    write(
        "p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    write(
        "p/ignition/script-python/m/resource.json",
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    );
    write("p/ignition/script-python/m/code.py", code);
    dir
}

/// Review round 12 (Opus) blocker: the safe fixes of a tag event script
/// edit `tags.json`, whose content scripts read without a file path:
/// `system.tag.getConfiguration`, `readBlocking` of the `eventScripts`
/// property, `exportTags` to a file that `open` then reads (both literal
/// paths outside the workspace), and the value of an expression tag that
/// references `{[.]T.eventScripts}` (verified on Ignition 8.3.3: each
/// returns the event script text with its comment and whitespace). A
/// read of the tag's value keeps the fixes safe.
#[test]
fn fixes_are_unsafe_when_scripts_read_tag_configuration() {
    let expression_tag = r#", {"name": "E", "tagType": "AtomicTag", "valueSource": "expr", "dataType": "String", "expression": "toStr({[.]T.eventScripts})"}"#;
    let value = "def f():\n    return system.tag.readBlocking(['[default]T'])[0].value\n";
    for (case, code, extra, what) in [
        (
            "getConfiguration",
            "def f():\n    return system.tag.getConfiguration('[default]T', False)\n",
            "",
            "system.tag.getConfiguration(",
        ),
        (
            "readBlocking",
            "def f():\n    return system.tag.readBlocking(['[default]T.eventScripts'])[0].value\n",
            "",
            "system.tag.readBlocking(",
        ),
        (
            "exportTags",
            "def f():\n    system.tag.exportTags('/tmp/x.json', ['[default]T'])\n    return open('/tmp/x.json').read()\n",
            "",
            "system.tag.exportTags(",
        ),
        (
            "expression tag",
            value,
            expression_tag,
            "tag path `[.]T.eventScripts`",
        ),
    ] {
        let dir = tag_configuration_workspace(code, extra);
        let tags = dir
            .path()
            .join("config/core/ignition/tag-definition/default/tags.json");
        let before = std::fs::read_to_string(&tags).unwrap();
        let out = iglint(dir.path())
            .args([
                "--no-cache",
                "--output-format",
                "json",
                "--select",
                "SCR123,SCR124",
            ])
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let found = v.as_array().unwrap();
        let mut rules: Vec<&str> = found.iter().map(|d| d["code"].as_str().unwrap()).collect();
        rules.sort_unstable();
        assert_eq!(rules, ["SCR123", "SCR124"], "{case}: {v}");
        for d in found {
            assert_eq!(d["fix"]["applicability"], "unsafe", "{case}: {d}");
            let reason = &d["fix"]["unsafe_reason"];
            assert!(
                reason["what"].as_str().unwrap().starts_with(what),
                "{case}: {reason}"
            );
        }
        iglint(dir.path())
            .args(["--no-cache", "--fix", "--select", "SCR123,SCR124"])
            .output()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&tags).unwrap(), before, "{case}");
    }
    // Reading the value only: the fixes stay safe and apply.
    let dir = tag_configuration_workspace(value, "");
    let tags = dir
        .path()
        .join("config/core/ignition/tag-definition/default/tags.json");
    iglint(dir.path())
        .args(["--no-cache", "--fix", "--select", "SCR123,SCR124"])
        .output()
        .unwrap();
    let fixed = std::fs::read_to_string(&tags).unwrap();
    assert!(
        fixed.contains(r#""script": "\tx = 1\n\ty = 2\n""#),
        "{fixed}"
    );
}

#[test]
fn reads_of_files_outside_the_workspace_keep_fixes_safe() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("p/ignition/script-python/m")).unwrap();
    std::fs::write(
        dir.path().join("p/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("p/ignition/script-python/m/resource.json"),
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    )
    .unwrap();
    let m = dir.path().join("p/ignition/script-python/m/code.py");
    for code in [
        "x = open('/tmp/report.csv').read()",
        // Writes, appends and renames between literal paths outside the
        // workspace.
        "import os\nos.rename('/tmp/a', '/tmp/b')\nf = open('/tmp/x.log', 'a')",
    ] {
        std::fs::write(&m, format!("{code}  \n")).unwrap();
        iglint(dir.path())
            .args(["--no-cache", "--fix", "--select", "SCR124"])
            .output()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&m).unwrap(), format!("{code}\n"));
    }
}

/// Reviewer round 11: moving a workspace file to a literal path outside
/// the workspace makes that path an observer, and appending to a file
/// reveals its size (`tell`), so every path-taking facility counts,
/// whatever its mode or direction (verified on Jython in the 8.3.3 image:
/// the rename variant prints `True` before `SCR102`'s fix and `False`
/// after; the append variant prints the size).
#[test]
fn moves_and_appends_of_workspace_files_make_fixes_unsafe() {
    let target = "/usr/local/bin/ignition/data/projects/p/ignition/script-python/m/code.py";
    let rename = format!(
        "import math\nimport os\nos.rename('{target}', '/tmp/m.py')\nprint open('/tmp/m.py').read().startswith('import math\\n')\n"
    );
    let helper = format!(
        "import math\nh.mv('{target}', '/tmp/m.py')\nprint open('/tmp/m.py').read().startswith('import math\\n')\n"
    );
    let append = format!(
        "import math\ndef size(p):\n    f = open(p, 'a')\n    f.seek(0, 2)\n    return f.tell()\nprint size('{target}')\n"
    );
    let keyword =
        format!("import math\nf = open(name='{target}', mode='a')\nf.seek(0, 2)\nprint f.tell()\n");
    let file_append = format!("import math\nf = file('{target}', 'a')\nprint f.tell()\n");
    for (case, code) in [
        ("rename", &rename),
        ("helper", &helper),
        ("append", &append),
        ("keyword", &keyword),
        ("file", &file_append),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "p/project.json",
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        );
        for (name, text) in [
            ("m", code.as_str()),
            ("h", "import os\ndef mv(a, b):\n    os.rename(a, b)\n"),
        ] {
            write(
                &format!("p/ignition/script-python/{name}/resource.json"),
                r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
            );
            write(&format!("p/ignition/script-python/{name}/code.py"), text);
        }
        let out = iglint(dir.path())
            .args([
                "--no-cache",
                "--output-format",
                "json",
                "--select",
                "SCR102",
            ])
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let found = v.as_array().unwrap();
        assert_eq!(found.len(), 1, "{case}: {v}");
        assert_eq!(found[0]["fix"]["applicability"], "unsafe", "{case}: {v}");
        let message = found[0]["fix"]["unsafe_reason"]["message"]
            .as_str()
            .unwrap();
        assert!(
            message.contains("may read the files a fix edits"),
            "{case}: {message}"
        );
        let m = dir.path().join("p/ignition/script-python/m/code.py");
        iglint(dir.path())
            .args(["--no-cache", "--fix", "--select", "SCR102"])
            .output()
            .unwrap();
        assert_eq!(&std::fs::read_to_string(&m).unwrap(), code, "{case}");
    }
}

/// Reviewer round 12, K1, through the binary: in a library module every
/// import that runs binds `__package__`, so `--fix` keeps the only import
/// of a module that reads it (Ignition 8.3.3: `None` with the import,
/// `'ABSENT'` without), the JSON output says why, and `--fix
/// --unsafe-fixes` removes it. A module that does not read it still loses
/// its import with `--fix`.
#[test]
fn imports_that_bind_a_read_package_are_kept() {
    let repro = "import math\n\ndef get_package():\n    try:\n        return __package__\n    except NameError:\n        return 'ABSENT'\n";
    let control = "def g():\n    import math\n    return 1\n";
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    for (name, text) in [("m", repro), ("n", control)] {
        write(
            &format!("p/ignition/script-python/{name}/resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        );
        write(&format!("p/ignition/script-python/{name}/code.py"), text);
    }
    let out = iglint(dir.path())
        .args([
            "--no-cache",
            "--output-format",
            "json",
            "--select",
            "SCR102",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let found = v.as_array().unwrap();
    assert_eq!(found.len(), 2, "{v}");
    let of = |module: &str| {
        found
            .iter()
            .find(|d| {
                d["filename"]
                    .as_str()
                    .is_some_and(|f| f.ends_with(&format!("{module}/code.py")))
            })
            .unwrap_or_else(|| panic!("{module}: {v}"))
    };
    assert_eq!(of("m")["fix"]["applicability"], "unsafe", "{v}");
    let message = of("m")["fix"]["unsafe_reason"]["message"].as_str().unwrap();
    assert!(message.contains("`__package__`"), "{message}");
    assert_eq!(of("n")["fix"]["applicability"], "safe", "{v}");
    let m = dir.path().join("p/ignition/script-python/m/code.py");
    let n = dir.path().join("p/ignition/script-python/n/code.py");
    iglint(dir.path())
        .args(["--no-cache", "--fix", "--select", "SCR102"])
        .output()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&m).unwrap(), repro);
    assert_eq!(
        std::fs::read_to_string(&n).unwrap(),
        "def g():\n\n    return 1\n"
    );
    iglint(dir.path())
        .args([
            "--no-cache",
            "--fix",
            "--unsafe-fixes",
            "--select",
            "SCR102",
        ])
        .output()
        .unwrap();
    assert!(
        !std::fs::read_to_string(&m).unwrap().contains("import math"),
        "the unsafe fix applies"
    );
}

/// Reviewer round 10, O1 and O2, through the binary: `--fix` must not
/// rename a component referenced from a tag history date range or an HTTP
/// header value (fields iglint did not model), nor one whose name a script
/// reads through `name`; `--fix --unsafe-fixes` renames and updates the
/// references; the JSON output names the place that makes it unsafe.
#[test]
fn component_renames_respect_measured_fields_and_name_reads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("p")).unwrap();
    std::fs::write(
        dir.path().join("p/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("iglint.toml"),
        "[rules.NAME001]\nconvention = \"PascalCase\"\n",
    )
    .unwrap();
    let view = r#"{"custom": {"selected": ""}, "params": {}, "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [
  {"meta": {"name": "start_date"}, "type": "ia.input.date-time-input"},
  {"meta": {"name": "api_token"}, "type": "ia.input.text-field"},
  {"meta": {"name": "pump_a"}, "type": "ia.input.button", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tself.view.custom.selected = self.name"}}}}},
  {"meta": {"name": "Chart"}, "type": "ia.display.label", "propConfig": {
    "props.text": {"binding": {"type": "tag-history", "config": {"dateRange": {"startDate": "{../start_date.props.value}", "endDate": "now(60000)"}, "tags": [{"path": "[default]T"}], "returnSize": {"type": "raw"}}}},
    "props.style": {"binding": {"type": "http", "config": {"request": {"url": "'https://example.com'", "method": "GET", "header": [{"key": "Authorization", "value": "'Bearer ' + {../api_token.props.text}"}]}}}}}}]}}"#;
    let dir_v = dir
        .path()
        .join("p/com.inductiveautomation.perspective/views/Main");
    std::fs::create_dir_all(&dir_v).unwrap();
    std::fs::write(
        dir_v.join("resource.json"),
        r#"{"scope": "G", "version": 1, "files": ["view.json"]}"#,
    )
    .unwrap();
    let path = dir_v.join("view.json");
    std::fs::write(&path, view).unwrap();
    let out = iglint(dir.path())
        .args([
            "--no-cache",
            "--select",
            "NAME001",
            "--output-format",
            "json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let finding = |name: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|d| {
                d["message"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("'{name}'"))
            })
            .unwrap_or_else(|| panic!("{name}: {v}"))
            .clone()
    };
    for name in ["start_date", "api_token", "pump_a"] {
        assert_eq!(finding(name)["fix"]["applicability"], "unsafe", "{name}");
    }
    let reason = &finding("pump_a")["fix"]["unsafe_reason"];
    assert_eq!(
        reason["message"], "a script reads the component's name, which the rename changes",
        "{reason}"
    );
    assert_eq!(reason["what"], "name", "{reason}");
    iglint(dir.path())
        .args(["--no-cache", "--fix", "--select", "NAME001"])
        .output()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), view);
    iglint(dir.path())
        .args([
            "--no-cache",
            "--fix",
            "--unsafe-fixes",
            "--select",
            "NAME001",
        ])
        .output()
        .unwrap();
    let fixed = std::fs::read_to_string(&path).unwrap();
    assert!(fixed.contains("{../StartDate.props.value}"), "{fixed}");
    assert!(fixed.contains("{../APIToken.props.text}"), "{fixed}");
}

/// Coordinator, round 10: a WebDev handler that reaches the gateway
/// context (`request['context']`) holds every gateway manager, among them
/// tag managers that import tags with their scripts from a JSON string
/// (`getTagManager().importTagsAsync(...)`). That is unknown code for the
/// closed world: SCR102's fix becomes unsafe and names the site. A handler
/// that reads only plain request items does not.
#[test]
fn webdev_gateway_context_is_unknown_code() {
    for (handler, safe) in [
        (
            "def doPost(request, session):\n    tags = request['context'].getTagManager()\n    tags.importTagsAsync(None, request['data'], 'json', None)\n",
            false,
        ),
        (
            "def doPost(request, session):\n    ctx = request['context']\n    return {'json': str(ctx)}\n",
            false,
        ),
        (
            "def doPost(request, session):\n    return {'json': request['params']}\n",
            true,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let write = |rel: &str, text: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "p/project.json",
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        );
        let api = "p/com.inductiveautomation.webdev/resources/api";
        write(
            &format!("{api}/resource.json"),
            r#"{"scope": "G", "version": 1, "files": ["config.json", "doPost.py"]}"#,
        );
        write(
            &format!("{api}/config.json"),
            r#"{"resource-type": "python-resource", "doPost": {"enabled": true}}"#,
        );
        write(&format!("{api}/doPost.py"), handler);
        write(
            "p/ignition/script-python/m/resource.json",
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        );
        write(
            "p/ignition/script-python/m/code.py",
            "def f():\n    import math\n    return 1\n",
        );
        let out = iglint(dir.path())
            .args([
                "--no-cache",
                "--select",
                "SCR102",
                "--output-format",
                "json",
            ])
            .output()
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let found = v.as_array().unwrap();
        assert_eq!(found.len(), 1, "{v}");
        let fix = &found[0]["fix"];
        if safe {
            assert_eq!(fix["applicability"], "safe", "{fix}");
        } else {
            assert_eq!(fix["applicability"], "unsafe", "{fix}");
            let reason = &fix["unsafe_reason"];
            assert!(
                reason["what"].as_str().unwrap().starts_with("request"),
                "{reason}"
            );
            assert!(
                reason["filename"].as_str().unwrap().ends_with("doPost.py"),
                "{reason}"
            );
        }
    }
}

/// Reviewer round 11: a directory of loose `.py` files has no Ignition
/// project, so nothing under it is linted; the run says so (exit 0)
/// instead of only "All checks passed!". Named on the command line, the
/// same file is linted as a standalone script.
#[test]
fn directories_without_lintable_files_say_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("loose")).unwrap();
    std::fs::write(dir.path().join("loose/x.py"), "def f(:\n").unwrap();
    let out = iglint(dir.path())
        .args(["--no-cache", "loose"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("warning: no Ignition project or lintable files found under loose"),
        "{stderr}"
    );
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    // `--quiet` prints findings only.
    let out = iglint(dir.path())
        .args(["--no-cache", "--quiet", "loose"])
        .output()
        .unwrap();
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = iglint(dir.path())
        .args(["--no-cache", "loose/x.py"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("SCR001"));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("no Ignition project"), "{stderr}");
    // A project directory is not reported.
    std::fs::write(
        dir.path().join("loose/project.json"),
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    let out = iglint(dir.path())
        .args(["--no-cache", "loose"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("no Ignition project"), "{stderr}");
}
