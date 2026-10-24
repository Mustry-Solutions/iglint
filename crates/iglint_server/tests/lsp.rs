//! The language server, driven over an in-memory connection.

mod common;

use common::*;
use lsp_server::Message;
use lsp_types::notification::{DidChangeWatchedFiles, DidCloseTextDocument, Notification as _};
use lsp_types::request::{
    ApplyWorkspaceEdit, CodeActionRequest, CodeActionResolveRequest, ExecuteCommand, HoverRequest,
    RegisterCapability, Request as _,
};
use lsp_types::{
    CodeAction, CodeActionContext, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    DiagnosticSeverity, DiagnosticTag, ExecuteCommandParams, FileChangeType, FileEvent,
    HoverContents, HoverParams, Position, Range, TextDocumentContentChangeEvent,
    TextDocumentIdentifier, TextDocumentPositionParams, Url,
};
use serde_json::json;

fn root(dir: &tempfile::TempDir) -> std::path::PathBuf {
    iglint_linter::discovery::absolute(dir.path())
}

fn code_actions(
    client: &mut Client,
    uri: &Url,
    range: Range,
    only: Option<Vec<CodeActionKind>>,
) -> Vec<CodeAction> {
    let actions = client
        .request::<CodeActionRequest>(CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range,
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
        .unwrap_or_default();
    actions
        .into_iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(a) => Some(a),
            CodeActionOrCommand::Command(_) => None,
        })
        .collect()
}

fn full_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(10_000, 0))
}

#[test]
fn publishes_diagnostics_with_lsp_details() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));

    // An expression with a non-BMP character in a string literal, then an
    // invalid one: the range must count UTF-16 code units.
    let text = popup_view("speed").replace(
        r#"{"props.text": {"binding": {"type": "property", "config": {"path": "view.params.speed"}}}}"#,
        "{\"props.text\": {\"binding\": {\"type\": \"property\", \"config\": {\"path\": \"view.params.speed\"}}}, \"custom.e\": {\"binding\": {\"type\": \"expr\", \"config\": {\"expression\": \"'\u{1F600}' + \u{1F600}\"}}}}",
    );
    std::fs::write(&popup, &text).unwrap();
    let uri = client.open(&popup, &text);
    let published = client.diagnostics(&uri, Some(1));
    let d = published
        .diagnostics
        .iter()
        .find(|d| d.code == Some(lsp_types::NumberOrString::String("BIND001".into())))
        .unwrap_or_else(|| panic!("{published:#?}"));
    assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(d.source.as_deref(), Some("iglint"));
    assert!(
        d.code_description
            .as_ref()
            .unwrap()
            .href
            .as_str()
            .ends_with("BIND001.md")
    );
    // The second emoji, after the first one (two UTF-16 units).
    let line_no = text.lines().position(|l| l.contains("' + ")).unwrap();
    let line = text.lines().nth(line_no).unwrap();
    let at = line.rfind('\u{1F600}').unwrap();
    let utf16 = |s: &str| s.encode_utf16().count() as u32;
    assert_eq!(
        d.range.start,
        Position::new(line_no as u32, utf16(line.get(..at).unwrap()))
    );
    assert_eq!(
        d.range.end,
        Position::new(line_no as u32, utf16(line.get(..at).unwrap()) + 2)
    );
    client.shutdown();
}

#[test]
fn incremental_changes_and_dependent_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, caller) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let popup_text = popup_view("speed");
    let popup_uri = client.open(&popup, &popup_text);
    let caller_uri = client.open(&caller, CALLER);
    assert!(!codes(&client.diagnostics(&caller_uri, Some(1))).contains(&"REF007".to_owned()));
    client.diagnostics(&popup_uri, Some(1));

    // Rename the parameter with an incremental edit of every occurrence.
    let changes: Vec<TextDocumentContentChangeEvent> = popup_text
        .lines()
        .enumerate()
        .flat_map(|(n, line)| {
            line.match_indices("speed")
                .map(move |(c, _)| (n as u32, c as u32))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|(line, col)| TextDocumentContentChangeEvent {
            range: Some(Range::new(
                Position::new(line, col),
                Position::new(line, col + 5),
            )),
            range_length: None,
            text: "rate".to_owned(),
        })
        .collect();
    assert_eq!(changes.len(), 3);
    client.change(&popup_uri, 2, changes);
    let popup_after = client.diagnostics(&popup_uri, Some(2));
    assert!(
        !codes(&popup_after).contains(&"BIND005".to_owned()),
        "{popup_after:#?}"
    );
    // The caller passes `speed`, which the popup no longer declares.
    let caller_after = client.diagnostics(&caller_uri, Some(1));
    assert!(
        codes(&caller_after).contains(&"REF007".to_owned()),
        "{caller_after:#?}"
    );

    // Closing the popup goes back to the disk text, which declares `speed`.
    client.notify(
        DidCloseTextDocument::METHOD,
        json!({"textDocument": {"uri": popup_uri}}),
    );
    let caller_reverted = client.diagnostics(&caller_uri, Some(1));
    assert!(!codes(&caller_reverted).contains(&"REF007".to_owned()));
    client.shutdown();
}

#[test]
fn quick_fixes_fix_all_and_hover() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let text = popup_view("speed").replace(
        "\"params\": {\"speed\": 1},",
        "\"params\": {\"speed\": 1},\n  \"custom\": {\"unusedA\": 1, \"unusedB\": 2},",
    );
    let uri = client.open(&popup, &text);
    let published = client.diagnostics(&uri, Some(1));
    let unused: Vec<&lsp_types::Diagnostic> = published
        .diagnostics
        .iter()
        .filter(|d| d.message.contains("unusedA") || d.message.contains("unusedB"))
        .collect();
    assert_eq!(unused.len(), 2, "{published:#?}");
    assert_eq!(unused[0].tags, Some(vec![DiagnosticTag::UNNECESSARY]));

    // Quick fix for the first one.
    let actions = code_actions(&mut client, &uri, unused[0].range, None);
    let quick = actions
        .iter()
        .find(|a| a.kind == Some(CodeActionKind::QUICKFIX))
        .unwrap_or_else(|| panic!("{actions:#?}"));
    assert!(quick.title.starts_with("VIEW015"));
    assert_eq!(quick.is_preferred, Some(true));
    let edits = &quick.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri];
    let fixed = apply(&text, edits);
    assert!(
        !fixed.contains("unusedA") && fixed.contains("unusedB"),
        "{fixed}"
    );
    serde_json::from_str::<serde_json::Value>(&fixed).unwrap();

    // Fix all, as a source action resolved lazily.
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
    );
    assert_eq!(actions.len(), 1, "{actions:#?}");
    assert_eq!(
        actions[0].kind.as_ref().unwrap().as_str(),
        "source.fixAll.iglint"
    );
    assert!(actions[0].edit.is_none());
    let resolved = client.request::<CodeActionResolveRequest>(actions[0].clone());
    let edits = &resolved.edit.unwrap().changes.unwrap()[&uri];
    assert_eq!(edits.len(), 1);
    let fixed = apply(&text, edits);
    assert!(
        !fixed.contains("unusedA") && !fixed.contains("unusedB"),
        "{fixed}"
    );

    // Hover on the finding shows the rule.
    let hover = client
        .request::<HoverRequest>(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                position: unused[1].range.start,
            },
            work_done_progress_params: Default::default(),
        })
        .expect("a hover");
    let HoverContents::Markup(markup) = hover.contents else {
        panic!("markdown expected");
    };
    assert!(
        markup
            .value
            .contains("**VIEW015** `unused-custom-property`")
    );
    assert!(markup.value.contains("## What it does"));

    // Nothing to hover outside findings.
    let none = client.request::<HoverRequest>(HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position::new(0, 0),
        },
        work_done_progress_params: Default::default(),
    });
    assert!(none.is_none());

    // The fix-all command asks the client to apply the edit.
    let applied = client.request::<ExecuteCommand>(ExecuteCommandParams {
        command: iglint_server::COMMAND_FIX_ALL.to_owned(),
        arguments: vec![json!(uri)],
        work_done_progress_params: Default::default(),
    });
    assert_eq!(applied, Some(json!(true)));
    let edit = client.wait_for(|m| match m {
        Message::Request(r) if r.method == ApplyWorkspaceEdit::METHOD => Some(r.params.clone()),
        _ => None,
    });
    assert!(
        edit["edit"]["changes"].get(uri.as_str()).is_some(),
        "{edit}"
    );

    // Rule docs.
    let docs = client.request::<ExecuteCommand>(ExecuteCommandParams {
        command: iglint_server::COMMAND_RULE_DOCS.to_owned(),
        arguments: vec![json!("unused-custom-property")],
        work_done_progress_params: Default::default(),
    });
    assert!(
        docs.unwrap()
            .as_str()
            .unwrap()
            .starts_with("# VIEW015 unused-custom-property")
    );
    client.shutdown();
}

/// Quick fixes go through the same fix function as fix-all and `--fix`: a
/// safe fix that the safe-fix net rejects is not offered, an unsafe fix
/// is offered as unsafe, and fix-all applies neither (unsafe fixes off).
#[test]
fn quick_fixes_go_through_the_safety_net() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let marker = iglint_linter::safety::TEST_REJECT_MARKER;
    let text = format!(
        "# {marker}\nx = 1  # iglint: ignore[SCR101]\ny = 2  # iglint: ignore[SCR101] legacy\n"
    );
    let path = write(&root, &format!("{lib}/code.py"), &text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, &text);
    let published = client.diagnostics(&uri, Some(1));
    let line = |n: u32| {
        published
            .diagnostics
            .iter()
            .find(|d| {
                d.range.start.line == n
                    && d.code.as_ref().is_some_and(|c| {
                        *c == lsp_types::NumberOrString::String("SCR123".to_owned())
                    })
            })
            .unwrap_or_else(|| panic!("{published:#?}"))
            .range
    };
    let fixes = |client: &mut Client, range| -> Vec<CodeAction> {
        code_actions(client, &uri, range, Some(vec![CodeActionKind::QUICKFIX]))
            .into_iter()
            .filter(|a| a.title.starts_with("SCR123") && !a.title.contains("suppress on this line"))
            .collect()
    };
    // The safe fix the net drops is offered as unsafe, with the net's
    // reason, and not preferred (review round 18, N1).
    let rejected = fixes(&mut client, line(1));
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert!(
        rejected[0].title.contains("(unsafe: the safe fix could change what the code does (the test marker rejects safe fixes))"),
        "{rejected:#?}"
    );
    assert_ne!(rejected[0].is_preferred, Some(true), "{rejected:#?}");
    let unsafe_fix = fixes(&mut client, line(2));
    assert_eq!(unsafe_fix.len(), 1, "{unsafe_fix:#?}");
    // Every unsafe fix says why (review round 14).
    assert!(
        unsafe_fix[0]
            .title
            .ends_with("(unsafe: the comment holds more than the suppression (other text, a `# pylint:` directive or a coding declaration), and the fix removes all of it)"),
        "{}",
        unsafe_fix[0].title
    );
    assert_eq!(unsafe_fix[0].is_preferred, Some(false));
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
    );
    for action in actions {
        let resolved = client.request::<CodeActionResolveRequest>(action);
        if let Some(edit) = resolved.edit {
            let fixed = apply(&text, &edit.changes.unwrap()[&uri]);
            assert!(fixed.contains("x = 1  # iglint: ignore[SCR101]"), "{fixed}");
        }
    }
    client.shutdown();
}

#[test]
fn fix_all_without_resolve_support_is_eager() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    let mut client = Client::start_with(
        Some(&root),
        json!({"debounceMs": 0}),
        lsp_types::ClientCapabilities::default(),
    );
    let text =
        popup_view("speed").replace("\"params\": {", "\"custom\": {\"x\": 1},\n  \"params\": {");
    let uri = client.open(&popup, &text);
    client.diagnostics(&uri, Some(1));
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::new("source.fixAll.iglint")]),
    );
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edit.is_some());
    // Quick fixes are not source actions.
    assert!(
        actions
            .iter()
            .all(|a| a.kind != Some(CodeActionKind::QUICKFIX))
    );
    client.shutdown();
}

#[test]
fn configuration_reload_and_watched_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, caller) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    // The server registers file watchers.
    client.wait_for(|m| match m {
        Message::Request(r) if r.method == RegisterCapability::METHOD => Some(()),
        _ => None,
    });
    let text = CALLER.replace("\"Popup\"", "\"Missing\"");
    let uri = client.open(&caller, &text);
    assert!(codes(&client.diagnostics(&uri, Some(1))).contains(&"REF001".to_owned()));

    // Ignoring the rule in iglint.toml takes effect after the change event.
    let config = write(&root, "iglint.toml", "extend-ignore = [\"REF001\"]\n");
    client.notify(
        DidChangeWatchedFiles::METHOD,
        json!({"changes": [FileEvent { uri: Url::from_file_path(&config).unwrap(), typ: FileChangeType::CREATED }]}),
    );
    let after = client.diagnostics(&uri, Some(1));
    assert!(!codes(&after).contains(&"REF001".to_owned()), "{after:#?}");

    // A view changed on disk (not open) updates the open views that use it.
    std::fs::remove_file(&config).unwrap();
    client.notify(
        DidChangeWatchedFiles::METHOD,
        json!({"changes": [FileEvent { uri: Url::from_file_path(&config).unwrap(), typ: FileChangeType::DELETED }]}),
    );
    client.diagnostics(&uri, Some(1));
    client.replace_all(&uri, 2, CALLER);
    assert!(!codes(&client.diagnostics(&uri, Some(2))).contains(&"REF007".to_owned()));
    std::fs::write(&popup, popup_view("rate")).unwrap();
    client.notify(
        DidChangeWatchedFiles::METHOD,
        json!({"changes": [FileEvent { uri: Url::from_file_path(&popup).unwrap(), typ: FileChangeType::CHANGED }]}),
    );
    let after = client.diagnostics(&uri, Some(2));
    assert!(codes(&after).contains(&"REF007".to_owned()), "{after:#?}");
    client.shutdown();
}

#[test]
fn invalid_configuration_falls_back_to_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    write(&root, "iglint.toml", "[lint\nselect = 1");
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let shown = client.wait_for(|m| match m {
        Message::Notification(n) if n.method == "window/showMessage" => Some(n.params.clone()),
        _ => None,
    });
    assert!(
        shown["message"].as_str().unwrap().contains("configuration"),
        "{shown}"
    );
    let uri = client.open(&popup, "{\"root\": ");
    assert!(codes(&client.diagnostics(&uri, Some(1))).contains(&"VIEW001".to_owned()));
    client.shutdown();
}

#[test]
fn workspace_scope_publishes_unopened_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (_, caller) = demo_project(&root);
    std::fs::write(&caller, CALLER.replace("\"Popup\"", "\"Missing\"")).unwrap();
    let mut client = Client::start(Some(&root), json!({"scope": "workspace"}));
    let uri = Url::from_file_path(&caller).unwrap();
    let published = client.diagnostics(&uri, None);
    assert!(codes(&published).contains(&"REF001".to_owned()));
    assert_eq!(published.version, None);

    // Back to open files only: unopened files are cleared.
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"iglint": {"scope": "open"}}}),
    );
    let cleared = client.diagnostics(&uri, None);
    assert!(cleared.diagnostics.is_empty());
    client.shutdown();
}

#[test]
fn files_outside_folders_and_standalone_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root.join("elsewhere"));
    let loose = write(&root, "loose/view.json", "[1 2]");
    // No workspace folder at all.
    let mut client = Client::start(None, json!({"debounceMs": 0}));
    let uri = client.open(
        &popup,
        &popup_view("speed").replace("view.params.speed", "view.params.nope"),
    );
    assert!(codes(&client.diagnostics(&uri, Some(1))).contains(&"BIND005".to_owned()));
    let loose_uri = client.open(&loose, "[1 2]");
    assert_eq!(
        codes(&client.diagnostics(&loose_uri, Some(1))),
        vec!["VIEW001"]
    );
    // Not an iglint file: ignored.
    let css = write(&root, "loose/style.css", "a {}");
    client.open(&css, "a {}");
    client.shutdown();
}

/// Malformed input of every kind must never take the server down.
#[test]
fn survives_malformed_input() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    let script = write(
        &root,
        "projects/demo/ignition/script-python/lib/code.py",
        "def f(:\n",
    );
    write(
        &root,
        "projects/demo/ignition/script-python/lib/resource.json",
        r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
    );
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&popup, "");
    client.diagnostics(&uri, Some(1));
    let base = popup_view("speed") + "\u{0}\u{FEFF}\u{1F600}\"\\u12\"\\\"\"\"\"\"\"  \t \u{2028}";
    let mut version = 1;
    // Every prefix of a view (in steps) and some garbage.
    let mut texts: Vec<String> = (0..base.len())
        .step_by(7)
        .filter_map(|i| base.get(..i).map(str::to_owned))
        .collect();
    texts.extend(
        [
            "{\"root\": {\"type\": 1, \"children\": {}}}",
            "{\"root\": {\"children\": [null, 1, \"x\", {\"type\": \"ia.container.flex\", \"meta\": null}]}}",
            "{\"custom\": [], \"params\": \"x\", \"propConfig\": {\"custom.a\": {\"binding\": {\"type\": \"expr\", \"config\": {\"expression\": \"{\"}}}}}",
            "{\"root\": {\"events\": {\"dom\": {\"onClick\": {\"type\": \"script\", \"config\": {\"script\": \"\\tdef (:\\n\\t\\\"\\\"\\\"\"}}}}}}",
        ]
        .map(str::to_owned),
    );
    for text in &texts {
        version += 1;
        client.replace_all(&uri, version, text);
    }
    client.diagnostics(&uri, Some(version));
    // Incremental edits with positions past the end are clamped.
    version += 1;
    client.change(
        &uri,
        version,
        vec![TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(999, 999), Position::new(5, 0))),
            range_length: None,
            text: "\u{1F600}".to_owned(),
        }],
    );
    client.diagnostics(&uri, Some(version));
    let py = client.open(&script, "def f(:\n\tpass\n\u{0}");
    client.diagnostics(&py, Some(1));
    // Requests on unknown documents and bad parameters are answered.
    let actions = code_actions(
        &mut client,
        &Url::parse("file:///nowhere/view.json").unwrap(),
        full_range(),
        None,
    );
    assert!(actions.is_empty());
    let docs = client.request::<ExecuteCommand>(ExecuteCommandParams {
        command: iglint_server::COMMAND_RULE_DOCS.to_owned(),
        arguments: vec![json!(42)],
        work_done_progress_params: Default::default(),
    });
    assert_eq!(docs, None);
    client.shutdown();
}

#[test]
fn on_save_mode_waits_for_save() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (_, caller) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"run": "onSave", "debounceMs": 0}));
    let uri = client.open(&caller, CALLER);
    client.diagnostics(&uri, Some(1));
    client.replace_all(&uri, 2, &CALLER.replace("\"Popup\"", "\"Missing\""));
    client.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri}}),
    );
    let after = client.diagnostics(&uri, Some(2));
    assert!(codes(&after).contains(&"REF001".to_owned()));
    // The only diagnostics for version 2 came after the save.
    assert!(client.inbox.iter().all(|m| match m {
        Message::Notification(n) =>
            n.params.get("version") != Some(&json!(2))
                || n.method != "textDocument/publishDiagnostics",
        _ => true,
    }));
    client.shutdown();
}

#[test]
fn script_findings_can_be_suppressed() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (popup, _) = demo_project(&root);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let text = popup_view("speed").replace(
        r#""meta": {"name": "Label"},"#,
        r#""meta": {"name": "Label"}, "events": {"dom": {"onClick": {"type": "script", "scope": "G", "config": {"script": "\timport os\n\tsystem.perspective.print(1)"}}}},"#,
    );
    let uri = client.open(&popup, &text);
    let published = client.diagnostics(&uri, Some(1));
    let d = published
        .diagnostics
        .iter()
        .find(|d| d.code == Some(lsp_types::NumberOrString::String("SCR102".into())))
        .unwrap_or_else(|| panic!("{published:#?}"))
        .clone();
    assert_eq!(d.tags, Some(vec![DiagnosticTag::UNNECESSARY]));
    let actions = code_actions(
        &mut client,
        &uri,
        d.range,
        Some(vec![CodeActionKind::QUICKFIX]),
    );
    let suppress = actions
        .iter()
        .find(|a| a.title == "SCR102: suppress on this line")
        .unwrap_or_else(|| panic!("{actions:#?}"));
    let fixed = apply(
        &text,
        &suppress.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri],
    );
    assert!(
        fixed.contains(r#"\timport os  # iglint: ignore[SCR102]\n"#),
        "{fixed}"
    );
    client.replace_all(&uri, 2, &fixed);
    let after = client.diagnostics(&uri, Some(2));
    assert!(!codes(&after).contains(&"SCR102".to_owned()), "{after:#?}");
    client.shutdown();
}

/// Reviewer round 8 (Codex) blocker, through the language server: the
/// quick fix that removes `import math` from a module whose function,
/// defined before `import re`, reaches `eval` through `re.sys` is offered
/// only as unsafe (not preferred), and fix-all keeps the import.
#[test]
fn quick_fix_for_import_read_through_an_earlier_function_is_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "def capture():\n    return re.sys.modules['__builtin__']\nimport re\ndef target():\n    import math\n    print capture().eval('math.pi')\ntarget()\n";
    let path = write(&root, &format!("{lib}/code.py"), text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, text);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.range.start.line == 4
                && d.code
                    .as_ref()
                    .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes: Vec<CodeAction> = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
    .collect();
    assert!(!fixes.is_empty(), "{fixes:#?}");
    for fix in &fixes {
        // The title says why the fix is unsafe and where (reviewer round 8,
        // finding 2).
        assert!(
            fix.title.ends_with(
                "(unsafe: the script uses code that is not vetted: `re.sys` at \
                 projects/demo/ignition/script-python/m/code.py:2:12)"
            ),
            "{fix:#?}"
        );
        assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
    }
    let related = published
        .diagnostics
        .iter()
        .find(|d| d.range == range)
        .and_then(|d| d.related_information.clone())
        .unwrap_or_default();
    assert!(
        related.iter().any(|r| r.location.uri.path().ends_with("m/code.py")
            && r.location.range.start == Position::new(1, 11)
            && r.message
                == "the fix is unsafe because the script uses code that is not vetted: `re.sys`"),
        "{related:#?}"
    );
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
    );
    for action in actions {
        let resolved = client.request::<CodeActionResolveRequest>(action);
        if let Some(edit) = resolved.edit {
            let fixed = apply(text, &edit.changes.unwrap()[&uri]);
            assert!(fixed.contains("    import math\n"), "{fixed}");
        }
    }
    client.shutdown();
}

/// Reviewer round 12, K1: in a library module every import that runs
/// binds `__package__`, so the quick fix that removes the only import of a
/// module reading it is offered only as unsafe (not preferred), with the
/// reason in its title, and fix-all keeps the import.
#[test]
fn quick_fix_for_an_import_whose_package_is_read_is_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "import math\n\ndef get_package():\n    try:\n        return __package__\n    except NameError:\n        return 'ABSENT'\n";
    let path = write(&root, &format!("{lib}/code.py"), text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, text);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    assert_eq!(range.start, Position::new(0, 7));
    let fixes: Vec<CodeAction> = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
    .collect();
    assert!(!fixes.is_empty(), "{fixes:#?}");
    for fix in &fixes {
        assert!(
            fix.title
                .ends_with("(unsafe: the import binds `__package__`, which code may read)"),
            "{fix:#?}"
        );
        assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
    }
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
    );
    for action in actions {
        let resolved = client.request::<CodeActionResolveRequest>(action);
        if let Some(edit) = resolved.edit {
            let fixed = apply(text, &edit.changes.unwrap()[&uri]);
            assert!(fixed.starts_with("import math\n"), "{fixed}");
        }
    }
    client.shutdown();
}

/// Review round 13: fix-all must keep the imports a class body reads by
/// name (mangled, or before the class binds the name) and the one the
/// interpreter reads as `__metaclass__`; none is reported.
#[test]
fn fix_all_keeps_imports_read_by_class_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let texts = [
        "import math as _D__m\n\nclass D(object):\n    x = __m.floor(3.7)\n",
        "import math as m\n\nclass C(object):\n    x = m.pi\n    m = 1\n\nm = None\n",
        "from math import pi as _D__pi\n\nclass D(object):\n    x = __pi\n",
        "import math as __metaclass__\ntry:\n    class C:\n        pass\nexcept TypeError:\n    print 'failed'\n",
    ];
    let mut paths = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let lib = format!("projects/demo/ignition/script-python/m{i}");
        write(
            &root,
            &format!("{lib}/resource.json"),
            &MANIFEST.replace("view.json", "code.py"),
        );
        paths.push(write(&root, &format!("{lib}/code.py"), text));
    }
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    for (path, text) in paths.iter().zip(texts) {
        let uri = client.open(path, text);
        let published = client.diagnostics(&uri, Some(1));
        assert!(
            !codes(&published)
                .iter()
                .any(|c| c == "SCR101" || c == "SCR102"),
            "{text}: {published:#?}"
        );
        let actions = code_actions(
            &mut client,
            &uri,
            full_range(),
            Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
        );
        for action in actions {
            let resolved = client.request::<CodeActionResolveRequest>(action);
            if let Some(edit) = resolved.edit {
                let fixed = apply(text, &edit.changes.unwrap()[&uri]);
                assert_eq!(fixed, text);
            }
        }
    }
    client.shutdown();
}

/// The VIEW015 quick fixes (and fix-all) of `view` in a workspace where
/// another view runs `script`: the titles and whether each is preferred.
fn view015_fixes(script: &str) -> Vec<(String, Option<bool>)> {
    view015_fixes_with(Some(script), None, None)
}

/// [`view015_fixes`] with view `B` holding a button that runs `script`, or
/// the view `b`, and `config` as the workspace's `iglint.toml`.
fn view015_fixes_with(
    script: Option<&str>,
    b: Option<&str>,
    config: Option<&str>,
) -> Vec<(String, Option<bool>)> {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let views = "projects/demo/com.inductiveautomation.perspective/views";
    let button = |script: &str| {
        format!(
            r#"{{"meta": {{"name": "Btn"}}, "type": "ia.input.button", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}}}"#
        )
    };
    let view = |custom: &str, script: &str| {
        format!(
            "{{\"custom\": {custom}, \"params\": {{}},\n  \"root\": {{\"meta\": {{\"name\": \"root\"}}, \"type\": \"ia.container.flex\", \"children\": [{}]}}}}\n",
            button(script)
        )
    };
    let a = view(
        r#"{"foo": 1, "bar": 2}"#,
        r"\tsystem.perspective.print(self.view.custom.bar)",
    );
    write(&root, &format!("{views}/A/resource.json"), MANIFEST);
    let path = write(&root, &format!("{views}/A/view.json"), &a);
    write(&root, &format!("{views}/B/resource.json"), MANIFEST);
    let b = match (script, b) {
        (Some(script), _) => view("{}", script),
        (None, Some(b)) => b.to_owned(),
        (None, None) => view("{}", ""),
    };
    write(&root, &format!("{views}/B/view.json"), &b);
    if let Some(config) = config {
        write(&root, "iglint.toml", config);
    }
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, &a);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| *c == lsp_types::NumberOrString::String("VIEW015".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("VIEW015") && !a.title.contains("suppress"))
    .map(|a| (a.title, a.is_preferred))
    .collect();
    client.shutdown();
    fixes
}

/// Reviewer round 9 (Opus) blocker, through the language server: another
/// view that reaches this view through the page makes the VIEW015 quick
/// fix unsafe (not preferred), naming the site; without it the fix is
/// safe and preferred.
#[test]
fn quick_fix_for_a_property_read_from_another_view_is_unsafe() {
    let fixes =
        view015_fixes(r"\tfor v in self.page.views:\n\t\tsystem.perspective.print(v.custom.foo)");
    assert!(!fixes.is_empty());
    for (title, preferred) in &fixes {
        assert!(title.contains("(unsafe: closed world"), "{title}");
        assert!(title.contains("views/B/view.json"), "{title}");
        assert_eq!(*preferred, Some(false), "{title}");
    }
    let fixes = view015_fixes(r"\tsystem.perspective.print(self.page.props.path)");
    assert!(!fixes.is_empty());
    for (title, preferred) in &fixes {
        assert!(!title.contains("unsafe"), "{title}");
        assert_eq!(*preferred, Some(true), "{title}");
    }
}

/// Reviewer round 18 (Opus) blocker B1, through the language server: a
/// third-party `system.perspective.runJavaScriptAsync` call, an expression
/// function the gateway does not register and a component that runs
/// JavaScript in the browser, in another view, make the VIEW015 quick fix
/// unsafe with the site; the project can vet the function.
#[test]
fn quick_fix_with_unknown_functions_or_browser_code_is_unsafe() {
    let b = |child: &str| {
        format!(
            r#"{{"custom": {{"y": 1}}, "propConfig": {{"custom.y": {{"binding": {{"type": "expr", "config": {{"expression": "now(0)"}}}}}}}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{child}]}}}}"#
        )
    };
    for (script, view, site) in [
        (
            Some(r"\tsystem.perspective.runJavaScriptAsync('x')"),
            None,
            "system.perspective.runJavaScriptAsync",
        ),
        (
            None,
            Some(b(r#"{"meta": {"name": "F"}, "type": "ia.display.iframe"}"#)),
            "ia.display.iframe",
        ),
        (
            None,
            Some(b(
                r#"{"meta": {"name": "L"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "objectScript('x')"}}}}}"#,
            )),
            "objectScript",
        ),
    ] {
        let fixes = view015_fixes_with(script, view.as_deref(), None);
        assert!(!fixes.is_empty(), "{site}");
        for (title, preferred) in &fixes {
            assert!(title.contains("(unsafe: closed world"), "{title}");
            assert!(title.contains(site), "{title}");
            assert!(title.contains("views/B/view.json"), "{title}");
            assert_eq!(*preferred, Some(false), "{title}");
        }
    }
    let fixes = view015_fixes_with(
        Some(r"\tsystem.perspective.runJavaScriptAsync('x')"),
        None,
        Some("[fix]\nvetted-system-functions = [\"system.perspective.runJavaScriptAsync\"]\n"),
    );
    assert!(!fixes.is_empty());
    for (title, preferred) in &fixes {
        assert!(!title.contains("unsafe"), "{title}");
        assert_eq!(*preferred, Some(true), "{title}");
    }
}

/// Reviewer round 18 (Opus) blockers B2 and N1, through the language
/// server: the quick fix of the review's repro (an import in a function
/// near the 64 KB method limit) is unsafe, says why and is not preferred.
#[test]
fn quick_fix_near_the_method_limit_is_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let mut text = String::from("def g():\n    import math\n");
    for i in 0..2_176 {
        text.push_str(&format!("    a = abs({})\n", i % 7));
    }
    text.push_str("    c = 1\n    c = 1\n    return a + c\n");
    let path = write(&root, &format!("{lib}/code.py"), &text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, &text);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes: Vec<CodeAction> = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
    .collect();
    assert!(!fixes.is_empty());
    for f in &fixes {
        assert!(
            f.title.contains("compile status is not known"),
            "{}",
            f.title
        );
        assert_ne!(f.is_preferred, Some(true), "{}", f.title);
    }
    client.shutdown();
}

/// Reviewer round 20 (Codex) blocker, through the language server: the
/// quick fix of the review's repro (an import in a function of 1,683
/// reads of an attribute of 16,001 characters) is unsafe, names the
/// identifier and is not preferred.
#[test]
fn quick_fix_with_long_identifiers_is_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let mut text = String::from("def work(obj):\n    import math\n");
    for _ in 0..1_683 {
        text.push_str(&format!("    obj.{}\n", "n".repeat(16_001)));
    }
    text.push_str("    return 1\nprint \"loaded\"\n");
    let path = write(&root, &format!("{lib}/code.py"), &text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, &text);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes: Vec<CodeAction> = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
    .collect();
    assert!(!fixes.is_empty());
    for f in &fixes {
        assert!(
            f.title.ends_with(
                "(unsafe: the safe fix could change what the code does (it changes a script whose compile status is not known: identifier of 16,001 characters: Jython's code size cannot be bounded precisely))"
            ),
            "{}",
            f.title
        );
        assert_ne!(f.is_preferred, Some(true), "{}", f.title);
    }
    client.shutdown();
}

/// Reviewer round 9 (Codex) blocker, through the language server: removing
/// an import that fails (a class that does not exist) changes what the
/// program does when the caller catches the error, so the quick fix is
/// unsafe; an import of a class measured to exist keeps the safe fix.
#[test]
fn quick_fix_for_an_import_that_fails_is_unsafe() {
    for (import, safe) in [
        ("from java.util import MissingReviewClass", false),
        ("from java.util import ArrayList", true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        let lib = "projects/demo/ignition/script-python/m";
        write(
            &root,
            &format!("{lib}/resource.json"),
            &MANIFEST.replace("view.json", "code.py"),
        );
        let text = format!(
            "def f():\n    {import}\ntry:\n    f()\nexcept Exception:\n    print 'failed'\nelse:\n    print 'succeeded'\n"
        );
        let path = write(&root, &format!("{lib}/code.py"), &text);
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let uri = client.open(&path, &text);
        let published = client.diagnostics(&uri, Some(1));
        let range = published
            .diagnostics
            .iter()
            .find(|d| {
                d.code
                    .as_ref()
                    .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
            })
            .unwrap_or_else(|| panic!("{published:#?}"))
            .range;
        let fixes: Vec<CodeAction> = code_actions(
            &mut client,
            &uri,
            range,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
        .into_iter()
        .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
        .collect();
        assert!(!fixes.is_empty(), "{fixes:#?}");
        assert_eq!(
            fixes.iter().any(|f| f.is_preferred == Some(true)),
            safe,
            "{import}: {fixes:#?}"
        );
        if !safe {
            assert!(
                fixes.iter().all(|f| f.title.contains("(unsafe")),
                "{fixes:#?}"
            );
        }
        client.shutdown();
    }
}

/// Reviewer round 10 (Codex) blocker, through the language server: a
/// script that reads its own file (`open(...).read()`) or another script's
/// file observes every fix, so the quick fixes of `SCR102` and `SCR124` are
/// unsafe and name the read, and fix-all applies neither; with `[fix]
/// assume-scripts-do-not-read-workspace-files = true` they are safe again.
#[test]
fn quick_fixes_are_unsafe_when_scripts_read_workspace_files() {
    for assume in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        let mut config = String::from("extend-select = [\"SCR124\"]\n");
        if assume {
            config.push_str("[fix]\nassume-scripts-do-not-read-workspace-files = true\n");
        }
        write(&root, "iglint.toml", &config);
        let lib = "projects/demo/ignition/script-python";
        let manifest = MANIFEST.replace("view.json", "code.py");
        write(&root, &format!("{lib}/r/resource.json"), &manifest);
        write(
            &root,
            &format!("{lib}/r/code.py"),
            "def last(p):\n    return open(p).read().splitlines()[-1]\n",
        );
        write(&root, &format!("{lib}/m/resource.json"), &manifest);
        let text = "import math\nprint open('/w/projects/p/ignition/script-python/m/code.py').read().startswith('import math\\n')\nx = 1  # note  \n";
        let path = write(&root, &format!("{lib}/m/code.py"), text);
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let uri = client.open(&path, text);
        let published = client.diagnostics(&uri, Some(1));
        for code in ["SCR102", "SCR124"] {
            let range = published
                .diagnostics
                .iter()
                .find(|d| {
                    d.code
                        .as_ref()
                        .is_some_and(|c| *c == lsp_types::NumberOrString::String(code.to_owned()))
                })
                .unwrap_or_else(|| panic!("{code}: {published:#?}"))
                .range;
            let fixes: Vec<CodeAction> = code_actions(
                &mut client,
                &uri,
                range,
                Some(vec![CodeActionKind::QUICKFIX]),
            )
            .into_iter()
            .filter(|a| a.title.starts_with(code) && !a.title.contains("suppress"))
            .collect();
            assert!(!fixes.is_empty(), "{code}: {fixes:#?}");
            for fix in &fixes {
                if assume {
                    assert!(!fix.title.contains("unsafe"), "{fix:#?}");
                } else {
                    assert!(
                        fix.title
                            .contains("(unsafe: code in the workspace may read the files"),
                        "{fix:#?}"
                    );
                    assert!(fix.title.contains("open("), "{fix:#?}");
                    assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
                }
            }
        }
        let actions = code_actions(
            &mut client,
            &uri,
            full_range(),
            Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
        );
        let mut fixed = text.to_owned();
        for action in actions {
            let resolved = client.request::<CodeActionResolveRequest>(action);
            if let Some(edit) = resolved.edit {
                fixed = apply(text, &edit.changes.unwrap()[&uri]);
            }
        }
        if assume {
            assert!(fixed.ends_with("x = 1  # note\n"), "{fixed}");
        } else {
            assert_eq!(fixed, text);
        }
        client.shutdown();
    }
}

/// Reviewer round 11: a rename of a workspace file to a literal outside
/// path, through a library helper, and an append-mode open (which reveals
/// the size) count as possible reads in the language server too.
#[test]
fn quick_fixes_are_unsafe_when_scripts_move_or_append_to_workspace_files() {
    let target = "/usr/local/bin/ignition/data/projects/p/ignition/script-python/m/code.py";
    for text in [
        format!(
            "import math\nimport os\nos.rename('{target}', '/tmp/m.py')\nprint open('/tmp/m.py').read()\n"
        ),
        format!("import math\nr.mv('{target}', '/tmp/m.py')\nprint open('/tmp/m.py').read()\n"),
        format!("import math\nf = open('{target}', 'a')\nf.seek(0, 2)\nprint f.tell()\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        let lib = "projects/demo/ignition/script-python";
        let manifest = MANIFEST.replace("view.json", "code.py");
        write(&root, &format!("{lib}/r/resource.json"), &manifest);
        write(
            &root,
            &format!("{lib}/r/code.py"),
            "import os\ndef mv(a, b):\n    os.rename(a, b)\n",
        );
        write(&root, &format!("{lib}/m/resource.json"), &manifest);
        let path = write(&root, &format!("{lib}/m/code.py"), &text);
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let uri = client.open(&path, &text);
        let published = client.diagnostics(&uri, Some(1));
        let range = published
            .diagnostics
            .iter()
            .find(|d| {
                d.code
                    .as_ref()
                    .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
            })
            .unwrap_or_else(|| panic!("{published:#?}"))
            .range;
        let fixes: Vec<CodeAction> = code_actions(
            &mut client,
            &uri,
            range,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
        .into_iter()
        .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
        .collect();
        assert!(!fixes.is_empty(), "{text}: {fixes:#?}");
        for fix in &fixes {
            assert!(
                fix.title
                    .contains("(unsafe: code in the workspace may read the files"),
                "{text}: {fix:#?}"
            );
            assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
        }
        client.shutdown();
    }
}

/// Review round 12 (Opus) blocker, in the language server: the quick fix
/// of trailing whitespace in a tag event script edits `tags.json`, which
/// `system.tag.getConfiguration`, a `readBlocking` of `T.eventScripts` and
/// `exportTags` followed by `open` read without naming the file. A read of
/// the tag's value keeps the fix safe.
#[test]
fn quick_fixes_are_unsafe_when_scripts_read_tag_configuration() {
    for (code, unsafe_fix) in [
        (
            "def f():\n    return system.tag.getConfiguration('[default]T')\n",
            true,
        ),
        (
            "def f():\n    return system.tag.readBlocking(['[default]T.eventScripts'])\n",
            true,
        ),
        (
            "def f():\n    system.tag.exportTags('/tmp/x.json', ['[default]T'])\n    return open('/tmp/x.json').read()\n",
            true,
        ),
        (
            "def f():\n    return system.tag.readBlocking(['[default]T'])\n",
            false,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        write(&root, "iglint.toml", "extend-select = [\"SCR124\"]\n");
        let lib = "projects/demo/ignition/script-python";
        write(
            &root,
            &format!("{lib}/m/resource.json"),
            &MANIFEST.replace("view.json", "code.py"),
        );
        write(&root, &format!("{lib}/m/code.py"), code);
        write(&root, "config/core/config-mode.json", "{}");
        write(
            &root,
            "config/core/ignition/tag-provider/default/resource.json",
            &MANIFEST.replace("view.json", "config.json"),
        );
        write(
            &root,
            "config/core/ignition/tag-provider/default/config.json",
            r#"{"profile": {"type": "STANDARD"}, "settings": {}}"#,
        );
        write(
            &root,
            "config/core/ignition/tag-definition/default/unary-resource.json",
            &MANIFEST
                .replace("view.json", "tags.json")
                .replace(r#""attributes": {}"#, r#""attributes": {"config": {}}"#),
        );
        let tags = r#"[{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Int4", "eventScripts": [{"eventid": "valueChanged", "script": "\tx = 1   \n"}]}]"#;
        let path = write(
            &root,
            "config/core/ignition/tag-definition/default/tags.json",
            tags,
        );
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let uri = client.open(&path, tags);
        let published = client.diagnostics(&uri, Some(1));
        let range = published
            .diagnostics
            .iter()
            .find(|d| {
                d.code
                    .as_ref()
                    .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR124".to_owned()))
            })
            .unwrap_or_else(|| panic!("{code}: {published:#?}"))
            .range;
        let fixes: Vec<CodeAction> = code_actions(
            &mut client,
            &uri,
            range,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
        .into_iter()
        .filter(|a| a.title.starts_with("SCR124") && !a.title.contains("suppress"))
        .collect();
        assert!(!fixes.is_empty(), "{code}: {fixes:#?}");
        for fix in &fixes {
            if unsafe_fix {
                assert!(
                    fix.title
                        .contains("(unsafe: code in the workspace may read the files"),
                    "{code}: {fix:#?}"
                );
                assert!(fix.title.contains("system.tag."), "{code}: {fix:#?}");
                assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
            } else {
                assert!(!fix.title.contains("unsafe"), "{code}: {fix:#?}");
            }
        }
        client.shutdown();
    }
}

/// The NAME001 quick fixes offered for the component `name` of `view`: the
/// title and whether each is preferred (safe).
fn name001_fixes(view: &str, name: &str) -> Vec<(String, Option<bool>)> {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    write(
        &root,
        "iglint.toml",
        "extend-select = [\"NAME001\"]\n\n[rules.NAME001]\nconvention = \"PascalCase\"\n",
    );
    let views = "projects/demo/com.inductiveautomation.perspective/views";
    write(&root, &format!("{views}/R10/resource.json"), MANIFEST);
    let path = write(&root, &format!("{views}/R10/view.json"), view);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, view);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| d.message.contains(&format!("'{name}'")))
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("NAME001") && !a.title.contains("suppress"))
    .map(|a| (a.title, a.is_preferred))
    .collect();
    client.shutdown();
    fixes
}

/// Reviewer round 10, O1 and O2, through the language server: a
/// reference in an HTTP header value, or a script reading the
/// component's `name`, makes the rename quick fix unsafe (not preferred);
/// without them it is safe and preferred.
#[test]
fn rename_quick_fix_sees_measured_fields_and_name_reads() {
    let view = |extra: &str, script: &str| {
        format!(
            r#"{{"custom": {{"s": ""}}, "params": {{}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [
  {{"meta": {{"name": "api_token"}}, "type": "ia.input.text-field", "events": {{"component": {{"onActionPerformed": {{"type": "script", "scope": "G", "config": {{"script": "{script}"}}}}}}}}}},
  {{"meta": {{"name": "Chart"}}, "type": "ia.display.label"{extra}}}]}}}}
"#
        )
    };
    let http = r#", "propConfig": {"props.text": {"binding": {"type": "http", "config": {"request": {"url": "'u'", "method": "GET", "header": [{"key": "A", "value": "'Bearer ' + {../api_token.props.text}"}]}}}}}"#;
    for (extra, script) in [
        (http, r"\tpass"),
        ("", r"\tself.view.custom.s = self.name"),
        ("", r"\tself.view.custom.s = self.getName()"),
    ] {
        let fixes = name001_fixes(&view(extra, script), "api_token");
        assert!(!fixes.is_empty(), "{script}");
        for (title, preferred) in &fixes {
            assert!(title.contains("unsafe"), "{title}");
            assert_eq!(*preferred, Some(false), "{title}");
        }
    }
    let fixes = name001_fixes(&view("", r"\tpass"), "api_token");
    assert!(!fixes.is_empty());
    for (title, preferred) in &fixes {
        assert!(!title.contains("unsafe"), "{title}");
        assert_eq!(*preferred, Some(true), "{title}");
    }
}

/// Review round 21, B1, through the language server: a rename to the name
/// that broken references already use (a script lookup and a binding, an
/// expression, a case-only change) is offered as unsafe, not preferred,
/// with the reason in its title.
#[test]
fn rename_quick_fix_to_a_name_broken_references_use_is_unsafe() {
    let lookup = r#"{"meta": {"name": "Btn"}, "type": "ia.input.button", "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlbl = self.getSibling('MyLabel')\n\tprint lbl is None"}}}}}"#;
    let binding = r#"{"meta": {"name": "Other"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "property", "config": {"path": "../MyLabel.props.text"}}}}}"#;
    let expression = r#"{"meta": {"name": "Other"}, "type": "ia.display.label", "propConfig": {"props.text": {"binding": {"type": "expr", "config": {"expression": "coalesce({/root/MyLabel.props.text}, 'none')"}}}}}"#;
    for (old, others) in [
        ("my_label", format!("{lookup}, {binding}")),
        ("my_label", expression.to_owned()),
        ("myLabel", lookup.to_owned()),
    ] {
        let view = format!(
            r#"{{"custom": {{}}, "params": {{}}, "root": {{"meta": {{"name": "root"}}, "type": "ia.container.flex", "children": [{{"meta": {{"name": "{old}"}}, "type": "ia.display.label"}}, {others}]}}}}"#
        );
        let fixes = name001_fixes(&view, old);
        assert_eq!(fixes.len(), 1, "{fixes:?}");
        let (title, preferred) = &fixes[0];
        assert!(
            title.contains("(unsafe: the new name already occurs in the view"),
            "{title}"
        );
        assert_eq!(*preferred, Some(false), "{title}");
    }
}

/// Review round 14 (Opus) blocker: a folder opened on one project still
/// sees the rest of the workspace (the root is discovered above it), so a
/// gateway tag script that uses `m.os` keeps `import os` used, and one
/// that is not vetted makes the fix unsafe, as in a run over the root.
#[test]
fn a_folder_inside_the_workspace_sees_the_whole_workspace() {
    let module = "projects/A/ignition/script-python/m/code.py";
    let text = "import os\n\ndef f():\n    return 1\n";
    for (tag_script, expected) in [
        ("\\tprint m.os.sep\\n", None),
        ("\\tprint eval('1')\\n", Some("closed world")),
        ("\\tprint 1\\n", Some("")),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        write(
            &root,
            "projects/A/project.json",
            r#"{"title": "A", "enabled": true, "inheritable": false}"#,
        );
        write(
            &root,
            "projects/A/ignition/script-python/m/resource.json",
            &MANIFEST.replace("view.json", "code.py"),
        );
        let path = write(&root, module, text);
        let core = "services/gw/config/resources/core";
        write(&root, &format!("{core}/config-mode.json"), "{}");
        write(
            &root,
            &format!("{core}/ignition/tag-definition/default/unary-resource.json"),
            r#"{"scope": "G", "version": 1, "files": ["tags.json"], "attributes": {"config": {}}}"#,
        );
        write(
            &root,
            &format!("{core}/ignition/tag-definition/default/tags.json"),
            &format!(
                r#"[{{"name": "T", "tagType": "AtomicTag", "valueSource": "memory", "dataType": "Int4", "eventScripts": [{{"eventid": "valueChanged", "script": "{tag_script}"}}]}}]"#
            ),
        );
        let mut client = Client::start(Some(&root.join("projects/A")), json!({"debounceMs": 0}));
        let uri = client.open(&path, text);
        let published = client.diagnostics(&uri, Some(1));
        let found: Vec<_> = published
            .diagnostics
            .iter()
            .filter(|d| d.code == Some(lsp_types::NumberOrString::String("SCR102".into())))
            .collect();
        let Some(reason) = expected else {
            assert!(found.is_empty(), "{tag_script}: {published:#?}");
            continue;
        };
        assert_eq!(found.len(), 1, "{tag_script}: {published:#?}");
        let fixes: Vec<CodeAction> = code_actions(
            &mut client,
            &uri,
            found[0].range,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
        .into_iter()
        .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
        .collect();
        assert_eq!(fixes.len(), 1, "{tag_script}: {fixes:#?}");
        if reason.is_empty() {
            assert_eq!(
                fixes[0].is_preferred,
                Some(true),
                "{tag_script}: {fixes:#?}"
            );
        } else {
            assert!(
                fixes[0].title.contains(&format!("(unsafe: {reason}")),
                "{tag_script}: {}",
                fixes[0].title
            );
        }
        client.shutdown();
    }
}

/// The language server sends positions in the editor's lines, where a lone
/// CR ends a line (the protocol's rule), although iglint's own output
/// numbers lines as Jython does (review round 14): the range must cover
/// the same text either way.
#[test]
fn positions_with_lone_cr_follow_the_protocol() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    write(
        &root,
        "projects/A/project.json",
        r#"{"title": "A", "enabled": true, "inheritable": false}"#,
    );
    write(
        &root,
        "projects/A/ignition/script-python/m/resource.json",
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "x = 1\r\ry = $";
    let path = write(&root, "projects/A/ignition/script-python/m/code.py", text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, text);
    let published = client.diagnostics(&uri, Some(1));
    let d = published
        .diagnostics
        .iter()
        .find(|d| d.code == Some(lsp_types::NumberOrString::String("SCR001".into())))
        .unwrap_or_else(|| panic!("{published:#?}"));
    assert_eq!(d.range.start, Position::new(2, 0), "{d:#?}");
    client.shutdown();
}

/// Reviewer round 16 B1: project `Q`'s library module `lib/a` is a
/// symbolic link to `P`'s file, and `Q` uses `lib.a.os`. The quick fix
/// that removes `P`'s unused `import os` changes `Q`'s module too, so it is
/// unsafe (not preferred), names the link, and fix-all leaves the file.
#[cfg(unix)]
#[test]
fn quick_fix_of_a_file_reached_through_a_link_is_unsafe() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let manifest = MANIFEST.replace("view.json", "code.py");
    for p in ["P", "Q"] {
        write(
            &root,
            &format!("projects/{p}/project.json"),
            r#"{"title": "p", "enabled": true, "inheritable": false}"#,
        );
        write(
            &root,
            &format!("projects/{p}/ignition/script-python/lib/a/resource.json"),
            &manifest,
        );
    }
    let text = "import math\nimport os\n\ndef f():\n    return math.pi\n";
    let path = write(
        &root,
        "projects/P/ignition/script-python/lib/a/code.py",
        text,
    );
    let link = root.join("projects/Q/ignition/script-python/lib/a/code.py");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    write(
        &root,
        "projects/Q/ignition/script-python/use/b/resource.json",
        &manifest,
    );
    write(
        &root,
        "projects/Q/ignition/script-python/use/b/code.py",
        "def g():\n    return lib.a.os.sep\n",
    );
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, text);
    let published = client.diagnostics(&uri, Some(1));
    let range = published
        .diagnostics
        .iter()
        .find(|d| {
            d.code
                .as_ref()
                .is_some_and(|c| *c == lsp_types::NumberOrString::String("SCR102".to_owned()))
        })
        .unwrap_or_else(|| panic!("{published:#?}"))
        .range;
    let fixes: Vec<CodeAction> = code_actions(
        &mut client,
        &uri,
        range,
        Some(vec![CodeActionKind::QUICKFIX]),
    )
    .into_iter()
    .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
    .collect();
    assert!(!fixes.is_empty(), "{fixes:#?}");
    for fix in &fixes {
        assert!(
            fix.title.contains("(unsafe: the fix edits `")
                && fix.title.contains(
                    "projects/Q/ignition/script-python/lib/a/code.py` (a symbolic link to it)"
                ),
            "{fix:#?}"
        );
        assert_eq!(fix.is_preferred, Some(false), "{fix:#?}");
    }
    let actions = code_actions(
        &mut client,
        &uri,
        full_range(),
        Some(vec![CodeActionKind::SOURCE_FIX_ALL]),
    );
    for action in actions {
        let resolved = client.request::<CodeActionResolveRequest>(action);
        if let Some(edit) = resolved.edit {
            assert_eq!(apply(text, &edit.changes.unwrap()[&uri]), text);
        }
    }
    client.shutdown();
}

/// Runs Git in `cwd` with a fixed identity; fails the test when it fails.
fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .current_dir(cwd)
        .args([
            "-c",
            "user.name=iglint",
            "-c",
            "user.email=iglint@example.invalid",
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

/// Review round 17 (Opus) blocker: a linked work tree in a hidden
/// directory of the folder (`git worktree add .worktrees/x`) is a
/// checkout of its own. A file opened from it is linted with the work
/// tree's workspace, where a library reads files, so the quick fix is
/// unsafe; the same file of the main checkout keeps its safe fix.
#[test]
fn a_linked_work_tree_below_the_folder_is_its_own_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    write(
        &root,
        "projects/A/project.json",
        r#"{"title": "A", "enabled": true, "inheritable": false}"#,
    );
    let manifest = MANIFEST.replace("view.json", "code.py");
    let lib = "projects/A/ignition/script-python";
    write(&root, &format!("{lib}/m/resource.json"), &manifest);
    let text = "import os\n\ndef f():\n    return 1\n";
    let main = write(&root, &format!("{lib}/m/code.py"), text);
    git(&root, &["init", "-q"]);
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-q", "-m", "main"]);
    git(
        &root,
        &["worktree", "add", "-q", "-b", "feature", ".worktrees/x"],
    );
    let wt = root.join(".worktrees/x");
    write(&wt, &format!("{lib}/r/resource.json"), &manifest);
    write(
        &wt,
        &format!("{lib}/r/code.py"),
        "def last(p):\n    return open(p).read()\n",
    );
    let feature = wt.join(lib).join("m/code.py");
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    for (path, unsafe_fix) in [(&feature, true), (&main, false)] {
        let uri = client.open(path, text);
        let published = client.diagnostics(&uri, Some(1));
        assert!(
            !codes(&published).contains(&"REF009".to_owned()),
            "{published:#?}"
        );
        let range = published
            .diagnostics
            .iter()
            .find(|d| d.code == Some(lsp_types::NumberOrString::String("SCR102".into())))
            .unwrap_or_else(|| panic!("{published:#?}"))
            .range;
        let fixes: Vec<CodeAction> = code_actions(
            &mut client,
            &uri,
            range,
            Some(vec![CodeActionKind::QUICKFIX]),
        )
        .into_iter()
        .filter(|a| a.title.starts_with("SCR102") && !a.title.contains("suppress"))
        .collect();
        assert!(!fixes.is_empty(), "{fixes:#?}");
        for fix in &fixes {
            assert_eq!(
                fix.title
                    .contains("(unsafe: code in the workspace may read the files"),
                unsafe_fix,
                "{} {fix:#?}",
                path.display()
            );
        }
    }
    client.shutdown();
}
