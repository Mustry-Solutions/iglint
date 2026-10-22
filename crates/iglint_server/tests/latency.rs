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
use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
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
fn measure(client: &mut Client, path: &Path, text: &str, at: Position, rounds: usize) -> Vec<Duration> {
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
        client.diagnostics(&uri, Some(version));
        samples.push(start.elapsed());
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

    for debounce in [0u64, 15] {
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
            assert!(s.median < Duration::from_millis(50), "median {:?}", s.median);
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
    client.request_raw("workspace/executeCommand", json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}));
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
            (t.ends_with('"') && t.len() > 1)
                .then(|| (n, t[..t.len() - 1].encode_utf16().count()))
        }) else {
            continue;
        };
        let samples = measure(&mut client, path, &text, Position::new(line as u32, column as u32), 10);
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
