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
        "closed world: code in the workspace is not vetted, reads frames, reaches other views or runs in the browser, first"
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
             reads frames, reaches other views or runs in the browser, first: `re.sys` at \
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

/// Review round 13 (both reviewers): imports that a class body reads by
/// name, spelled mangled (`__m` is `_D__m` in class `D`) or before the
/// class binds the name, and an import the interpreter reads as a class's
/// `__metaclass__`, are used. `--fix` used to delete them (Jython then
/// raises `NameError` or creates the class); now they are not reported and
/// stay, while a control module's unused import is still removed safely.
#[test]
fn imports_read_by_class_bodies_survive_fix() {
    let modules = [
        (
            "mangled",
            "import math as _D__m\n\nclass D(object):\n    x = __m.floor(3.7)\n",
        ),
        (
            "before",
            "import math as m\n\nclass C(object):\n    x = m.pi\n    m = 1\n\nm = None\n",
        ),
        (
            "infunction",
            "import math as m\n\ndef f():\n    class C(object):\n        x = m.pi\n        m = 1\n    return C\n",
        ),
        (
            "mangledfrom",
            "from math import pi as _D__pi\n\nclass D(object):\n    x = __pi\n",
        ),
        (
            "metaclass",
            "import math as __metaclass__\ntry:\n    class C:\n        pass\nexcept TypeError:\n    print \"failed\"\nelse:\n    print \"created\"\n",
        ),
        ("control", "def g():\n    import math\n    return 1\n"),
    ];
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
    for (name, text) in modules {
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
            "SCR101,SCR102",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let found = v.as_array().unwrap();
    assert_eq!(found.len(), 1, "{v}");
    assert!(
        found[0]["filename"]
            .as_str()
            .unwrap()
            .ends_with("control/code.py"),
        "{v}"
    );
    assert_eq!(found[0]["fix"]["applicability"], "safe", "{v}");
    for args in [&["--fix"][..], &["--fix", "--unsafe-fixes"][..]] {
        iglint(dir.path())
            .args(["--no-cache", "--select", "SCR102"])
            .args(args)
            .output()
            .unwrap();
        for (name, text) in modules {
            let path = dir
                .path()
                .join(format!("p/ignition/script-python/{name}/code.py"));
            let now = std::fs::read_to_string(path).unwrap();
            if name == "control" {
                assert_eq!(now, "def g():\n\n    return 1\n");
            } else {
                assert_eq!(now, text, "{name}");
            }
        }
    }
}

/// Reviewer round 13: the most common `SCR102` safe fix (the import's
/// line emptied before a blank line) gave a hunk header whose ranges did
/// not match its lines (`@@ -1,3 +2,2 @@`), which `git apply` and `patch`
/// reject. The diff now applies with both and gives the fixed file.
#[cfg(unix)]
#[test]
fn diff_of_an_emptied_import_line_applies() {
    let source = |dir: &std::path::Path| {
        let write = |rel: &str, text: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "p/project.json",
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        );
        write(
            "p/ignition/script-python/m/resource.json",
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        );
        write(
            "p/ignition/script-python/m/code.py",
            "import math\n\nx = 1\n",
        );
    };
    let dir = tempfile::tempdir().unwrap();
    source(dir.path());
    let diff = iglint(dir.path())
        .args(["--isolated", "--no-cache", "--select", "SCR102", "--diff"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(diff.stdout).unwrap();
    let patch = stdout.split("\nWould fix").next().unwrap().to_owned();
    assert!(patch.contains("@@ -1,3 +1,3 @@\n"), "{patch}");
    let tools: [(&str, &[&str]); 2] = [("patch", &["-p1", "-s", "-t"]), ("git", &["apply"])];
    for (tool, args) in tools {
        let target = tempfile::tempdir().unwrap();
        source(target.path());
        let mut child = match std::process::Command::new(tool)
            .args(args)
            .current_dir(target.path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                eprintln!("{tool} not found: skipped");
                continue;
            }
        };
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(patch.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{tool}: {}{}\n{patch}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("p/ignition/script-python/m/code.py"))
                .unwrap(),
            "\n\nx = 1\n",
            "{tool}"
        );
    }
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

/// Reviewer round 18 (Opus) blocker B1, through the binary: a `system.*`
/// function the 8.3 stubs do not list, an expression function the gateway
/// does not register and a component that runs JavaScript in the browser
/// are unknown code. Each keeps `SCR102`'s fix unsafe (with the site),
/// and `--fix` does not apply it; the project vets the functions it lists
/// under `[fix]`.
#[test]
fn unknown_functions_and_browser_code_keep_fixes_unsafe() {
    let manifest = |file: &str| format!(r#"{{"scope": "A", "version": 1, "files": ["{file}"]}}"#);
    let module = "import math\n\ndef f():\n    return 1\n";
    for (other, file, what, config) in [
        (
            "def g():\n    return system.thirdparty.evalHere('x')\n",
            "p/ignition/script-python/n/code.py",
            "system.thirdparty.evalHere",
            "[fix]\nvetted-system-functions = [\"system.thirdparty.evalHere\"]\n",
        ),
        (
            r#"{"custom": {"x": 1}, "propConfig": {"custom.x": {"binding": {"type": "expr", "config": {"expression": "objectScript('1')"}}}}, "root": {"type": "ia.container.flex", "meta": {"name": "root"}}}"#,
            "p/com.inductiveautomation.perspective/views/B/view.json",
            "objectScript",
            "[fix]\nvetted-expression-functions = [\"objectScript\"]\n",
        ),
        (
            r#"{"root": {"type": "ia.container.flex", "meta": {"name": "root"}, "children": [{"type": "ia.display.iframe", "meta": {"name": "F"}}]}}"#,
            "p/com.inductiveautomation.perspective/views/B/view.json",
            "root/F (ia.display.iframe)",
            "",
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
        write(
            "p/ignition/script-python/m/resource.json",
            &manifest("code.py"),
        );
        write("p/ignition/script-python/m/code.py", module);
        let name = std::path::Path::new(file)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        write(
            &std::path::Path::new(file)
                .with_file_name("resource.json")
                .to_string_lossy(),
            &manifest(name),
        );
        write(file, other);
        let run = || {
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
            let found = v.as_array().unwrap().clone();
            assert_eq!(found.len(), 1, "{v}");
            found[0]["fix"].clone()
        };
        let fix = run();
        assert_eq!(fix["applicability"], "unsafe", "{fix}");
        let reason = &fix["unsafe_reason"];
        assert!(
            reason["what"].as_str().unwrap().starts_with(what),
            "{reason}"
        );
        assert_eq!(reason["filename"], file, "{reason}");
        iglint(dir.path())
            .args(["--no-cache", "--select", "SCR102", "--fix"])
            .output()
            .unwrap();
        let after =
            std::fs::read_to_string(dir.path().join("p/ignition/script-python/m/code.py")).unwrap();
        assert_eq!(after, module, "{what}");
        if !config.is_empty() {
            write("iglint.toml", config);
            let fix = run();
            assert_eq!(fix["applicability"], "safe", "{what}: {fix}");
        }
    }
}

/// Reviewer round 18 (Opus) blockers B2 and N1, through the binary: the
/// review's repro (an import in a function near the 64 KB method limit)
/// is reported with an unsafe fix that says why, and `--fix` leaves the
/// file as it is.
#[test]
fn fixes_near_the_method_limit_are_reported_unsafe() {
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
    write(
        "p/ignition/script-python/m/resource.json",
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    );
    let mut code = String::from("def g():\n    import math\n");
    for i in 0..2_176 {
        code.push_str(&format!("    a = abs({})\n", i % 7));
    }
    code.push_str("    c = 1\n    c = 1\n    return a + c\n");
    write("p/ignition/script-python/m/code.py", &code);
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
    let fix = &v[0]["fix"];
    assert_eq!(fix["applicability"], "unsafe", "{v}");
    assert!(
        fix["unsafe_reason"]["message"]
            .as_str()
            .unwrap()
            .contains("compile status is not known"),
        "{fix}"
    );
    let out = iglint(dir.path())
        .args(["--no-cache", "--select", "SCR102", "--fix"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("fix not applied"), "{text}");
    let after =
        std::fs::read_to_string(dir.path().join("p/ignition/script-python/m/code.py")).unwrap();
    assert_eq!(after, code);
}

/// Reviewer round 20 (Codex) blocker, through the binary: the review's
/// repro (an import in a function of 1,683 reads of an attribute of
/// 16,001 characters, which Jython rejects and compiles without the
/// import) is reported with an unsafe fix that names the identifier, and
/// `--fix` leaves the file as it is.
#[test]
fn fixes_in_scripts_with_long_identifiers_are_reported_unsafe() {
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
    write(
        "p/ignition/script-python/m/resource.json",
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    );
    let mut code = String::from("def work(obj):\n    import math\n");
    for _ in 0..1_683 {
        code.push_str(&format!("    obj.{}\n", "n".repeat(16_001)));
    }
    code.push_str("    return 1\nprint \"loaded\"\n");
    write("p/ignition/script-python/m/code.py", &code);
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
    let fix = &v[0]["fix"];
    assert_eq!(fix["applicability"], "unsafe", "{}", v[0]["code"]);
    assert_eq!(
        fix["unsafe_reason"]["message"],
        "the safe fix could change what the code does (it changes a script whose compile status is not known: identifier of 16,001 characters: Jython's code size cannot be bounded precisely)",
    );
    let out = iglint(dir.path())
        .args(["--no-cache", "--select", "SCR102", "--fix"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("fix not applied"), "{}", text.len());
    let after =
        std::fs::read_to_string(dir.path().join("p/ignition/script-python/m/code.py")).unwrap();
    assert!(after == code, "the file changed");
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
    // Reviewer round 15 N1: the working directory itself is shown as `.`.
    let out = iglint(&dir.path().join("loose"))
        .args(["--no-cache"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lintable files found under . (Python"),
        "{stderr}"
    );
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

/// A repository with project `A` (library module `m` with an unused
/// `import os`) and a gateway collection whose tag `T` has a value change
/// script `tag_script`.
fn closed_world_repository(tag_script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "projects/A/project.json",
        r#"{"title": "A", "enabled": true, "inheritable": false}"#,
    );
    write(
        "projects/A/ignition/script-python/m/resource.json",
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    );
    write(
        "projects/A/ignition/script-python/m/code.py",
        CLOSED_WORLD_MODULE,
    );
    let core = "services/gw/config/resources/core";
    write(&format!("{core}/config-mode.json"), "{}");
    write(
        &format!("{core}/ignition/tag-provider/default/resource.json"),
        r#"{"scope": "G", "version": 1, "files": ["config.json"]}"#,
    );
    write(
        &format!("{core}/ignition/tag-provider/default/config.json"),
        r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
    );
    write(
        &format!("{core}/ignition/tag-definition/default/unary-resource.json"),
        r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["tags.json"], "attributes": {"config": {}}}"#,
    );
    let script = serde_json::to_string(tag_script).unwrap();
    write(
        &format!("{core}/ignition/tag-definition/default/tags.json"),
        &format!(
            r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Int4", "eventScripts": [{{"eventid": "valueChanged", "script": {script}}}]}}]"#
        ),
    );
    dir
}

const CLOSED_WORLD_MODULE: &str = "import os\n\ndef f():\n    return 1\n";
const MODULE: &str = "projects/A/ignition/script-python/m/code.py";

/// SCR102 findings of `m` as (applicability, reason) from a JSON run.
fn scr102_fixes(out: &std::process::Output) -> Vec<(String, String)> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stdout)));
    v.as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "SCR102")
        .map(|d| {
            (
                d["fix"]["applicability"].as_str().unwrap_or("").to_owned(),
                d["fix"]["unsafe_reason"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            )
        })
        .collect()
}

/// Review round 14 (Opus) blocker: the closed world and the uses of
/// library names come from the workspace root, not from the paths given.
/// A gateway tag script uses `m.os`, so the import must stay whichever way
/// the module is linted: the file (pre-commit), its project, from inside
/// the project, through standard input.
#[test]
fn fixes_see_the_whole_workspace_whatever_is_linted() {
    let dir = closed_world_repository("\tprint m.os.sep\n");
    let root = dir.path();
    let project = root.join("projects/A");
    let runs: [(&std::path::Path, Vec<&str>); 4] = [
        (root, vec![MODULE]),
        (root, vec!["projects/A"]),
        (&project, vec!["."]),
        (&project, vec!["ignition/script-python/m/code.py"]),
    ];
    for (cwd, paths) in &runs {
        let mut args = vec!["check", "--no-cache", "--fix", "--select", "SCR102"];
        args.extend(paths.iter().copied());
        iglint(cwd).args(&args).assert().success();
        assert_eq!(
            std::fs::read_to_string(root.join(MODULE)).unwrap(),
            CLOSED_WORLD_MODULE,
            "{} {paths:?}",
            cwd.display()
        );
    }
    let out = iglint(root)
        .args([
            "check",
            "--fix",
            "--select",
            "SCR102",
            "--stdin-filename",
            MODULE,
        ])
        .write_stdin(CLOSED_WORLD_MODULE)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), CLOSED_WORLD_MODULE);
    // Without the tag script's use, the import goes, from the file too.
    let dir = closed_world_repository("\tprint 1\n");
    iglint(dir.path())
        .args(["check", "--no-cache", "--fix", "--select", "SCR102", MODULE])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(MODULE)).unwrap(),
        "\n\ndef f():\n    return 1\n"
    );
    // Reviewer round 15 B1: the same holds for every layout and every
    // file of it.
    for layout in LAYOUTS {
        for content in CONTENTS {
            single_file_runs_match_the_root_run(layout, content);
        }
    }
}

/// Where a fixture puts its projects and gateway collections.
struct Layout {
    name: &'static str,
    git: bool,
    /// A configuration file (relative path) with harmless contents.
    config: Option<&'static str>,
    /// Project directories; the first holds the library module `m`.
    projects: &'static [&'static str],
    /// Collection directories; each holds a tag with the event script.
    collections: &'static [&'static str],
    /// A directory that is a Git work tree of its own inside the top one.
    inner: Option<(&'static str, Inner)>,
    /// A linked work tree of the top repository (`git worktree add`),
    /// whose files hold another content (a feature branch).
    worktree: Option<Worktree>,
}

/// Where `git worktree add` puts a linked work tree.
#[derive(Clone, Copy)]
enum Worktree {
    /// At this path inside the main checkout.
    Inside(&'static str),
    /// Next to the main checkout, outside it.
    Outside,
}

/// How a nested work tree is made.
#[derive(Clone, Copy)]
enum Inner {
    /// `git init` inside the outer work tree (a `.git` directory).
    Repository,
    /// `git submodule add` of a local repository (a `.git` file that
    /// points into the outer repository's `.git/modules`); needs Git.
    Submodule,
}

const LAYOUTS: &[Layout] = &[
    // Reviewer round 15 B1: a gateway data directory next to `projects/`.
    Layout {
        name: "gateway data directory beside projects",
        git: true,
        config: None,
        projects: &["projects/A"],
        collections: &["gateway/data/config/resources/core"],
        inner: None,
        worktree: None,
    },
    // Reviewer round 15 B1, variant.
    Layout {
        name: "projects in src, gateway configuration elsewhere",
        git: true,
        config: None,
        projects: &["src/projects/A"],
        collections: &["gateway/config/resources/core"],
        inner: None,
        worktree: None,
    },
    Layout {
        name: "monorepo with several gateways",
        git: true,
        config: None,
        projects: &["gw1/data/projects/A", "gw2/data/projects/B"],
        collections: &[
            "gw1/data/config/resources/core",
            "gw2/services/gw/config/resources/core",
        ],
        inner: None,
        worktree: None,
    },
    Layout {
        name: "layout roots at different depths",
        git: true,
        config: None,
        projects: &["ignition/projects/A", "x/y/z/projects/C"],
        collections: &["deploy/site/services/gw/config/resources/core"],
        inner: None,
        worktree: None,
    },
    Layout {
        name: "configuration file in a subdirectory",
        git: true,
        config: Some("ignition/iglint.toml"),
        projects: &["ignition/projects/A", "other/projects/B"],
        collections: &["other/gateway/config/resources/core"],
        inner: None,
        worktree: None,
    },
    Layout {
        name: "no Git, configuration at the top",
        git: false,
        config: Some("iglint.toml"),
        projects: &["projects/A"],
        collections: &["gateway/data/config/resources/core"],
        inner: None,
        worktree: None,
    },
    // Reviewer round 16 B3: the projects in a nested repository, the
    // gateway configuration in the enclosing one.
    Layout {
        name: "nested repository",
        git: true,
        config: None,
        projects: &["ign/projects/A"],
        collections: &["gateway/config/resources/core"],
        inner: Some(("ign", Inner::Repository)),
        worktree: None,
    },
    Layout {
        name: "submodule",
        git: true,
        config: None,
        projects: &["ign/projects/A", "other/projects/B"],
        collections: &["gateway/config/resources/core"],
        inner: Some(("ign", Inner::Submodule)),
        worktree: None,
    },
    // Review round 17 (Opus) blocker: a linked work tree is a checkout of
    // its own, wherever `git worktree add` puts it; the main checkout's
    // walk skips hidden directories and must not take its files either.
    Layout {
        name: "linked work tree in .worktrees",
        git: true,
        config: None,
        projects: &["projects/A"],
        collections: &["gateway/config/resources/core"],
        inner: None,
        worktree: Some(Worktree::Inside(".worktrees/x")),
    },
    Layout {
        name: "linked work tree in .claude/worktrees",
        git: true,
        config: None,
        projects: &["projects/A", "other/projects/B"],
        collections: &["gateway/data/config/resources/core"],
        inner: None,
        worktree: Some(Worktree::Inside(".claude/worktrees/x")),
    },
    Layout {
        name: "linked work tree in a directory that is not hidden",
        git: true,
        config: None,
        projects: &["projects/A"],
        collections: &["gateway/config/resources/core"],
        inner: None,
        worktree: Some(Worktree::Inside("trees/x")),
    },
    Layout {
        name: "linked work tree outside the repository",
        git: true,
        config: None,
        projects: &["src/projects/A"],
        collections: &["gateway/config/resources/core"],
        inner: None,
        worktree: Some(Worktree::Outside),
    },
];

/// (library module `m`, tag event script, what the root run must show).
const CONTENTS: &[(&str, &str, &str)] = &[
    // The tag script keeps `import os` in `m` used.
    (CLOSED_WORLD_MODULE, "\tprint m.os.sep\n", "no SCR102"),
    // Code that is not vetted: the import's removal is unsafe.
    (CLOSED_WORLD_MODULE, "\tprint eval('1')\n", "SCR102 unsafe"),
    // A library function reads files: every fix is unsafe.
    (
        "def f(p):\n    return open(p).read()\n",
        "\tprint 1  # iglint: ignore[SCR001]\n",
        "SCR123 unsafe",
    ),
];

fn write_layout(layout: &Layout, module: &str, tag_script: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let write = |rel: &str, text: &str| {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    if layout.git
        && !matches!(layout.inner, Some((_, Inner::Submodule)))
        && layout.worktree.is_none()
    {
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    }
    if let Some(config) = layout.config {
        write(config, "respect-gitignore = true\n");
    }
    write_contents(dir.path(), layout, module, tag_script);
    match layout.inner {
        Some((inner, Inner::Repository)) => {
            std::fs::create_dir_all(dir.path().join(inner).join(".git")).unwrap();
        }
        Some((inner, Inner::Submodule)) => make_submodule(dir.path(), inner),
        None => {}
    }
    dir
}

/// Writes the projects and collections of `layout` below `top`, with the
/// library module `m` and the tag event script.
fn write_contents(top: &std::path::Path, layout: &Layout, module: &str, tag_script: &str) {
    let write = |rel: &str, text: &str| {
        let path = top.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    for (i, project) in layout.projects.iter().enumerate() {
        write(
            &format!("{project}/project.json"),
            r#"{"title": "P", "enabled": true, "inheritable": false}"#,
        );
        if i == 0 {
            write(
                &format!("{project}/ignition/script-python/m/resource.json"),
                r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
            );
            write(
                &format!("{project}/ignition/script-python/m/code.py"),
                module,
            );
        }
    }
    let script = serde_json::to_string(tag_script).unwrap();
    for core in layout.collections {
        write(&format!("{core}/config-mode.json"), "{}");
        write(
            &format!("{core}/ignition/tag-definition/default/unary-resource.json"),
            r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["tags.json"], "attributes": {"config": {}}}"#,
        );
        write(
            &format!("{core}/ignition/tag-definition/default/tags.json"),
            &format!(
                r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Int4", "eventScripts": [{{"eventid": "valueChanged", "script": {script}}}]}}]"#
            ),
        );
    }
}

/// Runs Git in `cwd` with a fixed identity and settings, and fails the
/// test when it fails.
fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "user.name=iglint",
            "-c",
            "user.email=iglint@example.invalid",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Commits everything below `top` to a new repository and adds a linked
/// work tree of it with `git worktree add`; returns the work tree's
/// directory. Its files are those of the commit until the caller changes
/// them. Git keeps the work tree's administrative files in
/// `top/.git/worktrees/<name>`.
fn make_worktree(top: &std::path::Path, worktree: Worktree) -> std::path::PathBuf {
    git(top, &["init", "-q"]);
    git(top, &["add", "-A"]);
    git(top, &["commit", "-q", "-m", "main"]);
    let path = match worktree {
        Worktree::Inside(rel) => top.join(rel),
        Worktree::Outside => top.with_file_name(format!(
            "{}-worktree",
            top.file_name().unwrap().to_string_lossy()
        )),
    };
    git(
        top,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            path.to_str().unwrap(),
        ],
    );
    assert!(path.join(".git").is_file());
    path
}

/// Makes `top` a Git repository and `top/inner` a submodule of it: the
/// files of `inner` are committed to a repository next to `top` and added
/// back with `git submodule add` from its `file://` URL.
fn make_submodule(top: &std::path::Path, inner: &str) {
    let source = top.with_file_name(format!(
        "{}-submodule-source",
        top.file_name().unwrap().to_string_lossy()
    ));
    std::fs::rename(top.join(inner), &source).unwrap();
    git(&source, &["init", "-q"]);
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-q", "-m", "inner"]);
    git(top, &["init", "-q"]);
    let url = format!("file://{}", source.display());
    git(top, &["submodule", "add", "-q", &url, inner]);
    std::fs::remove_dir_all(&source).unwrap();
    assert!(top.join(inner).join(".git").is_file());
}

/// The findings of a JSON run, one line each, with paths made absolute
/// (relative ones are relative to `cwd`): file, code, position, message,
/// fix applicability and the reason with its site.
fn findings(out: &std::process::Output, cwd: &std::path::Path) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let absolute = |p: &serde_json::Value| {
        p.as_str()
            .map(|p| cwd.join(p).display().to_string())
            .unwrap_or_default()
    };
    let mut found: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let reason = &d["fix"]["unsafe_reason"];
            format!(
                "{} {} {}:{}-{}:{} {} | {} | {} {} {}:{}",
                absolute(&d["filename"]),
                d["code"].as_str().unwrap_or(""),
                d["location"]["row"],
                d["location"]["column"],
                d["end_location"]["row"],
                d["end_location"]["column"],
                d["message"].as_str().unwrap_or(""),
                d["fix"]["applicability"].as_str().unwrap_or("no fix"),
                reason["message"].as_str().unwrap_or(""),
                absolute(&reason["filename"]),
                reason["location"]["row"],
                reason["location"]["column"],
            )
        })
        .collect();
    found.sort();
    found
}

/// Every file of the fixture, linted on its own (from the top, from
/// another directory with an absolute path, and from its own directory),
/// gives the findings and fix applicabilities the run over the whole
/// fixture gives for it.
fn single_file_runs_match_the_root_run(
    layout: &Layout,
    content @ (module, tag, expect): &(&str, &str, &str),
) {
    let dir = write_layout(layout, module, tag);
    let top = dir.path();
    // A linked work tree with another content: the next one of CONTENTS.
    let worktree = layout.worktree.map(|w| {
        let path = make_worktree(top, w);
        let i = CONTENTS.iter().position(|c| c == content).unwrap();
        let feature = CONTENTS[(i + 1) % CONTENTS.len()];
        write_contents(&path, layout, feature.0, feature.1);
        (path, feature)
    });
    let all = check_json(top, None);
    assert_expected(
        &all,
        expect,
        &format!("{}, {expect}: {all:#?}", layout.name),
    );
    let elsewhere = tempfile::tempdir().unwrap();
    let skip = worktree.as_ref().map(|(p, _)| p.clone());
    let files = fixture_files(top, skip.as_deref());
    if let Some(wt) = &skip {
        assert!(
            all.iter()
                .all(|f| !f.starts_with(&wt.display().to_string())),
            "{}: the main checkout's run reports files of the linked work tree: {all:#?}",
            layout.name
        );
    }
    for file in &files {
        let rel = file.strip_prefix(top).unwrap();
        let expected: Vec<&String> = all
            .iter()
            .filter(|f| f.starts_with(&format!("{} ", file.display())))
            .collect();
        for (cwd, path) in [
            (top, rel.to_path_buf()),
            (elsewhere.path(), file.clone()),
            (file.parent().unwrap(), file.file_name().unwrap().into()),
        ] {
            let got = check_json(cwd, Some(&path));
            assert_eq!(
                got.iter().collect::<Vec<_>>(),
                expected,
                "{}, {expect}: {} from {}",
                layout.name,
                path.display(),
                cwd.display()
            );
        }
    }
    if let Some((wt, feature)) = worktree {
        let outcome = linked_worktree_runs_match_own_repository(layout, &wt, feature.2);
        if matches!(layout.worktree, Some(Worktree::Outside)) {
            std::fs::remove_dir_all(&wt).unwrap();
        }
        outcome();
    }
}

/// A JSON run of `iglint check` in `cwd` over `path` (or the working
/// directory), as [`findings`].
fn check_json(cwd: &std::path::Path, path: Option<&std::path::Path>) -> Vec<String> {
    let mut command = iglint(cwd);
    command.args(["check", "--no-cache", "--output-format", "json"]);
    if let Some(path) = path {
        command.arg(path);
    }
    findings(&command.output().unwrap(), cwd)
}

/// Checks what the run over a whole fixture must show for `expect`.
fn assert_expected(all: &[String], expect: &str, shown: &str) {
    match expect {
        "no SCR102" => assert!(all.iter().all(|f| !f.contains(" SCR102 ")), "{shown}"),
        "SCR102 unsafe" => assert!(
            all.iter()
                .any(|f| f.contains(" SCR102 ") && f.contains("| unsafe | closed world:")),
            "{shown}"
        ),
        _ => assert!(
            all.iter().any(|f| f.contains(" SCR123 ")
                && f.contains("| unsafe | code in the workspace may read")),
            "{shown}"
        ),
    }
}

/// The files of a fixture below `top`, sorted, without Git's files and
/// without the directory `skip`.
fn fixture_files(top: &std::path::Path, skip: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![top.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if entry.file_name() == ".git"
                || entry.file_name() == ".gitmodules"
                || Some(path.as_path()) == skip
            {
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

/// Every file of the linked work tree `wt`, linted on its own from the
/// work tree's top, from its own directory and from outside, gives what a
/// run over a copy of the work tree made its own repository gives for it.
/// Returns the assertions to run once the caller has cleaned up.
fn linked_worktree_runs_match_own_repository(
    layout: &Layout,
    wt: &std::path::Path,
    expect: &str,
) -> impl FnOnce() {
    let own = tempfile::tempdir().unwrap();
    let files = fixture_files(wt, None);
    for file in &files {
        let to = own.path().join(file.strip_prefix(wt).unwrap());
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(file, &to).unwrap();
    }
    std::fs::create_dir_all(own.path().join(".git")).unwrap();
    let own_prefix = own.path().display().to_string();
    let wt_prefix = wt.display().to_string();
    let all: Vec<String> = check_json(own.path(), None)
        .into_iter()
        .map(|f| f.replace(&own_prefix, &wt_prefix))
        .collect();
    let elsewhere = tempfile::tempdir().unwrap();
    let mut failures = Vec::new();
    let shown = format!("{} (work tree), {expect}: {all:#?}", layout.name);
    for file in &files {
        let rel = file.strip_prefix(wt).unwrap();
        let expected: Vec<String> = all
            .iter()
            .filter(|f| f.starts_with(&format!("{} ", file.display())))
            .cloned()
            .collect();
        for (cwd, path) in [
            (wt, rel.to_path_buf()),
            (elsewhere.path(), file.clone()),
            (file.parent().unwrap(), file.file_name().unwrap().into()),
        ] {
            let got = check_json(cwd, Some(&path));
            if got != expected {
                failures.push(format!(
                    "{} (work tree), {expect}: {} from {}:\n got {got:#?}\n expected {expected:#?}",
                    layout.name,
                    path.display(),
                    cwd.display()
                ));
            }
        }
    }
    let expect = expect.to_owned();
    move || {
        assert_expected(&all, &expect, &shown);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

/// The `eval` variant: a tag script that is not vetted makes the fix
/// unsafe under the closed-world rule, the same for a run over the root,
/// the file, the project, from inside the project and through standard
/// input, with and without the cache.
#[test]
fn closed_world_reasons_do_not_depend_on_what_is_linted() {
    let dir = closed_world_repository("\tprint eval('1')\n");
    let root = dir.path();
    let project = root.join("projects/A");
    let closed = "closed world: code in the workspace is not vetted, reads frames, reaches \
                  other views or runs in the browser, first";
    let expected = vec![("unsafe".to_owned(), closed.to_owned())];
    let runs: [(&std::path::Path, Vec<&str>); 5] = [
        (root, vec![]),
        (root, vec![MODULE]),
        (root, vec!["projects/A"]),
        (&project, vec!["."]),
        (&project, vec!["ignition/script-python/m/code.py"]),
    ];
    for cache in [false, true] {
        for (cwd, paths) in &runs {
            let mut args = vec!["check", "--select", "SCR102", "--output-format", "json"];
            if cache {
                args.extend(["--cache-dir", "cache"]);
            } else {
                args.push("--no-cache");
            }
            args.extend(paths.iter().copied());
            let out = iglint(cwd).args(&args).output().unwrap();
            assert_eq!(scr102_fixes(&out), expected, "{} {paths:?}", cwd.display());
        }
    }
    let out = iglint(root)
        .args([
            "check",
            "--select",
            "SCR102",
            "--output-format",
            "json",
            "--stdin-filename",
            MODULE,
        ])
        .write_stdin(CLOSED_WORLD_MODULE)
        .output()
        .unwrap();
    assert_eq!(scr102_fixes(&out), expected);
}

/// The SCR102 and SCR123 fixes of a JSON run as (code, applicability,
/// reason).
fn fixes_of(out: &std::process::Output) -> Vec<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&out.stdout)));
    v.as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "SCR102" || d["code"] == "SCR123")
        .map(|d| {
            (
                d["code"].as_str().unwrap().to_owned(),
                d["fix"]["applicability"].as_str().unwrap_or("").to_owned(),
                d["fix"]["unsafe_reason"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
            )
        })
        .collect()
}

/// Reviewer round 15 B2: projects and collections named like build or
/// dependency directories are linted and are part of the workspace (they
/// load on the gateway).
#[test]
fn unusual_project_and_collection_directories_are_linted() {
    let dir = closed_world_repository("\tprint 1  # iglint: ignore[SCR001]\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    // `projects/A` becomes `projects/target`, the collection `target`.
    std::fs::rename(root.join("projects/A"), root.join("projects/target")).unwrap();
    std::fs::rename(
        root.join("services/gw/config/resources/core"),
        root.join("services/gw/config/resources/target"),
    )
    .unwrap();
    let out = iglint(root)
        .args(["check", "--no-cache", "--output-format", "json"])
        .output()
        .unwrap();
    let mut fixes = fixes_of(&out);
    fixes.sort();
    assert_eq!(
        fixes,
        [
            ("SCR102".to_owned(), "safe".to_owned(), String::new()),
            ("SCR123".to_owned(), "safe".to_owned(), String::new()),
        ],
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("REF009"), "{text}");
    // A library function in a project named `node_modules` that reads
    // files: it is part of the workspace, so every fix is unsafe, from
    // any file.
    std::fs::rename(
        root.join("projects/target"),
        root.join("projects/node_modules"),
    )
    .unwrap();
    std::fs::write(
        root.join("projects/node_modules/ignition/script-python/m/code.py"),
        "def f(p):\n    return open(p).read()\n",
    )
    .unwrap();
    let tags = "services/gw/config/resources/target/ignition/tag-definition/default/tags.json";
    for args in [vec![], vec![tags]] {
        let out = iglint(root)
            .args(["check", "--no-cache", "--output-format", "json"])
            .args(&args)
            .output()
            .unwrap();
        let fixes = fixes_of(&out);
        assert_eq!(fixes.len(), 1, "{args:?} {fixes:?}");
        assert_eq!(fixes[0].1, "unsafe", "{args:?} {fixes:?}");
        assert!(
            fixes[0].2.starts_with("code in the workspace may read"),
            "{fixes:?}"
        );
    }
}

/// The findings of a JSON run as (file, code, applicability, reason,
/// message).
fn all_fixes(out: &std::process::Output) -> Vec<(String, String, String, String, String)> {
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let mut found: Vec<_> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["filename"].as_str().unwrap_or("").to_owned(),
                d["code"].as_str().unwrap_or("").to_owned(),
                d["fix"]["applicability"]
                    .as_str()
                    .unwrap_or("no fix")
                    .to_owned(),
                d["fix"]["unsafe_reason"]["message"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned(),
                d["message"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    found.sort();
    found
}

/// Reviewer round 16 B2: Ignition 8.3.3 does not follow symbolic links to
/// directories (verified on a throwaway gateway: a linked project never
/// starts, a linked `core` collection keeps the gateway from starting, a
/// linked library folder, `script-python` or `ignition` directory gives
/// `ImportError: No module named lib`, a linked tag folder has no tags).
/// Neither does iglint: what a link holds is not part of the workspace,
/// the files behind it are reported by REF009 and get no fixes (the edit
/// would land where the link points), and the link counts as a directory
/// that was not read, so every safe fix elsewhere is unsafe with the link
/// named.
#[cfg(unix)]
#[test]
fn symbolic_links_to_directories_are_not_followed() {
    let link = |target: &std::path::Path, at: &std::path::Path| {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, at).unwrap();
    };
    // (where the link is, relative to the repository; what it replaces).
    let cases: [(&str, &str); 5] = [
        ("projects/S", "a project"),
        ("services/gw/config/resources/core", "a collection"),
        ("projects/A/ignition/script-python/lib", "a library folder"),
        ("projects/B/ignition/script-python", "the script library"),
        ("projects/B/ignition", "the module directory"),
    ];
    for (at, what) in cases {
        // The tag script uses `m.os`: with the collection behind a link,
        // that use is not known, and nothing else may be assumed either.
        let dir = closed_world_repository("\tprint m.os.sep\n");
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let lib = "def f(p):\n    return 1\nimport os\n";
        for project in ["S", "B"] {
            let p = root.join(format!("projects/{project}"));
            std::fs::create_dir_all(p.join("ignition/script-python/lib/n")).unwrap();
            std::fs::write(
                p.join("project.json"),
                r#"{"title": "P", "enabled": true, "inheritable": false}"#,
            )
            .unwrap();
            std::fs::write(
                p.join("ignition/script-python/lib/n/resource.json"),
                r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
            )
            .unwrap();
            std::fs::write(p.join("ignition/script-python/lib/n/code.py"), lib).unwrap();
        }
        let a_lib = root.join("projects/A/ignition/script-python/lib/n");
        std::fs::create_dir_all(&a_lib).unwrap();
        std::fs::copy(
            root.join("projects/S/ignition/script-python/lib/n/resource.json"),
            a_lib.join("resource.json"),
        )
        .unwrap();
        std::fs::write(a_lib.join("code.py"), lib).unwrap();
        let moved = elsewhere.path().join("moved");
        std::fs::rename(root.join(at), &moved).unwrap();
        link(&moved, &root.join(at));

        let run = |args: &[&str]| {
            let out = iglint(root)
                .args(["check", "--no-cache", "--output-format", "json"])
                .args(args)
                .output()
                .unwrap();
            all_fixes(&out)
        };
        let found = run(&[]);
        let shown = format!("{what}: {found:#?}");
        // Every SCR102 finding of the workspace: unsafe, the link named.
        let outside: Vec<_> = found
            .iter()
            .filter(|f| f.1 == "SCR102" && !f.0.starts_with(at))
            .collect();
        assert!(!outside.is_empty(), "{shown}");
        for f in outside {
            assert_eq!(f.2, "unsafe", "{shown}");
            assert!(
                f.3.starts_with("closed world unknown: `")
                    && f.3
                        .contains(&format!("{at}` could hold Ignition resources"))
                    && f.3
                        .contains("symbolic link to a directory: Ignition 8.3.3 does not follow"),
                "{shown}"
            );
        }
        // The files behind the link: REF009, and no fix at all.
        let behind: Vec<_> = found.iter().filter(|f| f.0.starts_with(at)).collect();
        assert!(!behind.is_empty(), "{shown}");
        assert!(
            behind.iter().any(|f| f.1 == "REF009"
                && f.4.contains("is behind `")
                && f.4.contains("a symbolic link to a directory")),
            "{shown}"
        );
        assert!(behind.iter().all(|f| f.2 == "no fix"), "{shown}");
        // `--fix --unsafe-fixes` leaves the link target alone.
        let before: Vec<_> = walk_files(&moved)
            .into_iter()
            .map(|p| (p.clone(), std::fs::read(&p).unwrap()))
            .collect();
        iglint(root)
            .args(["check", "--no-cache", "--fix", "--unsafe-fixes"])
            .output()
            .unwrap();
        for (path, text) in before {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                text,
                "{what}: {}",
                path.display()
            );
        }
        assert!(
            std::fs::symlink_metadata(root.join(at))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

/// Every file below `dir` (through no links), sorted.
fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Projects `P` and `Q` in a Git work tree: `P`'s library module `lib/a`
/// imports `math` and `os` and uses only `math`; `Q`'s module `lib/a` is a
/// symbolic link to `P`'s file, and `Q`'s module `use/b` uses
/// `lib.a.os.sep`.
#[cfg(unix)]
fn linked_file_repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let write = |rel: &str, text: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    let manifest = r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#;
    for p in ["P", "Q"] {
        write(
            &format!("projects/{p}/project.json"),
            r#"{"title": "P", "enabled": true, "inheritable": false}"#,
        );
        write(
            &format!("projects/{p}/ignition/script-python/lib/a/resource.json"),
            manifest,
        );
    }
    write(LINKED_P, LINKED_MODULE);
    std::os::unix::fs::symlink(
        "../../../../../P/ignition/script-python/lib/a/code.py",
        root.join(LINKED_Q),
    )
    .unwrap();
    write(
        "projects/Q/ignition/script-python/use/b/resource.json",
        manifest,
    );
    write(
        "projects/Q/ignition/script-python/use/b/code.py",
        "def g():\n    return lib.a.os.sep\n",
    );
    dir
}

const LINKED_MODULE: &str = "import math\nimport os\n\ndef f():\n    return math.pi\n";
const LINKED_P: &str = "projects/P/ignition/script-python/lib/a/code.py";
const LINKED_Q: &str = "projects/Q/ignition/script-python/lib/a/code.py";

/// Reviewer round 16 B1: the gateway reads a library `code.py` that is a
/// symbolic link (verified on 8.3.3: project `Q` ran `P`'s file), so a fix
/// of `P`'s file changes `Q`'s module, whose `os` `Q` uses. Every fix of
/// a file that is a link, the target of a link of the workspace or a file
/// with hard links is unsafe, with the other paths named; `--fix` keeps
/// the import, and a fix written to a link keeps the link.
#[cfg(unix)]
#[test]
fn fixes_of_files_reached_through_links_are_unsafe() {
    let dir = linked_file_repository();
    let root = dir.path();
    for args in [vec![], vec![LINKED_P], vec!["projects/P"]] {
        let out = iglint(root)
            .args([
                "check",
                "--no-cache",
                "--output-format",
                "json",
                "--select",
                "SCR102",
            ])
            .args(&args)
            .output()
            .unwrap();
        let found = all_fixes(&out);
        let p: Vec<_> = found.iter().filter(|f| f.0 == LINKED_P).collect();
        assert_eq!(p.len(), 1, "{args:?} {found:#?}");
        assert_eq!(p[0].2, "unsafe", "{found:#?}");
        assert!(
            p[0].3.contains("which is the same file as `")
                && p[0]
                    .3
                    .contains(&format!("{LINKED_Q}` (a symbolic link to it)")),
            "{found:#?}"
        );
        iglint(root)
            .args(["check", "--no-cache", "--fix", "--select", "SCR102"])
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join(LINKED_P)).unwrap(),
            LINKED_MODULE
        );
    }
    // The link itself (Q does not use `math`... it uses `os`; make a
    // module whose link is the one with the finding).
    std::fs::write(
        root.join("projects/Q/ignition/script-python/use/b/code.py"),
        "def g():\n    return 1\n",
    )
    .unwrap();
    let out = iglint(root)
        .args([
            "check",
            "--no-cache",
            "--output-format",
            "json",
            "--select",
            "SCR102",
            LINKED_Q,
        ])
        .output()
        .unwrap();
    let found = all_fixes(&out);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].2, "unsafe", "{found:#?}");
    assert!(
        found[0].3.contains(&format!(
            "{LINKED_P}` (the file this symbolic link points to)"
        )),
        "{found:#?}"
    );
    // Applied on purpose, the fix writes through the link.
    iglint(root)
        .args([
            "check",
            "--no-cache",
            "--fix",
            "--unsafe-fixes",
            "--select",
            "SCR102",
            LINKED_Q,
        ])
        .assert()
        .success();
    assert!(
        std::fs::symlink_metadata(root.join(LINKED_Q))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(root.join(LINKED_P)).unwrap(),
        "import math\n\n\ndef f():\n    return math.pi\n"
    );

    // A link to a file outside the workspace, and a hard link.
    let dir = linked_file_repository();
    let root = dir.path();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("x.py"), LINKED_MODULE).unwrap();
    std::fs::remove_file(root.join(LINKED_Q)).unwrap();
    std::os::unix::fs::symlink(outside.path().join("x.py"), root.join(LINKED_Q)).unwrap();
    std::fs::hard_link(root.join(LINKED_P), outside.path().join("hard.py")).unwrap();
    std::fs::write(
        root.join("projects/Q/ignition/script-python/use/b/code.py"),
        "def g():\n    return 1\n",
    )
    .unwrap();
    let out = iglint(root)
        .args([
            "check",
            "--no-cache",
            "--output-format",
            "json",
            "--select",
            "SCR102",
        ])
        .output()
        .unwrap();
    let found = all_fixes(&out);
    let reason = |file: &str| {
        found
            .iter()
            .find(|f| f.0 == file)
            .map(|f| (f.2.clone(), f.3.clone()))
            .unwrap_or_else(|| panic!("{found:#?}"))
    };
    let (applicability, why) = reason(LINKED_Q);
    assert_eq!(applicability, "unsafe");
    assert!(
        why.contains("x.py` (the file this symbolic link points to)"),
        "{why}"
    );
    let (applicability, why) = reason(LINKED_P);
    assert_eq!(applicability, "unsafe");
    assert!(
        why.contains("1 other hard link outside the workspace"),
        "{why}"
    );
    iglint(root)
        .args([
            "check",
            "--no-cache",
            "--fix",
            "--unsafe-fixes",
            "--select",
            "SCR102",
        ])
        .assert()
        .success();
    assert!(
        std::fs::symlink_metadata(root.join(LINKED_Q))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("hard.py")).unwrap(),
        std::fs::read_to_string(root.join(LINKED_P)).unwrap(),
        "the hard link is kept"
    );
    assert_ne!(
        std::fs::read_to_string(root.join(LINKED_P)).unwrap(),
        LINKED_MODULE
    );
}

/// Reviewer round 16: manifests that are symbolic links to files are read,
/// like the gateway reads them (verified on 8.3.3 for `project.json`,
/// `resource.json`, `config-mode.json` and `tags.json`).
#[cfg(unix)]
#[test]
fn manifests_that_are_links_are_read() {
    let dir = closed_world_repository("\tprint m.os.sep\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let store = tempfile::tempdir().unwrap();
    let core = "services/gw/config/resources/core";
    for (i, rel) in [
        "projects/A/project.json".to_owned(),
        "projects/A/ignition/script-python/m/resource.json".to_owned(),
        format!("{core}/config-mode.json"),
        format!("{core}/ignition/tag-definition/default/tags.json"),
    ]
    .iter()
    .enumerate()
    {
        let kept = store.path().join(format!("{i}.json"));
        std::fs::rename(root.join(rel), &kept).unwrap();
        std::os::unix::fs::symlink(&kept, root.join(rel)).unwrap();
    }
    let out = iglint(root)
        .args(["check", "--no-cache", "--output-format", "json"])
        .output()
        .unwrap();
    let found = all_fixes(&out);
    // The tag script (in a linked tags.json of a collection with a linked
    // config-mode.json) keeps `import os` of `m` used.
    assert!(
        !found.iter().any(|f| f.1 == "SCR102" || f.1 == "REF009"),
        "{found:#?}"
    );
}

/// Reviewer round 16 B3: a nested Git repository (made with `git init`
/// inside another work tree) holding `projects/` is part of the enclosing
/// work tree, where a gateway tag script uses `m.os`: linting the module
/// on its own gives the root run's answer (no SCR102).
#[test]
fn nested_repositories_belong_to_the_enclosing_work_tree() {
    let dir = closed_world_repository("\tprint m.os.sep\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("ign")).unwrap();
    std::fs::rename(root.join("projects"), root.join("ign/projects")).unwrap();
    std::fs::create_dir_all(root.join("ign/.git")).unwrap();
    let module = format!("ign/{MODULE}");
    for (cwd, path) in [
        (root.to_path_buf(), module.clone()),
        (root.join("ign"), MODULE.to_owned()),
        (
            root.join("ign/projects/A/ignition/script-python/m"),
            "code.py".to_owned(),
        ),
    ] {
        let out = iglint(&cwd)
            .args([
                "check",
                "--no-cache",
                "--output-format",
                "json",
                "--select",
                "SCR102",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(scr102_fixes(&out), [], "{} {path}", cwd.display());
    }
    let out = iglint(root)
        .args(["check", "--no-cache", "--show-settings"])
        .arg(&module)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(&format!("Workspace root: {}\n", root.display())),
        "{text}"
    );
}

/// Hidden resource directories and folders inside a project load on the
/// gateway (verified on 8.3.3, like resources named `target`,
/// `node_modules` or `__pycache__`), so they are linted and part of the
/// closed world.
#[test]
fn hidden_resource_directories_are_linted() {
    let dir = closed_world_repository("\tprint 1\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let lib = root.join("projects/A/ignition/script-python");
    for module in [".hidden", "pkg/.inner", "__pycache__"] {
        std::fs::create_dir_all(lib.join(module)).unwrap();
        std::fs::write(
            lib.join(module).join("resource.json"),
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        )
        .unwrap();
        std::fs::write(lib.join(module).join("code.py"), "def f(:\n").unwrap();
    }
    let out = iglint(root)
        .args(["check", "--no-cache", "--output-format", "json"])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut syntax: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "SCR001")
        .map(|d| d["filename"].as_str().unwrap().to_owned())
        .collect();
    syntax.sort();
    assert_eq!(
        syntax,
        [
            "projects/A/ignition/script-python/.hidden/code.py",
            "projects/A/ignition/script-python/__pycache__/code.py",
            "projects/A/ignition/script-python/pkg/.inner/code.py",
        ]
    );
    // A hidden module that is not vetted is in the closed world.
    std::fs::write(lib.join(".hidden/code.py"), "x = eval('1')\n").unwrap();
    let out = iglint(root)
        .args(["check", "--no-cache", "--output-format", "json", MODULE])
        .output()
        .unwrap();
    let fixes = fixes_of(&out);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].1, "unsafe", "{fixes:?}");
}

/// Reviewer round 15 B2: a directory of the workspace that cannot be
/// walked (a broken link, a link to a directory that holds projects, no
/// permission to list it) may
/// hold projects; the closed world is then unknown, never "no code
/// there", and every fix says which directory.
#[cfg(unix)]
#[test]
fn directories_that_cannot_be_read_make_the_closed_world_unknown() {
    let fixes = |root: &std::path::Path| {
        let out = iglint(root)
            .args(["check", "--no-cache", "--output-format", "json", MODULE])
            .output()
            .unwrap();
        fixes_of(&out)
    };
    let dir = closed_world_repository("\tprint 1\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    assert_eq!(
        fixes(root),
        [("SCR102".to_owned(), "safe".to_owned(), String::new())]
    );
    let unknown = |what: &str| {
        let found = fixes(root);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].1, "unsafe", "{found:?}");
        assert!(
            found[0].2.starts_with("closed world unknown: `")
                && found[0]
                    .2
                    .contains("` could hold Ignition resources but was not read (")
                && found[0].2.contains(what),
            "{found:?}"
        );
    };
    std::os::unix::fs::symlink(root.join("gone"), root.join("projects/B")).unwrap();
    unknown("broken symbolic link");
    std::fs::remove_file(root.join("projects/B")).unwrap();
    std::os::unix::fs::symlink(root, root.join("tools")).unwrap();
    unknown("symbolic link to a directory");
    std::fs::remove_file(root.join("tools")).unwrap();
    let locked = root.join("vendor");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_err() {
        unknown("cannot read directory");
    }
    std::fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(fixes(root).len(), 1);
    assert_eq!(fixes(root)[0].1, "safe");
}

/// Reviewer round 15 B2: a resource file inside a project below a
/// directory that the walk from the repository skips by name is not said
/// to be outside every project. Review round 17 (Opus): nor is it linted
/// against the repository's workspace, whose walk does not reach it: the
/// root is the nearest candidate whose walk does (here the layout root
/// around the project), so REF009 has nothing to say about the walk.
#[test]
fn files_below_skipped_directories_get_a_root_that_reaches_them() {
    let dir = closed_world_repository("\tprint 1\n");
    let root = dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let pkg = root.join("node_modules/pkg");
    let hidden = pkg.join("projects/P/ignition/script-python/n");
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(pkg.join("projects/P/project.json"), "{}").unwrap();
    std::fs::write(
        hidden.join("resource.json"),
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    )
    .unwrap();
    std::fs::write(hidden.join("code.py"), "x = 1\n").unwrap();
    for path in [hidden.join("resource.json"), hidden.join("code.py")] {
        let out = iglint(root)
            .args(["check", "--no-cache", "--select", "REF009"])
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!text.contains("did not reach"), "{text}");
        assert!(!text.contains("is not inside a project"), "{text}");
        assert_eq!(out.status.code(), Some(0), "{text}");
        let out = iglint(root)
            .args(["check", "--no-cache", "--show-settings"])
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.contains(&format!("Workspace root: {}\n", pkg.display())),
            "{text}"
        );
    }
}

/// With no workspace root around the file (a project directory on its
/// own, linted from elsewhere, outside any Git work tree, configuration
/// or `projects/` layout), the closed world is unknown: the fix that needs
/// it is unsafe, with that reason.
#[test]
fn fixes_that_need_the_closed_world_are_unsafe_without_a_root() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("lone/A");
    std::fs::create_dir_all(project.join("ignition/script-python/m")).unwrap();
    std::fs::write(
        project.join("project.json"),
        r#"{"title": "A", "enabled": true, "inheritable": false}"#,
    )
    .unwrap();
    std::fs::write(
        project.join("ignition/script-python/m/resource.json"),
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    )
    .unwrap();
    let module = project.join("ignition/script-python/m/code.py");
    std::fs::write(&module, CLOSED_WORLD_MODULE).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let out = iglint(elsewhere.path())
        .args([
            "check",
            "--no-cache",
            "--select",
            "SCR102",
            "--output-format",
            "json",
        ])
        .arg(&module)
        .output()
        .unwrap();
    let fixes = scr102_fixes(&out);
    assert_eq!(fixes.len(), 1, "{fixes:?}");
    assert_eq!(fixes[0].0, "unsafe");
    assert!(
        fixes[0]
            .1
            .starts_with("closed world unknown: no workspace root"),
        "{fixes:?}"
    );
    // Linted from the directory that holds the project, the root is known.
    let out = iglint(&dir.path().join("lone"))
        .args([
            "check",
            "--no-cache",
            "--select",
            "SCR102",
            "--output-format",
            "json",
        ])
        .arg(&module)
        .output()
        .unwrap();
    assert_eq!(scr102_fixes(&out), [("safe".to_owned(), String::new())]);
}

/// Review round 14: with lone CR line ends, Jython numbers lines by `\n`
/// only (a lone CR is a character of the line), and iglint showed
/// `SCR001` a line further down. Every output format now shows Jython's
/// line, and the column of the same character (Jython counts UTF-16 units
/// from 0, iglint characters from 1), checked against the parser oracle's
/// recorded cases (`crates/iglint_python/tests/oracle/line_endings.*`).
#[test]
fn positions_follow_jython_lines_with_lone_cr() {
    let oracle = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../iglint_python/tests/oracle/line_endings");
    let cases = std::fs::read_to_string(oracle.with_extension("cases")).unwrap();
    let expected = std::fs::read_to_string(oracle.with_extension("expected")).unwrap();
    let records: Vec<&str> = expected.split("### ").skip(1).collect();
    let mut checked = 0;
    for (case, record) in cases.split("### ").skip(1).zip(records) {
        let (name, source) = case.split_once('\n').unwrap();
        let source = if name.contains("[cr]") {
            source.replace('\n', "\r")
        } else if name.contains("[crlf]") {
            source.replace('\n', "\r\n")
        } else {
            source.to_owned()
        };
        let verdict = record.lines().nth(1).unwrap();
        let Some(at) = verdict.strip_prefix("error ") else {
            continue;
        };
        let mut parts = at.split(' ').map(|p| p.parse::<usize>().unwrap());
        let (line, units) = (parts.next().unwrap(), parts.next().unwrap());
        // The character at Jython's UTF-16 column of Jython's line.
        let text = source.split('\n').nth(line - 1).unwrap();
        let mut column = 1;
        let mut seen = 0;
        for c in text.chars() {
            if seen >= units {
                break;
            }
            seen += c.len_utf16();
            column += 1;
        }
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
        std::fs::write(module.join("code.py"), &source).unwrap();
        let run = |format: &str| -> String {
            let out = iglint(dir.path())
                .args([
                    "--no-cache",
                    "--select",
                    "SCR001",
                    "--output-format",
                    format,
                ])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let json: serde_json::Value = serde_json::from_str(&run("json")).unwrap();
        assert_eq!(json[0]["location"]["row"], line, "{name}: {json}");
        assert_eq!(json[0]["location"]["column"], column, "{name}: {json}");
        let concise = run("concise");
        assert!(
            concise.contains(&format!("code.py:{line}:{column}: SCR001")),
            "{name}: {concise}"
        );
        let text = run("text");
        assert!(
            text.contains(&format!("code.py:{line}:{column}: SCR001")),
            "{name}: {text}"
        );
        let sarif: serde_json::Value = serde_json::from_str(&run("sarif")).unwrap();
        let region = &sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region["startLine"], line, "{name}: {region}");
        assert_eq!(region["startColumn"], column, "{name}: {region}");
        checked += 1;
    }
    assert!(checked >= 7, "{checked}");
}
