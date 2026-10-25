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

/// Whether an open document other than `target` (by path) has an editor
/// text other than its file's text on disk: the server then computes no
/// fix (review round 33, B1).
fn others_modified(open: &[(PathBuf, String)], target: &Path) -> bool {
    open.iter().any(|(path, text)| {
        path != target && std::fs::read(path).ok().as_deref() != Some(text.as_bytes())
    })
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
                // Fixes are withheld while another open document has
                // unsaved changes (review round 33, B1).
                let open: Vec<(PathBuf, String)> = uris
                    .iter()
                    .map(|(k, (u, _))| (u.to_file_path().unwrap(), texts[*k].clone()))
                    .collect();
                let safe = if others_modified(&open, &root.join(rel)) {
                    BTreeSet::new()
                } else {
                    safe
                };
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
                let documents: Vec<(PathBuf, String)> = editor
                    .iter()
                    .flatten()
                    .map(|(u, _, t)| (u.to_file_path().unwrap(), t.clone()))
                    .collect();
                for k in &open {
                    let (uri, _, _) = editor[*k].clone().unwrap();
                    let rel = &slots[*k].rel;
                    let safe = if others_modified(&documents, &root.join(rel)) {
                        BTreeSet::new()
                    } else {
                        fresh.get(rel).map(|f| f.1.clone()).unwrap_or_default()
                    };
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

/// How the workspace folders lie over the workspace (review round 31,
/// B1): the folder is a child project whose parent is outside it, a
/// directory inside the project, nested folders, or overlapping folders
/// one of which is added after documents were opened.
#[derive(Clone, Copy, Debug)]
enum Layout {
    ChildProject,
    Subdirectory,
    Nested,
    AddedLater,
}

impl Layout {
    /// Where a slot's file lives.
    fn rel(self, rel: &str) -> String {
        match self {
            Layout::ChildProject => {
                let base = ["/m0/", "/m1/", "/m2/", "/b/"]
                    .iter()
                    .any(|m| rel.contains(m));
                rel.replacen(
                    "projects/p/",
                    if base {
                        "projects/base/"
                    } else {
                        "projects/child/"
                    },
                    1,
                )
            }
            _ => rel.to_owned(),
        }
    }

    fn projects(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Layout::ChildProject => vec![
                (
                    "projects/base/project.json",
                    r#"{"title":"base","parent":"","enabled":true,"inheritable":true}"#,
                ),
                (
                    "projects/child/project.json",
                    r#"{"title":"child","parent":"base","enabled":true,"inheritable":false}"#,
                ),
            ],
            _ => vec![(
                "projects/p/project.json",
                r#"{"title": "p", "enabled": true, "inheritable": false}"#,
            )],
        }
    }

    /// The workspace folders (relative; "" is the root), and those added
    /// after the documents were opened.
    fn folders(self) -> (Vec<&'static str>, Vec<&'static str>) {
        match self {
            Layout::ChildProject => (vec!["projects/child"], vec![]),
            Layout::Subdirectory => (vec!["projects/p/ignition/script-python"], vec![]),
            Layout::Nested => (vec!["", "projects/p/ignition/script-python/lib"], vec![]),
            Layout::AddedLater => (
                vec!["projects/p/ignition/script-python/lib"],
                vec!["projects/p/ignition/script-python"],
            ),
        }
    }

    /// The directory the server lints a file from: the innermost folder
    /// that contains it, else its project (a folder of its own).
    fn owner(self, rel: &str, added: bool) -> String {
        let (mut folders, later) = self.folders();
        if added {
            folders.extend(later);
        }
        folders
            .iter()
            .filter(|f| f.is_empty() || rel.starts_with(&format!("{f}/")))
            .max_by_key(|f| f.len())
            .map(|f| (*f).to_owned())
            .unwrap_or_else(|| {
                let project: Vec<&str> = rel.splitn(3, '/').take(2).collect();
                project.join("/")
            })
    }
}

fn write_layout(root: &Path, layout: Layout, slots: &[Slot], texts: &[String]) {
    for (rel, text) in layout.projects() {
        write(root, rel, text);
    }
    for (slot, text) in slots.iter().zip(texts) {
        let rel = layout.rel(&slot.rel);
        let dir = Path::new(&rel).parent().unwrap();
        write(
            root,
            &format!("{}/resource.json", dir.display()),
            &manifest(&rel),
        );
        write(root, &rel, text);
    }
}

/// A fresh `iglint check` and `iglint check --fix` (safe fixes, not
/// written) of the folder `owner` of a copy of the workspace: the starts
/// of the findings with a safe fix and the fixed text of each file, by
/// file (relative to the workspace root).
/// By file: the starts of the findings with a safe fix, and the texts
/// `--fix` makes.
type Fresh = (BTreeMap<String, BTreeSet<FixAt>>, BTreeMap<String, String>);

fn fresh_in(layout: Layout, slots: &[Slot], texts: &[String], owner: &str) -> Fresh {
    let dir = tempfile::tempdir().unwrap();
    let root = iglint_linter::discovery::absolute(dir.path());
    write_layout(&root, layout, slots, texts);
    let folder = if owner.is_empty() {
        root.clone()
    } else {
        root.join(owner)
    };
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: folder.clone(),
            start: Some(folder.clone()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .unwrap();
    let rel = |p: &Path| {
        p.strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut safe: BTreeMap<String, BTreeSet<FixAt>> = BTreeMap::new();
    let result = iglint_linter::check(
        &config.settings,
        std::slice::from_ref(&folder),
        &iglint_linter::CheckOptions::default(),
    );
    for d in &result.diagnostics {
        if d.fix
            .as_ref()
            .is_some_and(|f| f.applicability == iglint_linter::Applicability::Safe)
        {
            let start = position(d.file.text(), d.range.start().to_usize());
            safe.entry(rel(d.file.path()))
                .or_default()
                .insert((d.rule.code().to_owned(), start));
        }
    }
    let result = iglint_linter::check(
        &config.settings,
        std::slice::from_ref(&folder),
        &iglint_linter::CheckOptions {
            fix_mode: iglint_linter::FixMode::DiffOnly {
                unsafe_fixes: false,
            },
            ..iglint_linter::CheckOptions::default()
        },
    );
    let fixed = result
        .fixed
        .iter()
        .filter(|f| f.fixed != *f.original)
        .map(|f| (rel(&f.path), f.fixed.clone()))
        .collect();
    (safe, fixed)
}

/// Review round 31 (Opus and Codex, B1): whatever the workspace folders
/// are (a child project whose parent lies outside the folder, a directory
/// inside the project, nested folders, a folder added after documents
/// were opened) and however files change (typing in any open document,
/// saving, closing, and changes on disk with or without a
/// `didChangeWatchedFiles` notification, also of files outside every
/// folder), every safe fix offered or applied for a file equals what a
/// fresh `iglint check` of the folder that publishes the file reports for
/// the editor's texts and the files on disk: every quick fix offered as
/// preferred is safe there, and "fix all" gives what `--fix` gives.
#[test]
fn fixes_match_a_fresh_check_in_every_folder_layout() {
    let slots = slots();
    // `IGLINT_CONSISTENCY_SEEDS=n` runs `n` sequences per layout (default 3).
    let seeds: u64 = std::env::var("IGLINT_CONSISTENCY_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let mut asked = 0;
    let mut offered_total = 0;
    let mut fix_alls = 0;
    let mut unnotified = 0;
    for layout in [
        Layout::ChildProject,
        Layout::Subdirectory,
        Layout::Nested,
        Layout::AddedLater,
    ] {
        for seed in 1..=seeds {
            let mut rng = Rng(seed * 104_729 + layout as u64);
            let init = match rng.next(3) {
                0 => json!({"run": "onType", "debounceMs": 0}),
                1 => json!({"run": "onType", "debounceMs": 60000}),
                _ => json!({"run": "onSave"}),
            };
            let dir = tempfile::tempdir().unwrap();
            let root = iglint_linter::discovery::absolute(dir.path());
            let mut disk: Vec<String> = slots
                .iter()
                .map(|s| s.variants[rng.next(s.variants.len())].clone())
                .collect();
            write_layout(&root, layout, &slots, &disk);
            let (folders, later) = layout.folders();
            let folder_paths: Vec<PathBuf> = folders
                .iter()
                .map(|f| {
                    if f.is_empty() {
                        root.clone()
                    } else {
                        root.join(f)
                    }
                })
                .collect();
            let folder_refs: Vec<&Path> = folder_paths.iter().map(PathBuf::as_path).collect();
            let mut client =
                Client::start_folders(&folder_refs, init.clone(), client_capabilities());
            let mut added = later.is_empty();
            let mut editor: Vec<Option<(Url, i32, String)>> = vec![None; slots.len()];
            for step in 0..20 {
                if !added && step == 4 {
                    let uri = Url::from_file_path(root.join(later[0])).unwrap();
                    client.notify(
                        lsp_types::notification::DidChangeWorkspaceFolders::METHOD,
                        json!({"event": {"added": [{"uri": uri, "name": "later"}], "removed": []}}),
                    );
                    added = true;
                }
                let i = rng.next(slots.len());
                let rel = layout.rel(&slots[i].rel);
                let path = root.join(&rel);
                let variant = slots[i].variants[rng.next(slots[i].variants.len())].clone();
                match (rng.next(7), editor[i].is_some()) {
                    (0 | 1, false) => {
                        let uri = client.open(&path, &disk[i]);
                        editor[i] = Some((uri, 1, disk[i].clone()));
                    }
                    (0, true) => {
                        let (uri, _, _) = editor[i].take().unwrap();
                        client.notify(
                            lsp_types::notification::DidCloseTextDocument::METHOD,
                            json!({"textDocument": {"uri": uri}}),
                        );
                    }
                    (1, true) => {
                        let (uri, _, text) = editor[i].clone().unwrap();
                        std::fs::write(&path, &text).unwrap();
                        disk[i] = text;
                        client.notify(
                            lsp_types::notification::DidSaveTextDocument::METHOD,
                            json!({"textDocument": {"uri": uri}}),
                        );
                    }
                    (_, true) => {
                        let (uri, version, text) = editor[i].as_mut().unwrap();
                        *version += 1;
                        *text = variant;
                        let (uri, version, text) = (uri.clone(), *version, text.clone());
                        client.replace_all(&uri, version, &text);
                    }
                    (n, false) => {
                        // On disk, reported or not (another editor, `git
                        // pull`).
                        std::fs::write(&path, &variant).unwrap();
                        disk[i] = variant;
                        if n % 2 == 0 {
                            client.notify(
                                DidChangeWatchedFiles::METHOD,
                                json!({"changes": [{"uri": Url::from_file_path(&path).unwrap(), "type": 2}]}),
                            );
                        } else {
                            unnotified += 1;
                        }
                    }
                }
                let texts: Vec<String> = (0..slots.len())
                    .map(|k| match &editor[k] {
                        Some((_, _, t)) => t.clone(),
                        None => disk[k].clone(),
                    })
                    .collect();
                let open: Vec<usize> = (0..slots.len()).filter(|k| editor[*k].is_some()).collect();
                if open.is_empty() {
                    continue;
                }
                let mut fresh: BTreeMap<String, Fresh> = BTreeMap::new();
                for _ in 0..1 + rng.next(2) {
                    let k = open[rng.next(open.len())];
                    let (uri, _, text) = editor[k].clone().unwrap();
                    let rel = layout.rel(&slots[k].rel);
                    let owner = layout.owner(&rel, added);
                    let (safe, fixed) = fresh
                        .entry(owner.clone())
                        .or_insert_with(|| fresh_in(layout, &slots, &texts, &owner));
                    let safe = safe.get(&rel).cloned().unwrap_or_default();
                    let offered = preferred_fixes(&mut client, &uri);
                    assert!(
                        offered.is_subset(&safe),
                        "{layout:?} seed {seed} ({init}) step {step}: {rel} offers {offered:?}, fresh {safe:?}\n{texts:#?}"
                    );
                    offered_total += offered.len();
                    if let Some(result) = fix_all_result(&mut client, &uri, &text) {
                        assert_eq!(
                            Some(&result),
                            fixed.get(&rel),
                            "{layout:?} seed {seed} ({init}) step {step}: {rel} fix all\n{texts:#?}"
                        );
                        fix_alls += 1;
                    }
                    asked += 1;
                }
            }
            client.shutdown();
        }
    }
    eprintln!(
        "{asked} code action requests, {offered_total} preferred fixes, {fix_alls} fix-all edits, {unnotified} changes without notification"
    );
    assert!(asked > 40, "{asked}");
    assert!(offered_total > 5, "{offered_total}");
    assert!(fix_alls > 5, "{fix_alls}");
    assert!(unnotified > 5, "{unnotified}");
}

/// What a file of [`differential_items`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    /// A library module (`code.py` next to its `resource.json`), which
    /// disk operations may turn into links.
    Module,
    /// Any other data file (a view).
    Data,
    /// A project manifest.
    Manifest,
    /// The configuration file.
    Config,
    /// A configuration file outside the workspace that the configuration
    /// may extend (with any extension).
    Shared,
}

/// A file of the differential test, its kind and the texts it may have.
struct Item {
    /// Relative to the top directory: `ws/...` in the workspace,
    /// `shared/...` outside it.
    rel: String,
    kind: ItemKind,
    /// Opened at the start.
    open: bool,
    variants: Vec<String>,
}

const BASE_LIBRARY: &str = "projects/base/ignition/script-python";

/// Extensions of the extended configuration files (review round 33, B1:
/// any name may be extended).
const EXTENSIONS: [&str; 7] = [".cfg", ".toml", ".txt", ".json", ".py", "", ".iglint"];

/// The files of [`applied_safe_fixes_equal_a_fresh_cli_run`]: the
/// generated libraries of [`slots`], a module of the parent project `base`
/// that `p` may inherit from, `p`'s `project.json`, the configuration, and
/// a chain of two configuration files outside the workspace (`base{ext0}`
/// and `more{ext1}`) that it may extend.
fn differential_items(ext: [&str; 2]) -> Vec<Item> {
    let mut items: Vec<Item> = slots()
        .into_iter()
        .map(|s| Item {
            kind: if s.rel.ends_with("code.py") {
                ItemKind::Module
            } else {
                ItemKind::Data
            },
            rel: format!("ws/{}", s.rel),
            open: s.open,
            variants: s.variants,
        })
        .collect();
    let owned = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    items.push(Item {
        rel: format!("ws/{BASE_LIBRARY}/bm/code.py"),
        kind: ItemKind::Module,
        open: false,
        variants: owned(&[
            "def z():\n    return lib.util.json.dumps(1)\n",
            "def z():\n    return 1\n",
            "import os\ndef z():\n    return m0.util.os.sep\n",
        ]),
    });
    items.push(Item {
        rel: "ws/projects/p/project.json".to_owned(),
        kind: ItemKind::Manifest,
        open: false,
        variants: owned(&[
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
            r#"{"title": "p", "enabled": true, "inheritable": false, "parent": "base"}"#,
        ]),
    });
    let base = format!("../shared/base{}", ext[0]);
    items.push(Item {
        rel: "ws/iglint.toml".to_owned(),
        kind: ItemKind::Config,
        open: false,
        variants: vec![
            String::new(),
            "unfixable = [\"VIEW015\"]\n".to_owned(),
            "select = [\"SCR102\", \"VIEW015\"]\n".to_owned(),
            "select = [\n".to_owned(),
            format!("extend = \"{base}\"\n"),
            format!("extend = \"{base}\"\nselect = [\"SCR102\", \"VIEW015\"]\n"),
        ],
    });
    let more = format!("more{}", ext[1]);
    items.push(Item {
        rel: format!("shared/base{}", ext[0]),
        kind: ItemKind::Shared,
        open: false,
        variants: vec![
            "select = [\"SCR102\", \"VIEW015\"]\n".to_owned(),
            "unfixable = [\"SCR102\"]\n".to_owned(),
            format!("extend = \"{more}\"\n"),
            format!("extend = \"{more}\"\nunfixable = [\"VIEW015\"]\n"),
            "select = [\n".to_owned(),
        ],
    });
    items.push(Item {
        rel: format!("shared/{more}"),
        kind: ItemKind::Shared,
        open: false,
        variants: owned(&[
            "select = [\"SCR102\"]\n",
            "unfixable = [\"SCR102\"]\n",
            "ignore = [\"VIEW015\"]\n",
        ]),
    });
    items
}

/// The files of the differential test below `top`, with the texts `texts`
/// (one per item); `top/outside` holds links and hard links elsewhere.
fn write_items(top: &Path, items: &[Item], texts: &[String]) {
    write(
        top,
        "ws/projects/base/project.json",
        r#"{"title": "base", "enabled": true, "inheritable": true}"#,
    );
    for (item, text) in items.iter().zip(texts) {
        if item.kind == ItemKind::Module || item.kind == ItemKind::Data {
            let dir = Path::new(&item.rel).parent().unwrap();
            write(
                top,
                &format!("{}/resource.json", dir.display()),
                &manifest(&item.rel),
            );
        }
        write(top, &item.rel, text);
    }
}

/// What `iglint check` and `iglint check --fix` (safe fixes, not
/// written) report when run in the folder `folder`: the findings after
/// the baseline and the fixed text of each file that changes (absolute
/// paths), or `None` when the run fails (a broken configuration or
/// baseline).
type CliRun = (Vec<iglint_linter::Diagnostic>, BTreeMap<PathBuf, String>);

fn cli_run(folder: &Path) -> Option<CliRun> {
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: folder.to_path_buf(),
            start: Some(folder.to_path_buf()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .ok()?;
    let paths = vec![folder.to_path_buf()];
    let origin = iglint_linter::baseline::BaselineOrigin::new(&config.settings, &paths);
    let baseline_path = config
        .baseline
        .clone()
        .unwrap_or_else(|| origin.default_file());
    let baseline = if baseline_path.is_file() {
        Some(iglint_linter::baseline::Baseline::load(&baseline_path).ok()?)
    } else if config.baseline.is_some() {
        return None;
    } else {
        None
    };
    let checked = iglint_linter::check(
        &config.settings,
        &paths,
        &iglint_linter::CheckOptions::default(),
    );
    let mut diagnostics = checked.diagnostics;
    if let Some(b) = &baseline {
        diagnostics = b.filter(&origin, diagnostics).new;
    }
    let fixed = iglint_linter::check(
        &config.settings,
        &paths,
        &iglint_linter::CheckOptions {
            fix_mode: iglint_linter::FixMode::DiffOnly {
                unsafe_fixes: false,
            },
            ..iglint_linter::CheckOptions::default()
        },
    );
    let fixed = fixed
        .fixed
        .into_iter()
        .filter(|f| f.changed())
        .map(|f| (iglint_linter::discovery::absolute(&f.path), f.fixed))
        .collect();
    Some((diagnostics, fixed))
}

/// By path relative to the top: the findings, and the texts `--fix` makes.
type CliOnCopy = (
    Vec<(String, iglint_linter::Diagnostic)>,
    BTreeMap<String, String>,
);

/// A copy of `top` (links and hard links kept) with the editor's texts
/// written over it, as saving every open document would, and the CLI run
/// in the copy of `folder` (paths relative to `top` in the result).
fn cli_on_copy(top: &Path, folder: &Path, editor: &[(PathBuf, String)]) -> Option<CliOnCopy> {
    let copy = tempfile::tempdir().unwrap();
    let to = iglint_linter::discovery::absolute(copy.path());
    let status = std::process::Command::new("cp")
        .arg("-a")
        .arg(format!("{}/.", top.display()))
        .arg(&to)
        .status()
        .unwrap();
    assert!(status.success());
    for (path, text) in editor {
        let at = to.join(path.strip_prefix(top).unwrap());
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        // Through a link whose target's directory is gone, saving fails:
        // no fix may be applied then.
        std::fs::write(&at, text).ok()?;
    }
    let (diagnostics, fixed) = cli_run(&to.join(folder.strip_prefix(top).unwrap()))?;
    let rel = |p: &Path| {
        p.strip_prefix(&to)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default()
    };
    Some((
        diagnostics
            .into_iter()
            .map(|d| (rel(&iglint_linter::discovery::absolute(d.file.path())), d))
            .collect(),
        fixed.into_iter().map(|(p, t)| (rel(&p), t)).collect(),
    ))
}

/// The file a path reaches (device and inode), through links.
#[cfg(unix)]
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// The folder the server makes for a file outside every workspace folder.
fn adhoc_root(path: &Path) -> PathBuf {
    path.ancestors()
        .skip(1)
        .find(|dir| dir.join("project.json").is_file() || dir.join("config-mode.json").is_file())
        .or_else(|| path.parent())
        .unwrap_or(path)
        .to_path_buf()
}

/// An open document of the differential test: its path, the item it was
/// opened as (or the item an alias was made of), and the editor's state.
struct Doc {
    path: PathBuf,
    item: usize,
    editor: Option<(Url, i32, String)>,
}

/// Review rounds 32 and 33: the fixes the server applies are those of
/// `iglint check --fix` on the disk after saving the document they are
/// for, and none are applied while another open document has unsaved
/// changes. Random sequences of editor edits (in several documents at
/// once), saves, closes, a workspace folder added and removed, and
/// changes on disk with and without notifications: a file replaced by a
/// symbolic link to another one, a link retargeted, a link replaced by a
/// file, hard links inside and outside the workspace, renamed modules,
/// manifest edits (inheriting from `base` or not), configuration and
/// baseline writes, a configuration extending a chain of files outside the
/// workspace with arbitrary extensions, and names outside the folder for
/// any file (symbolic links and hard links to scripts, views, manifests,
/// configuration files), opened and edited as documents of their own. At
/// random points the code actions of an open document are asked for and
/// every fix resolved. When another open document's editor text differs
/// from its file on disk, or the document is open under several names
/// with different texts, no fix edit is given. Otherwise each edit given
/// for a safe quick fix is the fix of a finding that `iglint check` of a
/// copy (links and hard links kept) with the document's text written to
/// it reports as safe, and each "fix all" edit gives what `--fix` gives
/// there; when the check fails nothing is given. Edits are sometimes
/// applied to the document. With and without `codeAction/resolve`, and
/// with `disabledSupport`.
#[cfg(unix)]
#[test]
fn applied_safe_fixes_equal_a_fresh_cli_run() {
    use std::os::unix::fs::symlink;
    // `IGLINT_CONSISTENCY_SEEDS=n` runs `n` sequences (default 4).
    let seeds: u64 = std::env::var("IGLINT_CONSISTENCY_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let (mut asked, mut safe_quick, mut fix_alls, mut withheld, mut links) = (0, 0, 0, 0, 0);
    let (mut aliased, mut outside_targets, mut disabled, mut extended) = (0, 0, 0, 0);
    for seed in 1..=seeds {
        let mut rng = Rng(seed * 7_919 + 13);
        let ext = [
            EXTENSIONS[rng.next(EXTENSIONS.len())],
            EXTENSIONS[rng.next(EXTENSIONS.len())],
        ];
        let items = differential_items(ext);
        let modules: Vec<usize> = (0..items.len())
            .filter(|i| items[*i].kind == ItemKind::Module)
            .collect();
        let disabled_support = rng.next(3) == 0;
        let capabilities = if disabled_support {
            let mut c = client_capabilities();
            c.text_document
                .as_mut()
                .unwrap()
                .code_action
                .as_mut()
                .unwrap()
                .disabled_support = Some(true);
            c
        } else if rng.next(2) == 0 {
            client_capabilities()
        } else {
            lsp_types::ClientCapabilities::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let top = iglint_linter::discovery::absolute(dir.path());
        let ws = top.join("ws");
        std::fs::create_dir_all(top.join("outside")).unwrap();
        let initial: Vec<String> = items
            .iter()
            .map(|i| {
                let n = match i.kind {
                    ItemKind::Config => i.variants.len(),
                    _ => i.variants.len().min(3),
                };
                i.variants[rng.next(n)].clone()
            })
            .collect();
        write_items(&top, &items, &initial);
        let nested = ws.join(format!("{LIBRARY}/lib"));
        let mut nested_open = false;
        let mut client = Client::start_folders(&[&ws], json!({"debounceMs": 0}), capabilities);
        let mut renamed = vec![false; items.len()];
        let path_of = |i: usize, renamed: &[bool]| -> PathBuf {
            let rel = &items[i].rel;
            if renamed[i] {
                let dir = Path::new(rel).parent().unwrap();
                top.join(format!("{}_r/code.py", dir.display()))
            } else {
                top.join(rel)
            }
        };
        // The documents: one per item (opened under its own name), then
        // the names made outside the workspace.
        let mut docs: Vec<Doc> = (0..items.len())
            .map(|i| Doc {
                path: path_of(i, &renamed),
                item: i,
                editor: None,
            })
            .collect();
        let mut outside = 0;
        for (k, item) in items.iter().enumerate() {
            if item.open {
                let uri = client.open(&docs[k].path, &initial[k]);
                docs[k].editor = Some((uri, 1, initial[k].clone()));
            }
        }
        let notify = |client: &Client, rng: &mut Rng, path: &Path, typ: u32| {
            if rng.next(2) == 0 {
                client.notify(
                    DidChangeWatchedFiles::METHOD,
                    json!({"changes": [{"uri": Url::from_file_path(path).unwrap(), "type": typ}]}),
                );
            }
        };
        for step in 0..30 {
            let i = rng.next(items.len());
            let path = path_of(i, &renamed);
            if docs[i].editor.is_none() {
                docs[i].path = path.clone();
            }
            let variant = items[i].variants[rng.next(items[i].variants.len())].clone();
            let module = items[i].kind == ItemKind::Module && !renamed[i];
            let other = modules[rng.next(modules.len())];
            // A document: one of the items, or a name made outside.
            let d = rng.next(docs.len());
            let doc_variant = {
                let v = &items[docs[d].item].variants;
                v[rng.next(v.len())].clone()
            };
            match rng.next(20) {
                // Open or close.
                0 | 1 => match docs[d].editor.take() {
                    Some((uri, _, _)) => client.notify(
                        lsp_types::notification::DidCloseTextDocument::METHOD,
                        json!({"textDocument": {"uri": uri}}),
                    ),
                    None => {
                        if d < items.len() {
                            docs[d].path = path_of(d, &renamed);
                        }
                        if let Ok(text) = std::fs::read_to_string(&docs[d].path) {
                            let uri = client.open(&docs[d].path, &text);
                            docs[d].editor = Some((uri, 1, text));
                        }
                    }
                },
                // Type in an open document, else write the disk.
                2..=4 => match docs[d].editor.as_mut() {
                    Some((uri, version, text)) => {
                        *version += 1;
                        *text = doc_variant;
                        let (uri, version, text) = (uri.clone(), *version, text.clone());
                        client.replace_all(&uri, version, &text);
                    }
                    None if d < items.len() => {
                        let _ = std::fs::remove_file(&path);
                        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                        std::fs::write(&path, &variant).unwrap();
                        notify(&client, &mut rng, &path, 2);
                    }
                    None => {}
                },
                // Save.
                5 => {
                    // Through a link whose target is gone, saving fails.
                    if let Some((uri, _, text)) = &docs[d].editor
                        && std::fs::write(&docs[d].path, text).is_ok()
                    {
                        client.notify(
                            lsp_types::notification::DidSaveTextDocument::METHOD,
                            json!({"textDocument": {"uri": uri}}),
                        );
                    }
                }
                // A module becomes a link to another one, or a link is
                // retargeted (atomically).
                6 | 7 if module && other != i && !renamed[other] => {
                    let target = format!(
                        "{}/code.py",
                        pathdiff(
                            path_of(other, &renamed).parent().unwrap(),
                            path.parent().unwrap()
                        )
                    );
                    let tmp = path.with_file_name("tmp-link");
                    let _ = std::fs::remove_file(&tmp);
                    symlink(&target, &tmp).unwrap();
                    std::fs::rename(&tmp, &path).unwrap();
                    links += 1;
                    notify(&client, &mut rng, &path, 2);
                }
                // A link (or a file) becomes a file again.
                8 if module => {
                    let _ = std::fs::remove_file(&path);
                    std::fs::write(&path, &variant).unwrap();
                    notify(&client, &mut rng, &path, 2);
                }
                // Hard links: inside the workspace, or elsewhere.
                9 if module && other != i && !renamed[other] => {
                    let target = path_of(other, &renamed);
                    if rng.next(2) == 0 {
                        if let Ok(real) = std::fs::canonicalize(&target)
                            && real != std::fs::canonicalize(&path).unwrap_or_default()
                        {
                            let _ = std::fs::remove_file(&path);
                            std::fs::hard_link(&real, &path).unwrap();
                        }
                    } else if let Ok(real) = std::fs::canonicalize(&path) {
                        outside += 1;
                        let _ = std::fs::hard_link(&real, top.join(format!("outside/h{outside}")));
                    }
                    notify(&client, &mut rng, &path, 2);
                }
                // A closed module is renamed, or renamed back.
                10 if items[i].kind == ItemKind::Module && docs[i].editor.is_none() => {
                    let from = path.parent().unwrap().to_path_buf();
                    renamed[i] = !renamed[i];
                    let to = path_of(i, &renamed).parent().unwrap().to_path_buf();
                    if std::fs::rename(&from, &to).is_err() {
                        renamed[i] = !renamed[i];
                    }
                    docs[i].path = path_of(i, &renamed);
                    notify(&client, &mut rng, &to.join("code.py"), 1);
                }
                // The baseline: written from the disk now, or removed.
                11 => {
                    let file = ws.join(iglint_linter::baseline::DEFAULT_BASELINE_FILE);
                    if rng.next(2) == 0 {
                        let _ = std::fs::remove_file(&file);
                    } else if let Ok(config) = iglint_config::load(
                        &iglint_config::LoadOptions {
                            cwd: ws.clone(),
                            start: Some(ws.clone()),
                            ..iglint_config::LoadOptions::default()
                        },
                        &iglint_linter::rules::REGISTRY,
                    ) {
                        let paths = vec![ws.clone()];
                        let result = iglint_linter::check(
                            &config.settings,
                            &paths,
                            &iglint_linter::CheckOptions::default(),
                        );
                        let origin =
                            iglint_linter::baseline::BaselineOrigin::new(&config.settings, &paths);
                        // Some of the findings.
                        let some: Vec<iglint_linter::Diagnostic> = result
                            .diagnostics
                            .into_iter()
                            .filter(|_| rng.next(2) == 0)
                            .collect();
                        iglint_linter::baseline::Baseline::from_diagnostics(&origin, &some)
                            .save(&file)
                            .unwrap();
                    }
                    notify(&client, &mut rng, &file, 2);
                }
                // A workspace folder on the library, added or removed.
                12 => {
                    let uri = Url::from_file_path(&nested).unwrap();
                    let folder = json!([{"uri": uri, "name": "lib"}]);
                    let event = if nested_open {
                        json!({"added": [], "removed": folder})
                    } else {
                        json!({"added": folder, "removed": []})
                    };
                    nested_open = !nested_open;
                    client.notify(
                        lsp_types::notification::DidChangeWorkspaceFolders::METHOD,
                        json!({ "event": event }),
                    );
                }
                // Another name for a file, outside the workspace: a
                // symbolic link or a hard link, with the file's own name
                // or another extension, opened or not.
                13 | 14 => {
                    outside += 1;
                    let original = path_of(i, &renamed);
                    let name = if rng.next(2) == 0 {
                        original.file_name().unwrap().to_string_lossy().into_owned()
                    } else {
                        format!("a{}", EXTENSIONS[rng.next(EXTENSIONS.len())])
                    };
                    let at = top.join(format!("outside/n{outside}/{name}"));
                    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
                    let made = if rng.next(2) == 0 {
                        symlink(
                            format!(
                                "{}/{}",
                                pathdiff(original.parent().unwrap(), at.parent().unwrap()),
                                original.file_name().unwrap().to_string_lossy()
                            ),
                            &at,
                        )
                        .is_ok()
                    } else {
                        std::fs::canonicalize(&original)
                            .and_then(|real| std::fs::hard_link(real, &at))
                            .is_ok()
                    };
                    if made {
                        links += 1;
                        let editor = if rng.next(3) > 0
                            && let Ok(text) = std::fs::read_to_string(&at)
                        {
                            Some((client.open(&at, &text), 1, text))
                        } else {
                            None
                        };
                        docs.push(Doc {
                            path: at,
                            item: i,
                            editor,
                        });
                    }
                }
                // Save every open document but one.
                15 | 16 => {
                    let keep = rng.next(docs.len() + 1);
                    for (k, doc) in docs.iter().enumerate() {
                        if k == keep {
                            continue;
                        }
                        if let Some((uri, _, text)) = &doc.editor
                            && std::fs::read(&doc.path).ok().as_deref() != Some(text.as_bytes())
                            && std::fs::write(&doc.path, text).is_ok()
                        {
                            client.notify(
                                lsp_types::notification::DidSaveTextDocument::METHOD,
                                json!({"textDocument": {"uri": uri}}),
                            );
                        }
                    }
                }
                _ => {}
            }
            if rng.next(2) == 0 {
                continue;
            }
            let open: Vec<usize> = (0..docs.len())
                .filter(|k| {
                    docs[*k].editor.is_some()
                        && matches!(
                            items[docs[*k].item].kind,
                            ItemKind::Module | ItemKind::Data | ItemKind::Manifest
                        )
                })
                .collect();
            if open.is_empty() {
                continue;
            }
            let k = open[rng.next(open.len())];
            if rng.next(2) == 0 {
                // Every other document saved first.
                for (j, doc) in docs.iter().enumerate() {
                    if let Some((u, _, t)) = &doc.editor
                        && j != k
                        && std::fs::read(&doc.path).ok().as_deref() != Some(t.as_bytes())
                        && std::fs::write(&doc.path, t).is_ok()
                    {
                        client.notify(
                            lsp_types::notification::DidSaveTextDocument::METHOD,
                            json!({"textDocument": {"uri": u}}),
                        );
                    }
                }
            }
            let (uri, version, text) = docs[k].editor.clone().unwrap();
            let doc_path = Url::to_file_path(&uri).unwrap();
            let folder = if !doc_path.starts_with(&ws) {
                outside_targets += 1;
                adhoc_root(&doc_path)
            } else if nested_open && doc_path.starts_with(&nested) {
                nested.clone()
            } else {
                ws.clone()
            };
            if std::fs::read_to_string(ws.join("iglint.toml")).is_ok_and(|t| t.contains("extend")) {
                extended += 1;
            }
            // What may be applied: the document's text (the same under
            // every name it is open as), every other open document
            // unchanged.
            let target = identity(&doc_path);
            let mut texts: Vec<&str> = Vec::new();
            let mut modified: Option<PathBuf> = None;
            for doc in &docs {
                let Some((u, _, t)) = &doc.editor else {
                    continue;
                };
                let p = Url::to_file_path(u).unwrap();
                if p == doc_path || target.is_some() && identity(&p) == target {
                    texts.push(t);
                } else if std::fs::read(&p).ok().as_deref() != Some(t.as_bytes()) {
                    modified = Some(p);
                }
            }
            if texts.len() > 1 {
                aliased += 1;
            }
            let conflict = texts.iter().any(|t| *t != texts[0]);
            let withhold = conflict || modified.is_some();
            let reference = if withhold {
                None
            } else {
                cli_on_copy(&top, &folder, &[(doc_path.clone(), text.clone())])
            };
            let rel = doc_path
                .strip_prefix(&top)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let actions = client
                .request::<CodeActionRequest>(CodeActionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    range: Range::new(Position::new(0, 0), Position::new(10_000, 0)),
                    context: CodeActionContext {
                        diagnostics: Vec::new(),
                        only: None,
                        trigger_kind: None,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                })
                .unwrap_or_default();
            asked += 1;
            let context = format!("seed {seed} step {step} {rel}");
            let mut apply_to_editor: Option<String> = None;
            for action in actions {
                let CodeActionOrCommand::CodeAction(action) = action else {
                    continue;
                };
                if action.title.contains("suppress") {
                    continue;
                }
                if action.disabled.is_some() {
                    assert!(
                        disabled_support,
                        "{context}: disabled action without support"
                    );
                    assert!(
                        action.edit.is_none() && action.data.is_none(),
                        "{context}: a disabled action with an edit"
                    );
                    disabled += 1;
                    continue;
                }
                let quick = action.kind == Some(CodeActionKind::QUICKFIX);
                let safe = action.is_preferred == Some(true);
                let target = action
                    .diagnostics
                    .as_ref()
                    .and_then(|d| d.first())
                    .map(finding);
                let action = if action.edit.is_none() && action.data.is_some() {
                    client.request::<lsp_types::request::CodeActionResolveRequest>(action)
                } else {
                    action
                };
                let Some(edits) = action
                    .edit
                    .as_ref()
                    .and_then(|e| e.changes.as_ref())
                    .and_then(|c| c.get(&uri))
                else {
                    if quick && safe {
                        withheld += 1;
                    }
                    continue;
                };
                if withhold {
                    panic!(
                        "{context}: {} gives an edit while {}",
                        action.title,
                        match &modified {
                            Some(p) => format!("{} has unsaved changes", p.display()),
                            None => "the document is open with different texts".to_owned(),
                        }
                    );
                }
                let applied = apply(&text, edits);
                if quick && !safe {
                    continue;
                }
                let Some((diagnostics, fixed)) = &reference else {
                    panic!("{context}: {} applied while the CLI fails", action.title);
                };
                if quick {
                    let target = target.expect("a quick fix names its finding");
                    let found = diagnostics.iter().find(|(r, d)| {
                        *r == rel
                            && d.rule.code() == target.0
                            && position(d.file.text(), d.range.start().to_usize()) == target.1
                            && position(d.file.text(), d.range.end().to_usize()) == target.2
                            && d.message == target.3
                            && d.fix.as_ref().is_some_and(|f| {
                                f.applicability == iglint_linter::Applicability::Safe
                                    && action.title.contains(&f.message)
                            })
                    });
                    let Some((_, d)) = found else {
                        panic!(
                            "{context}: safe {} applied, not a safe fix of the CLI",
                            action.title
                        );
                    };
                    let own: Vec<iglint_text::TextEdit> = d
                        .fix
                        .as_ref()
                        .unwrap()
                        .edits
                        .iter()
                        .map(|e| e.edit.clone())
                        .collect();
                    let expected = iglint_text::apply_edits(&text, &own).unwrap();
                    assert_eq!(applied, expected, "{context}: {}", action.title);
                    safe_quick += 1;
                } else {
                    assert_eq!(
                        Some(&applied),
                        fixed.get(&rel).or(Some(&text)),
                        "{context}: fix all"
                    );
                    fix_alls += 1;
                }
                if apply_to_editor.is_none() && applied != text && rng.next(3) == 0 {
                    apply_to_editor = Some(applied);
                }
            }
            if withhold {
                withheld += 1;
            }
            if let Some(new) = apply_to_editor {
                client.replace_all(&uri, version + 1, &new);
                docs[k].editor = Some((uri, version + 1, new));
            }
        }
        client.shutdown();
    }
    eprintln!(
        "{asked} code action requests, {safe_quick} safe quick fixes and {fix_alls} fix-all edits equal to the CLI, {withheld} withheld, {disabled} disabled actions, {links} links made, {aliased} requests with the document open under several names, {outside_targets} for a name outside the folder, {extended} with an extended configuration"
    );
    assert!(asked > 10 * seeds as usize / 2, "{asked}");
    assert!(safe_quick > 0, "{safe_quick}");
    assert!(fix_alls > 0, "{fix_alls}");
}

/// The relative path from `from` (a directory) to `to`, both absolute.
fn pathdiff(to: &Path, from: &Path) -> String {
    let to: Vec<_> = to.components().collect();
    let from: Vec<_> = from.components().collect();
    let common = to.iter().zip(&from).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// Review round 32: on a private corpus (aggregates only), the live
/// workspace's findings of every file equal those of a fresh check, so the
/// server does not discover a folder again after a fresh fix computation
/// on an unchanged disk.
#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn private_corpus_live_matches_a_fresh_check() {
    let Some(corpus) = std::env::var_os("IGLINT_PRIVATE_CORPUS") else {
        eprintln!("IGLINT_PRIVATE_CORPUS is not set");
        return;
    };
    let root = iglint_linter::discovery::absolute(Path::new(&corpus));
    let config = iglint_config::load(
        &iglint_config::LoadOptions {
            cwd: root.clone(),
            start: Some(root.clone()),
            ..iglint_config::LoadOptions::default()
        },
        &iglint_linter::rules::REGISTRY,
    )
    .unwrap();
    let paths = vec![root.clone()];
    let result = iglint_linter::check(
        &config.settings,
        &paths,
        &iglint_linter::CheckOptions::default(),
    );
    let mut fresh: BTreeMap<PathBuf, Vec<iglint_linter::Diagnostic>> = BTreeMap::new();
    for d in result.diagnostics {
        fresh
            .entry(iglint_linter::discovery::absolute(d.file.path()))
            .or_default()
            .push(d);
    }
    let mut live = iglint_linter::live::LiveWorkspace::new(config.settings.clone(), paths);
    let files = live.lintable_files();
    let mut differ = 0;
    let mut rules: BTreeMap<&'static str, usize> = BTreeMap::new();
    for found in live.check(&files) {
        let empty = Vec::new();
        let expected = fresh.get(&found.path).unwrap_or(&empty);
        let missing: Vec<&iglint_linter::Diagnostic> = expected
            .iter()
            .filter(|d| {
                !found
                    .diagnostics
                    .iter()
                    .any(|f| iglint_linter::live::same_finding(d, f))
            })
            .collect();
        let extra: Vec<&iglint_linter::Diagnostic> = found
            .diagnostics
            .iter()
            .filter(|d| {
                !expected
                    .iter()
                    .any(|f| iglint_linter::live::same_finding(d, f))
            })
            .collect();
        if !missing.is_empty() || !extra.is_empty() {
            differ += 1;
            for d in missing.iter().chain(&extra) {
                *rules.entry(d.rule.code()).or_default() += 1;
            }
        }
    }
    eprintln!(
        "{} files, {differ} differ; findings that differ by rule: {rules:?}",
        files.len()
    );
    assert_eq!(differ, 0);
}
