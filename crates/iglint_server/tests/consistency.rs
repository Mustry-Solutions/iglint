//! Review round 29 (Opus, B2): after any sequence of edits, what the
//! language server publishes and the quick fixes it offers as preferred
//! (safe) are what a fresh `iglint check` of the same files reports.
//!
//! Generated libraries whose modules import each other, star-import each
//! other, reach names through chains of imports, store and delete names of
//! other modules, and a view that hands a component to library functions
//! (`VIEW015` through two calls). Each step replaces one file: an open
//! document through the editor, or a file that is not open on disk with a
//! `workspace/didChangeWatchedFiles` notification. After each step the
//! diagnostics last published for every open document, and the preferred
//! quick fixes of each, are compared with a fresh check of a copy of the
//! workspace.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use common::*;
use lsp_server::Message;
use lsp_types::notification::{DidChangeWatchedFiles, Notification as _, PublishDiagnostics};
use lsp_types::request::CodeActionRequest;
use lsp_types::{
    CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams, Position, Range,
    TextDocumentIdentifier, Url,
};
use serde_json::json;

const LIBRARY: &str = "projects/p/ignition/script-python";
const VIEW: &str = "projects/p/com.inductiveautomation.perspective/views/V";

/// A file of the workspace and the texts it may have.
struct Slot {
    rel: String,
    open: bool,
    variants: Vec<String>,
}

fn view(script: &str) -> String {
    format!(
        r#"{{"params": {{}},
  "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{{"meta": {{"name": "Btn"}}, "type": "ia.input.button", "custom": {{"foo": 1, "bar": 2}}, "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}}}]}}}}
"#
    )
}

fn slots() -> Vec<Slot> {
    let module = |name: &str, open: bool, variants: &[&str]| Slot {
        rel: format!("{LIBRARY}/{name}/code.py"),
        open,
        variants: variants.iter().map(|v| (*v).to_owned()).collect(),
    };
    vec![
        module(
            "lib/util",
            true,
            &[
                "import json\nimport os\ndef f():\n    return 1\n",
                "import json\ndef f():\n    return json.dumps(1)\n",
                "import os\n__all__ = ['f']\ndef f():\n    return 1\n",
                "import json\nfrom lib.two import *\ndef f():\n    return k\n",
            ],
        ),
        module(
            "lib/two",
            true,
            &[
                "def k(c):\n    return c.custom.foo\n",
                "import json\ndef k(c):\n    return c.custom.bar\n",
                "from lib import util\ndef k(c):\n    return util.f()\n",
                "def k(c):\n    return lib.util.json\n",
            ],
        ),
        module(
            "lib/a",
            false,
            &[
                "def f(c):\n    return lib.two.k(c)\n",
                "def f(c):\n    return 1\n",
                "import lib.two as t\ndef f(c):\n    return t.k(c)\n",
            ],
        ),
        module(
            "m0",
            true,
            &[
                "from lib import util\n",
                "import m1\n",
                "from lib.util import *\n",
                "def x():\n    return 1\n",
                "import lib.two as t\n",
                "from m1 import *\n",
            ],
        ),
        module(
            "m1",
            false,
            &[
                "import m2\n",
                "def x():\n    return 1\n",
                "from lib import util\n",
                "import m0\n",
                "import json\n",
            ],
        ),
        module(
            "m2",
            false,
            &[
                "from lib import util\n",
                "import m1\n",
                "def y():\n    return 1\n",
                "from lib.two import *\n",
            ],
        ),
        module(
            "b",
            true,
            &[
                "def g(n):\n    return m0.m1.m2.util.json.dumps(1)\n",
                "def g(n):\n    return m0.util.json.dumps(1)\n",
                "def g(n):\n    del m0.util.json\n",
                "def g(n):\n    return lib.util.f()\n",
                "def g(n):\n    m1.util.json = 5\n",
                "def g(n):\n    return m0.t.k(n)\n",
                "import os\ndef g(n):\n    return m0.x()\n",
            ],
        ),
        Slot {
            rel: format!("{VIEW}/view.json"),
            open: true,
            variants: [
                r"\tlib.a.f(self)",
                r"\tpass",
                r"\tsystem.perspective.print(self.custom.foo)",
                r"\tlib.two.k(self)",
            ]
            .iter()
            .map(|s| view(s))
            .collect(),
        },
    ]
}

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        ((z ^ (z >> 31)) % n as u64) as usize
    }
}

/// A finding as compared: rule, start, end and message.
type Finding = (String, (u32, u32), (u32, u32), String);

/// Where a finding with a fix starts: rule and start.
type FixAt = (String, (u32, u32));

/// By file: the findings, and the starts of those with a safe fix.
type Report = BTreeMap<String, (BTreeSet<Finding>, BTreeSet<FixAt>)>;

fn manifest(rel: &str) -> String {
    let file = Path::new(rel).file_name().unwrap().to_string_lossy();
    MANIFEST.replace("view.json", &file)
}

fn write_workspace(root: &Path, slots: &[Slot], texts: &[String]) {
    write(
        root,
        "projects/p/project.json",
        r#"{"title": "p", "enabled": true, "inheritable": false}"#,
    );
    for (slot, text) in slots.iter().zip(texts) {
        let dir = Path::new(&slot.rel).parent().unwrap();
        write(
            root,
            &format!("{}/resource.json", dir.display()),
            &manifest(&slot.rel),
        );
        write(root, &slot.rel, text);
    }
}

/// Line and UTF-16 column of a byte offset in ASCII text.
fn position(text: &str, offset: usize) -> (u32, u32) {
    let before = &text.as_bytes()[..offset.min(text.len())];
    let line = before.iter().filter(|b| **b == b'\n').count();
    let column = before.iter().rev().take_while(|b| **b != b'\n').count();
    (line as u32, column as u32)
}

/// A fresh check of the files: findings and the starts of findings with a
/// safe fix, by file.
fn fresh_check(slots: &[Slot], texts: &[String]) -> Report {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    write_workspace(&root, slots, texts);
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: root.clone(),
            start: Some(root.clone()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .unwrap();
    let result = iglint_linter::check(
        &config.settings,
        std::slice::from_ref(&root),
        &iglint_linter::CheckOptions::default(),
    );
    let mut out = Report::new();
    for d in &result.diagnostics {
        let rel = d
            .file
            .path()
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let text = d.file.text();
        let start = position(text, d.range.start().to_usize());
        let end = position(text, d.range.end().to_usize());
        let entry = out.entry(rel).or_default();
        entry
            .0
            .insert((d.rule.code().to_owned(), start, end, d.message.clone()));
        if d.fix
            .as_ref()
            .is_some_and(|f| f.applicability == iglint_linter::Applicability::Safe)
        {
            entry.1.insert((d.rule.code().to_owned(), start));
        }
    }
    out
}

fn finding(d: &lsp_types::Diagnostic) -> Finding {
    let code = match &d.code {
        Some(lsp_types::NumberOrString::String(s)) => s.clone(),
        _ => String::new(),
    };
    (
        code,
        (d.range.start.line, d.range.start.character),
        (d.range.end.line, d.range.end.character),
        d.message.clone(),
    )
}

/// Moves every publication in the client's inbox into `latest`, after
/// everything the server did before answering a request sent now.
fn settle(client: &mut Client, latest: &mut BTreeMap<Url, BTreeSet<Finding>>) {
    client.request_raw(
        "workspace/executeCommand",
        json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}),
    );
    client.inbox.retain(|m| match m {
        Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
            let p: lsp_types::PublishDiagnosticsParams =
                serde_json::from_value(n.params.clone()).unwrap();
            latest.insert(p.uri, p.diagnostics.iter().map(finding).collect());
            false
        }
        _ => true,
    });
}

/// The starts of the findings whose quick fix is offered as preferred.
fn preferred_fixes(client: &mut Client, uri: &Url) -> BTreeSet<FixAt> {
    let actions = client
        .request::<CodeActionRequest>(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(10_000, 0)),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: Some(vec![CodeActionKind::QUICKFIX]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap_or_default();
    actions
        .into_iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(a) if a.is_preferred == Some(true) => {
                a.diagnostics.as_ref().and_then(|d| d.first()).map(|d| {
                    let f = finding(d);
                    (f.0, f.1)
                })
            }
            _ => None,
        })
        .collect()
}

#[test]
fn published_findings_and_fixes_match_a_fresh_check_after_any_edits() {
    let slots = slots();
    let mut steps = 0;
    let mut compared_fixes = 0;
    // `IGLINT_CONSISTENCY_SEEDS=n` runs `n` sequences (default 8).
    let seeds: u64 = std::env::var("IGLINT_CONSISTENCY_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    for seed in 1..=seeds {
        let mut rng = Rng(seed);
        let dir = tempfile::tempdir().unwrap();
        let root = iglint_linter::discovery::absolute(dir.path());
        let mut texts: Vec<String> = slots
            .iter()
            .map(|s| s.variants[rng.next(s.variants.len())].clone())
            .collect();
        write_workspace(&root, &slots, &texts);
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let mut uris: BTreeMap<usize, (Url, i32)> = BTreeMap::new();
        let mut latest: BTreeMap<Url, BTreeSet<Finding>> = BTreeMap::new();
        for (i, slot) in slots.iter().enumerate() {
            if slot.open {
                let path: PathBuf = root.join(&slot.rel);
                let uri = client.open(&path, &texts[i]);
                let p = client.diagnostics(&uri, Some(1));
                latest.insert(uri.clone(), p.diagnostics.iter().map(finding).collect());
                uris.insert(i, (uri, 1));
            }
        }
        for step in 0..=10 {
            if step > 0 {
                let i = rng.next(slots.len());
                let slot = &slots[i];
                let mut next = rng.next(slot.variants.len());
                if slot.variants[next] == texts[i] {
                    next = (next + 1) % slot.variants.len();
                }
                texts[i] = slot.variants[next].clone();
                match uris.get_mut(&i) {
                    Some((uri, version)) => {
                        *version += 1;
                        let (uri, version) = (uri.clone(), *version);
                        client.replace_all(&uri, version, &texts[i]);
                        let p = client.diagnostics(&uri, Some(version));
                        latest.insert(uri, p.diagnostics.iter().map(finding).collect());
                    }
                    None => {
                        let path = root.join(&slot.rel);
                        std::fs::write(&path, &texts[i]).unwrap();
                        client.notify(
                            DidChangeWatchedFiles::METHOD,
                            json!({"changes": [{"uri": Url::from_file_path(&path).unwrap(), "type": 2}]}),
                        );
                    }
                }
            }
            settle(&mut client, &mut latest);
            let fresh = fresh_check(&slots, &texts);
            for (i, (uri, _)) in &uris {
                let rel = &slots[*i].rel;
                let (expected, safe) = fresh.get(rel).cloned().unwrap_or_default();
                let published = latest.get(uri).cloned().unwrap_or_default();
                assert_eq!(
                    published, expected,
                    "seed {seed} step {step}: {rel} published\n{texts:#?}"
                );
                let offered = preferred_fixes(&mut client, uri);
                assert_eq!(
                    offered, safe,
                    "seed {seed} step {step}: {rel} preferred fixes\n{texts:#?}"
                );
                compared_fixes += safe.len();
                // Code actions publish nothing new when nothing is stale.
                settle(&mut client, &mut latest);
                assert_eq!(latest.get(uri).cloned().unwrap_or_default(), expected);
            }
            steps += 1;
        }
        client.shutdown();
    }
    assert_eq!(steps, 11 * seeds as usize);
    assert!(compared_fixes > 20, "{compared_fixes}");
}

/// What a fresh `iglint check --fix` (safe fixes, not written) makes of
/// each file that changes, by file.
fn fresh_fixes(slots: &[Slot], texts: &[String]) -> BTreeMap<String, String> {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    write_workspace(&root, slots, texts);
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: root.clone(),
            start: Some(root.clone()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .unwrap();
    let result = iglint_linter::check(
        &config.settings,
        std::slice::from_ref(&root),
        &iglint_linter::CheckOptions {
            fix_mode: iglint_linter::FixMode::DiffOnly {
                unsafe_fixes: false,
            },
            ..iglint_linter::CheckOptions::default()
        },
    );
    result
        .fixed
        .iter()
        .filter(|f| f.fixed != *f.original)
        .map(|f| {
            let rel = f
                .path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, f.fixed.clone())
        })
        .collect()
}

/// The text "fix all" of `uri` makes of `text`, if offered (resolved when
/// the server defers the edit).
fn fix_all_result(client: &mut Client, uri: &Url, text: &str) -> Option<String> {
    let actions = client
        .request::<CodeActionRequest>(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(Position::new(0, 0), Position::new(0, 0)),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only: Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap_or_default();
    let action = actions.into_iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(a) => Some(a),
        CodeActionOrCommand::Command(_) => None,
    })?;
    let action = if action.edit.is_none() {
        client.request::<lsp_types::request::CodeActionResolveRequest>(action)
    } else {
        action
    };
    let edits = action.edit?.changes?.remove(uri)?;
    Some(apply(text, &edits))
}

/// Review round 30 (Codex, B2): with any run trigger (on type without and
/// with a long debounce, on save) and code actions asked for at any point
/// between edits (before the diagnostics of an edit arrive, after opening
/// and closing documents, after saves), every quick fix offered as
/// preferred is one a fresh check of the editor's current texts reports as
/// safe, and "fix all" gives what a fresh `--fix` gives. On type without a
/// debounce, once the diagnostics of every edit arrived, the offered safe
/// fixes are exactly those of the fresh check.
#[test]
fn fixes_offered_between_edits_match_a_fresh_check() {
    let slots = slots();
    // `IGLINT_CONSISTENCY_SEEDS=n` runs `n` sequences (default 9).
    let seeds: u64 = std::env::var("IGLINT_CONSISTENCY_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(9);
    let mut asked = 0;
    let mut offered_total = 0;
    let mut fix_alls = 0;
    for seed in 1..=seeds {
        let mut rng = Rng(seed * 7919);
        let (init, settled_mode) = match seed % 3 {
            0 => (json!({"run": "onType", "debounceMs": 0}), true),
            1 => (json!({"run": "onType", "debounceMs": 60000}), false),
            _ => (json!({"run": "onSave"}), false),
        };
        let dir = tempfile::tempdir().unwrap();
        let root = iglint_linter::discovery::absolute(dir.path());
        let mut disk: Vec<String> = slots
            .iter()
            .map(|s| s.variants[rng.next(s.variants.len())].clone())
            .collect();
        write_workspace(&root, &slots, &disk);
        let mut editor: Vec<Option<(Url, i32, String)>> = vec![None; slots.len()];
        let mut client = Client::start(Some(&root), init.clone());
        // Versions whose diagnostics the settled mode waits for.
        let mut awaiting: Vec<(Url, i32)> = Vec::new();
        for step in 0..24 {
            let i = rng.next(slots.len());
            let slot = &slots[i];
            let variant = slot.variants[rng.next(slot.variants.len())].clone();
            match (rng.next(6), editor[i].is_some()) {
                // Open with the disk text.
                (0, false) => {
                    let path = root.join(&slot.rel);
                    let uri = client.open(&path, &disk[i]);
                    awaiting.push((uri.clone(), 1));
                    editor[i] = Some((uri, 1, disk[i].clone()));
                }
                // Close: the disk text again.
                (0, true) => {
                    let (uri, _, _) = editor[i].take().unwrap();
                    awaiting.retain(|(u, _)| *u != uri);
                    client.notify(
                        lsp_types::notification::DidCloseTextDocument::METHOD,
                        json!({"textDocument": {"uri": uri}}),
                    );
                }
                // Save: the editor text on disk.
                (1, true) => {
                    let (uri, _, text) = editor[i].clone().unwrap();
                    std::fs::write(root.join(&slot.rel), &text).unwrap();
                    disk[i] = text;
                    client.notify(
                        lsp_types::notification::DidSaveTextDocument::METHOD,
                        json!({"textDocument": {"uri": uri}}),
                    );
                }
                // Type in an open document.
                (_, true) => {
                    let (uri, version, text) = editor[i].as_mut().unwrap();
                    *version += 1;
                    *text = variant;
                    let (uri, version, text) = (uri.clone(), *version, text.clone());
                    client.replace_all(&uri, version, &text);
                    awaiting.retain(|(u, _)| *u != uri);
                    awaiting.push((uri, version));
                }
                // Change a file that is not open, on disk.
                (_, false) => {
                    let path = root.join(&slot.rel);
                    std::fs::write(&path, &variant).unwrap();
                    disk[i] = variant;
                    client.notify(
                        DidChangeWatchedFiles::METHOD,
                        json!({"changes": [{"uri": Url::from_file_path(&path).unwrap(), "type": 2}]}),
                    );
                }
            }
            let texts: Vec<String> = (0..slots.len())
                .map(|k| match &editor[k] {
                    Some((_, _, t)) => t.clone(),
                    None => disk[k].clone(),
                })
                .collect();
            // Ask for fixes now, before any diagnostics arrived, in one or
            // two open documents.
            let open: Vec<usize> = (0..slots.len()).filter(|k| editor[*k].is_some()).collect();
            if open.is_empty() {
                continue;
            }
            let fresh = fresh_check(&slots, &texts);
            let fixed = fresh_fixes(&slots, &texts);
            for _ in 0..1 + rng.next(2) {
                let k = open[rng.next(open.len())];
                let (uri, _, text) = editor[k].clone().unwrap();
                let rel = &slots[k].rel;
                let safe = fresh.get(rel).map(|f| f.1.clone()).unwrap_or_default();
                let offered = preferred_fixes(&mut client, &uri);
                assert!(
                    offered.is_subset(&safe),
                    "seed {seed} ({init}) step {step}: {rel} offers {offered:?}, fresh {safe:?}\n{texts:#?}"
                );
                offered_total += offered.len();
                if let Some(result) = fix_all_result(&mut client, &uri, &text) {
                    assert_eq!(
                        Some(&result),
                        fixed.get(rel),
                        "seed {seed} ({init}) step {step}: {rel} fix all\n{texts:#?}"
                    );
                    fix_alls += 1;
                }
                asked += 1;
            }
            if settled_mode {
                // Every edit analysed: exactly the fresh check's.
                for (uri, version) in std::mem::take(&mut awaiting) {
                    client.diagnostics(&uri, Some(version));
                }
                let mut latest = BTreeMap::new();
                settle(&mut client, &mut latest);
                for k in &open {
                    let (uri, _, _) = editor[*k].clone().unwrap();
                    let rel = &slots[*k].rel;
                    let safe = fresh.get(rel).map(|f| f.1.clone()).unwrap_or_default();
                    assert_eq!(
                        preferred_fixes(&mut client, &uri),
                        safe,
                        "seed {seed} ({init}) step {step}: {rel} after settling\n{texts:#?}"
                    );
                }
            }
        }
        client.shutdown();
    }
    assert!(asked > 50, "{asked}");
    assert!(offered_total > 5, "{offered_total}");
    assert!(fix_alls > 5, "{fix_alls}");
}
