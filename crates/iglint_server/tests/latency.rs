//! Latency from a change to published diagnostics on a large view.
//!
//! `large_view_latency` runs on a generated view (invented content) and
//! asserts the 50 ms target in optimised builds:
//!
//! ```text
//! cargo test --release -p iglint_server --test latency -- --nocapture
//! ```
//!
//! `private_corpus_latency` measures the largest views of a private corpus
//! and prints aggregates only:
//!
//! ```text
//! IGLINT_PRIVATE_CORPUS=/path cargo test --release -p iglint_server \
//!     --test latency -- --ignored --nocapture
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;
use lsp_types::{Position, Range, TextDocumentContentChangeEvent, Url};
use serde_json::json;

/// A view with `n` components: nested containers of labels with expression
/// and property bindings, buttons with script actions, custom properties.
fn large_view(n: usize) -> String {
    let mut children = Vec::new();
    for i in 0..n {
        let component = match i % 3 {
            0 => format!(
                r#"{{"type": "ia.display.label", "meta": {{"name": "Label{i}"}},
 "custom": {{"value{i}": {i}}},
 "propConfig": {{"props.text": {{"binding": {{"type": "expr", "config": {{"expression": "if({{this.custom.value{i}}} > 10, 'high ' + {{view.params.title}}, toStr({{this.custom.value{i}}}))"}}}}}}}},
 "props": {{"style": {{"classes": ""}}}}}}"#
            ),
            1 => format!(
                r#"{{"type": "ia.input.button", "meta": {{"name": "Button{i}"}},
 "props": {{"text": "Go {i}"}},
 "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "\tvalue = self.view.params.title\n\tif value:\n\t\tsystem.perspective.print('clicked %s' % value)\n\tfor k in range(3):\n\t\tvalue = value + str(k)\n\tself.view.custom.last = value"}}}}}}}}}}"#
            ),
            _ => format!(
                r#"{{"type": "ia.display.label", "meta": {{"name": "Prop{i}"}},
 "propConfig": {{"props.text": {{"binding": {{"type": "property", "config": {{"path": "view.params.title"}},
   "transforms": [{{"type": "script", "code": "\treturn str(value) + '{i}'"}}]}}}}}}}}"#
            ),
        };
        children.push(component);
    }
    let groups: Vec<String> = children
        .chunks(50)
        .enumerate()
        .map(|(g, chunk)| {
            format!(
                r#"{{"type": "ia.container.flex", "meta": {{"name": "Group{g}"}}, "props": {{"direction": "column"}},
 "children": [{}]}}"#,
                chunk.join(",\n")
            )
        })
        .collect();
    format!(
        r#"{{
  "custom": {{"last": ""}},
  "params": {{"title": "x"}},
  "propConfig": {{"params.title": {{"paramDirection": "input"}}, "custom.last": {{"persistent": true}}}},
  "props": {{"defaultSize": {{"height": 800, "width": 1200}}}},
  "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [{}]}}
}}"#,
        groups.join(",\n")
    )
}

struct Stats {
    median: Duration,
    p95: Duration,
    max: Duration,
}

fn stats(mut samples: Vec<Duration>) -> Stats {
    samples.sort();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Stats {
        median: at(0.5),
        p95: at(0.95),
        max: *samples.last().unwrap(),
    }
}

/// Opens `path`, then types `rounds` characters at `position` and measures
/// each change until its diagnostics arrive.
fn measure(
    client: &mut Client,
    path: &Path,
    text: &str,
    at: Position,
    rounds: usize,
) -> Vec<Duration> {
    let uri = client.open(path, text);
    client.diagnostics(&uri, Some(1));
    let mut samples = Vec::new();
    for round in 0..rounds {
        let version = round as i32 + 2;
        let position = Position::new(at.line, at.character + round as u32);
        let start = Instant::now();
        client.change(
            &uri,
            version,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(position, position)),
                range_length: None,
                text: "x".to_owned(),
            }],
        );
        let published = client.diagnostics(&uri, Some(version));
        samples.push(start.elapsed());
        // The edit keeps the file valid: the measurement is of a full check.
        assert!(!codes(&published).contains(&"VIEW001".to_owned()));
        client.drain();
    }
    samples
}

#[test]
fn large_view_latency() {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    demo_project(&root);
    write(&root, &format!("{VIEWS}/Large/resource.json"), MANIFEST);
    let text = large_view(3000);
    let path = write(&root, &format!("{VIEWS}/Large/view.json"), &text);
    // Inside the first script.
    let (line, column) = text
        .lines()
        .enumerate()
        .find_map(|(n, l)| l.find("\\tvalue = self").map(|c| (n, c + 2)))
        .unwrap();
    let at = Position::new(line as u32, column as u32);

    for debounce in [0u64, 10] {
        let mut client = Client::start(Some(&root), json!({"debounceMs": debounce}));
        let samples = measure(&mut client, &path, &text, at, 15);
        let s = stats(samples);
        println!(
            "large view ({} KiB, 3000 components), debounce {debounce} ms: median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            text.len() / 1024,
            s.median.as_secs_f64() * 1000.0,
            s.p95.as_secs_f64() * 1000.0,
            s.max.as_secs_f64() * 1000.0,
        );
        if !cfg!(debug_assertions) && debounce == 0 {
            assert!(
                s.median < Duration::from_millis(50),
                "median {:?}",
                s.median
            );
        }
        client.shutdown();
    }
}

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn private_corpus_latency() {
    let Some(corpus) = std::env::var_os("IGLINT_PRIVATE_CORPUS") else {
        eprintln!("IGLINT_PRIVATE_CORPUS is not set");
        return;
    };
    let corpus = iglint_linter::discovery::absolute(Path::new(&corpus));
    let mut views: Vec<(u64, PathBuf)> = Vec::new();
    let mut stack = vec![corpus.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "view.json") {
                views.push((entry.metadata().map(|m| m.len()).unwrap_or(0), path));
            }
        }
    }
    views.sort();
    let total: u64 = views.iter().map(|v| v.0).sum();
    let start = Instant::now();
    let mut client = Client::start(Some(&corpus), json!({"debounceMs": 0}));
    // The initialize handshake returns before the folder is loaded; the
    // first request is answered after it.
    client.request_raw(
        "workspace/executeCommand",
        json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}),
    );
    let load = start.elapsed();
    println!(
        "corpus: {} views, {} MiB of views, workspace load {:.0} ms",
        views.len(),
        total / (1024 * 1024),
        load.as_secs_f64() * 1000.0
    );
    let mut all = Vec::new();
    for (size, path) in views.iter().rev().take(5) {
        let text = std::fs::read_to_string(path).unwrap();
        // Type at the end of the first line that holds a string value.
        let Some((line, column)) = text.lines().enumerate().find_map(|(n, l)| {
            let t = l.trim_end();
            let t = t.trim_end_matches(',');
            t.strip_suffix('"')
                .filter(|t| !t.is_empty())
                .map(|t| (n, t.encode_utf16().count()))
        }) else {
            continue;
        };
        let samples = measure(
            &mut client,
            path,
            &text,
            Position::new(line as u32, column as u32),
            10,
        );
        let s = stats(samples.clone());
        println!(
            "view of {} KiB: median {:.1} ms, p95 {:.1} ms",
            size / 1024,
            s.median.as_secs_f64() * 1000.0,
            s.p95.as_secs_f64() * 1000.0
        );
        all.extend(samples);
    }
    if !all.is_empty() {
        let s = stats(all);
        println!(
            "5 largest views: median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            s.median.as_secs_f64() * 1000.0,
            s.p95.as_secs_f64() * 1000.0,
            s.max.as_secs_f64() * 1000.0
        );
    }
    client.shutdown();
}

/// A workspace of `modules` library modules calling each other and `views`
/// views calling them (about two files per resource with the manifests).
fn library_workspace(root: &Path, modules: usize, views: usize) -> PathBuf {
    demo_project(root);
    let manifest = MANIFEST.replace("view.json", "code.py");
    let library = "projects/demo/ignition/script-python";
    let mut edited = PathBuf::new();
    for i in 0..modules {
        write(
            root,
            &format!("{library}/lib/m{i}/resource.json"),
            &manifest,
        );
        let previous = if i == 0 {
            "1".to_owned()
        } else {
            format!("lib.m{}.f{}(x)", i - 1, i - 1)
        };
        let path = write(
            root,
            &format!("{library}/lib/m{i}/code.py"),
            &format!(
                "import json\nimport os\ndef f{i}(x):\n    return {previous}\n\ndef value{i}():\n    return json.dumps({i})\n"
            ),
        );
        if i == modules / 2 {
            edited = path;
        }
    }
    for v in 0..views {
        write(root, &format!("{VIEWS}/V{v}/resource.json"), MANIFEST);
        let m = v % modules.max(1);
        write(
            root,
            &format!("{VIEWS}/V{v}/view.json"),
            &format!(
                r#"{{"custom": {{"a": 1, "b": 2}}, "params": {{}},
  "root": {{"type": "ia.container.flex", "meta": {{"name": "root"}}, "children": [
    {{"type": "ia.input.button", "meta": {{"name": "B"}}, "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "\tlib.m{m}.f{m}(self.view.custom.a)"}}}}}}}}}}]}}}}
"#
            ),
        );
    }
    edited
}

/// Review round 30 (Opus minor 1): in workspace scope, typing in a library
/// module (every keystroke renames a function, so every file may change)
/// publishes the edited module before the rest of the workspace is checked
/// again, and the next keystroke does not wait for that check: each
/// keystroke's diagnostics arrive in about the time of one file's check.
#[test]
fn library_edit_latency_in_workspace_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    let path = library_workspace(&root, 1000, 500);
    let text = std::fs::read_to_string(&path).unwrap();
    let (line, column) = text
        .lines()
        .enumerate()
        .find_map(|(n, l)| l.strip_prefix("def f").map(|_| (n, l.find('(').unwrap())))
        .unwrap();
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0, "scope": "workspace"}));
    let uri = client.open(&path, &text);
    client.diagnostics(&uri, Some(1));
    client.drain();
    let mut samples = Vec::new();
    for round in 0..15 {
        let version = round as i32 + 2;
        let position = Position::new(line as u32, (column + round) as u32);
        let start = Instant::now();
        client.change(
            &uri,
            version,
            vec![TextDocumentContentChangeEvent {
                range: Some(Range::new(position, position)),
                range_length: None,
                text: "x".to_owned(),
            }],
        );
        client.diagnostics(&uri, Some(version));
        samples.push(start.elapsed());
        // The next keystroke comes while the workspace is checked again.
        std::thread::sleep(Duration::from_millis(5));
    }
    // Once the keystrokes stop, the rest of the workspace is checked
    // again: `m501` calls the renamed `lib.m500.f500`.
    let caller = Url::from_file_path(
        path.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("m501/code.py"),
    )
    .unwrap();
    let start = Instant::now();
    let published = client.wait_for(|m| match m {
        lsp_server::Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
            let p: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(n.params.clone()).ok()?;
            (p.uri == caller && p.diagnostics.iter().any(|d| d.message.contains("f500")))
                .then_some(p)
        }
        _ => None,
    });
    assert!(!published.diagnostics.is_empty());
    println!(
        "  the rest of the workspace published {:.0} ms after the last keystroke's file",
        start.elapsed().as_secs_f64() * 1000.0
    );
    let s = stats(samples);
    println!(
        "library module, workspace scope (3,000 files): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        s.median.as_secs_f64() * 1000.0,
        s.p95.as_secs_f64() * 1000.0,
        s.max.as_secs_f64() * 1000.0,
    );
    if !cfg!(debug_assertions) {
        assert!(
            s.median < Duration::from_millis(50),
            "median {:?}",
            s.median
        );
    }
    client.shutdown();
}

/// Review round 30 (Opus minor 1): the private corpus in workspace scope,
/// typing in function names of the largest library modules (aggregates
/// only).
#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn private_corpus_library_latency() {
    let Some(corpus) = std::env::var_os("IGLINT_PRIVATE_CORPUS") else {
        eprintln!("IGLINT_PRIVATE_CORPUS is not set");
        return;
    };
    let corpus = iglint_linter::discovery::absolute(Path::new(&corpus));
    let mut modules: Vec<(u64, PathBuf)> = Vec::new();
    let mut stack = vec![corpus.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "code.py")
                && path.components().any(|c| c.as_os_str() == "script-python")
            {
                modules.push((entry.metadata().map(|m| m.len()).unwrap_or(0), path));
            }
        }
    }
    modules.sort();
    let mut client = Client::start(
        Some(&corpus),
        json!({"debounceMs": 0, "scope": "workspace"}),
    );
    let start = Instant::now();
    client.request_raw(
        "workspace/executeCommand",
        json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}),
    );
    let published = client
        .inbox
        .iter()
        .filter(|m| matches!(m, lsp_server::Message::Notification(n) if n.method == "textDocument/publishDiagnostics"))
        .count();
    println!(
        "corpus: {} library modules, workspace scope: {published} files published in {:.0} ms",
        modules.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    client.drain();
    let mut all = Vec::new();
    for (_, path) in modules.iter().rev().take(5) {
        let text = std::fs::read_to_string(path).unwrap();
        let Some((line, column)) = text.lines().enumerate().find_map(|(n, l)| {
            l.strip_prefix("def ")
                .and_then(|rest| rest.find('('))
                .map(|c| (n, c + 4))
        }) else {
            continue;
        };
        let uri = client.open(path, &text);
        client.diagnostics(&uri, Some(1));
        let mut samples = Vec::new();
        for round in 0..10 {
            let version = round as i32 + 2;
            let position = Position::new(line as u32, (column + round) as u32);
            let start = Instant::now();
            client.change(
                &uri,
                version,
                vec![TextDocumentContentChangeEvent {
                    range: Some(Range::new(position, position)),
                    range_length: None,
                    text: "x".to_owned(),
                }],
            );
            client.diagnostics(&uri, Some(version));
            samples.push(start.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        client.request_raw(
            "workspace/executeCommand",
            json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}),
        );
        println!(
            "  the rest of the workspace checked {:.0} ms after the last keystroke",
            start.elapsed().as_secs_f64() * 1000.0
        );
        client.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        );
        client.drain();
        all.extend(samples);
    }
    if !all.is_empty() {
        let s = stats(all);
        println!(
            "5 largest library modules: median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
            s.median.as_secs_f64() * 1000.0,
            s.p95.as_secs_f64() * 1000.0,
            s.max.as_secs_f64() * 1000.0
        );
    }
    client.shutdown();
}

/// The time of [`LiveWorkspace::refresh_from_disk`] on the workspace of
/// folder `root` when nothing changed: every file and directory compared
/// with what was read (review round 31, B1).
fn refresh_cost(root: &Path, rounds: usize) -> (usize, usize, Stats) {
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: root.to_path_buf(),
            start: Some(root.to_path_buf()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .unwrap();
    let mut live =
        iglint_linter::live::LiveWorkspace::new(config.settings, vec![root.to_path_buf()]);
    let files = live.workspace().files.len();
    let directories = live.workspace().directories.len();
    // The first refresh settles what was read within the slack.
    live.refresh_from_disk();
    let samples = (0..rounds)
        .map(|_| {
            let start = Instant::now();
            assert_eq!(
                live.refresh_from_disk(),
                iglint_linter::live::DiskChange::Unchanged
            );
            start.elapsed()
        })
        .collect();
    (files, directories, stats(samples))
}

/// Code actions over the whole of `path`: the time of each list request
/// (from the live workspace), and of resolving the first fix it offers
/// (review round 32: the edit comes from a fresh check, discovery
/// included). `None` when no fix was offered.
fn code_action_latency(client: &mut Client, path: &Path, rounds: usize) -> Option<(Stats, Stats)> {
    let text = std::fs::read_to_string(path).unwrap();
    let uri = client.open(path, &text);
    client.diagnostics(&uri, Some(1));
    let mut resolved = Vec::new();
    let samples = (0..rounds)
        .map(|_| {
            let start = Instant::now();
            let actions = client.request_raw(
                "textDocument/codeAction",
                json!({"textDocument": {"uri": uri}, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 100000, "character": 0}}, "context": {"diagnostics": []}}),
            );
            let elapsed = start.elapsed();
            let fix = actions.as_array().and_then(|a| {
                a.iter()
                    .find(|a| a.get("data").is_some_and(|d| d.get("fix").is_some()))
                    .cloned()
            });
            if let Some(fix) = fix {
                let start = Instant::now();
                let action = client.request_raw("codeAction/resolve", fix);
                let took = start.elapsed();
                if action.get("edit").is_some() {
                    resolved.push(took);
                }
            }
            elapsed
        })
        .collect();
    // A fresh check that disagrees with the live workspace makes the
    // server discover the folder again; on an unchanged disk it must not.
    let diverged = client.inbox.iter().any(|m| {
        matches!(m, lsp_server::Message::Notification(n)
            if n.method == "window/logMessage"
                && n.params["message"].as_str().is_some_and(|t| t.contains("differs from a fresh check")))
    });
    assert!(!diverged, "the live workspace disagrees with a fresh check");
    client.notify(
        "textDocument/didClose",
        json!({"textDocument": {"uri": uri}}),
    );
    client.drain();
    (!resolved.is_empty()).then(|| (stats(samples), stats(resolved)))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Review round 31 (B1): before a fix, the server compares every file and
/// directory of the workspace with what it read. On 3,000 generated files
/// that costs a few milliseconds, and a code action with a fix stays
/// responsive.
#[test]
fn fix_request_latency() {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    let path = library_workspace(&root, 1000, 500);
    // Files dated within the slack of their read are compared by content
    // until they settle.
    std::thread::sleep(Duration::from_millis(2200));
    let (files, directories, refresh) = refresh_cost(&root, 20);
    println!(
        "refresh from disk ({files} files, {directories} directories): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(refresh.median),
        ms(refresh.p95),
        ms(refresh.max)
    );
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let (list, fix) = code_action_latency(&mut client, &path, 15).expect("a fix");
    println!(
        "code action list: median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(list.median),
        ms(list.p95),
        ms(list.max)
    );
    println!(
        "resolving a fix (fresh check): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(fix.median),
        ms(fix.p95),
        ms(fix.max)
    );
    if !cfg!(debug_assertions) {
        assert!(
            refresh.median < Duration::from_millis(100),
            "{:?}",
            refresh.median
        );
    }
    client.shutdown();
}

/// Review round 31 (B1): the same on a private corpus (aggregates only):
/// the check of the disk alone, and code actions on the largest library
/// modules.
#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn private_corpus_fix_request_latency() {
    let Some(corpus) = std::env::var_os("IGLINT_PRIVATE_CORPUS") else {
        eprintln!("IGLINT_PRIVATE_CORPUS is not set");
        return;
    };
    let corpus = iglint_linter::discovery::absolute(Path::new(&corpus));
    let (files, directories, refresh) = refresh_cost(&corpus, 20);
    println!(
        "refresh from disk ({files} files, {directories} directories): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(refresh.median),
        ms(refresh.p95),
        ms(refresh.max)
    );
    let mut modules: Vec<(u64, PathBuf)> = Vec::new();
    let mut stack = vec![corpus.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "code.py")
                && path.components().any(|c| c.as_os_str() == "script-python")
            {
                modules.push((entry.metadata().map(|m| m.len()).unwrap_or(0), path));
            }
        }
    }
    modules.sort();
    let mut client = Client::start(Some(&corpus), json!({"debounceMs": 0}));
    let mut all = Vec::new();
    let mut fixes = Vec::new();
    // The largest library modules and views that offer a fix.
    let mut views: Vec<(u64, PathBuf)> = Vec::new();
    let mut stack = vec![corpus.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "view.json") {
                views.push((entry.metadata().map(|m| m.len()).unwrap_or(0), path));
            }
        }
    }
    views.sort();
    for list in [&modules, &views] {
        let mut measured = 0;
        for (_, path) in list.iter().rev() {
            if measured == 5 {
                break;
            }
            if let Some((s, f)) = code_action_latency(&mut client, path, 10) {
                all.extend([s.median, s.p95, s.max]);
                fixes.extend([f.median, f.p95, f.max]);
                measured += 1;
            }
        }
    }
    let s = stats(all);
    println!(
        "code action lists on the largest library modules and views with fixes (whole file): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(s.median),
        ms(s.p95),
        ms(s.max)
    );
    let f = stats(fixes);
    println!(
        "resolving a fix there (fresh check): median {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms(f.median),
        ms(f.p95),
        ms(f.max)
    );
    client.shutdown();
}
