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

/// Review round 26 (Opus, M1): while a script does not parse (in the
/// middle of an edit) its suppressions are not judged, so fix-all on save
/// keeps them.
#[test]
fn suppressions_in_scripts_that_do_not_parse_survive_fix_all() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "def f():\n    x = 1  # iglint: ignore[SCR103]\n    return 2\nv = (\n";
    let path = write(&root, &format!("{lib}/code.py"), text);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, text);
    let published = client.diagnostics(&uri, Some(1));
    let codes: Vec<_> = published
        .diagnostics
        .iter()
        .filter_map(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(c)) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    assert!(codes.contains(&"SCR001"), "{codes:?}");
    assert!(!codes.contains(&"SCR123"), "{codes:?}");
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
            assert!(fixed.contains("# iglint: ignore[SCR103]"), "{fixed}");
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
/// defined before `import re`, uses an attribute of `re` that is not
/// vetted is offered only as unsafe (not preferred), and fix-all keeps the
/// import. (The round 8 exploit reached `eval` through `re.sys`, which
/// since review round 25 reads names for the findings too: no finding.)
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
    let text = "def capture():\n    return re.sre_parse\nimport re\ndef target():\n    import math\n    print capture()\ntarget()\n";
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
                "(unsafe: the script uses code that is not vetted: `re.sre_parse` at \
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
                == "the fix is unsafe because the script uses code that is not vetted: `re.sre_parse`"),
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

/// Review round 22 (Codex): after `del math.pi`, `from math import pi`
/// raises `ImportError`; the quick fix that removes it is unsafe, not
/// preferred, and fix-all keeps the import.
#[test]
fn quick_fix_after_a_shared_module_change_is_not_preferred() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "import math\ndel math.pi\ndef f():\n    from math import pi\n    return 7\ntry:\n    print f()\nexcept ImportError:\n    print \"missing\"\n";
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
    assert_eq!(range.start, Position::new(3, 21));
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
                .contains("(unsafe: the script uses code that is not vetted"),
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
            assert!(fixed.contains("    from math import pi\n"), "{fixed}");
        }
    }
    client.shutdown();
}

/// Review round 23 (Opus): `class K: import math` then `del K.math.pi`
/// changes the shared module through a class attribute; the quick fix
/// that removes `from math import pi` is unsafe, not preferred, and
/// fix-all keeps the import.
#[test]
fn quick_fix_after_a_class_attribute_module_change_is_not_preferred() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let lib = "projects/demo/ignition/script-python/m";
    write(
        &root,
        &format!("{lib}/resource.json"),
        &MANIFEST.replace("view.json", "code.py"),
    );
    let text = "class K:\n    import math\ndel K.math.pi\ndef f():\n    from math import pi\n    return 7\ntry:\n    print f()\nexcept ImportError:\n    print \"missing\"\n";
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
    assert_eq!(range.start, Position::new(4, 21));
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
                .contains("(unsafe: the script uses code that is not vetted"),
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
            assert!(fixed.contains("    from math import pi\n"), "{fixed}");
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

const LIBRARY: &str = "projects/demo/ignition/script-python";

/// The diagnostics last published for `uri` since the last call, after
/// everything the server did before answering a request sent now.
fn settled(client: &mut Client, uri: &Url) -> Option<lsp_types::PublishDiagnosticsParams> {
    client.request_raw(
        "workspace/executeCommand",
        json!({"command": "iglint.ruleDocs", "arguments": ["VIEW001"]}),
    );
    let mut last = None;
    client.inbox.retain(|m| match m {
        Message::Notification(n)
            if n.method == lsp_types::notification::PublishDiagnostics::METHOD =>
        {
            match serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(n.params.clone()) {
                Ok(p) if p.uri == *uri => {
                    last = Some(p);
                    false
                }
                _ => true,
            }
        }
        _ => true,
    });
    last
}

fn has(p: &lsp_types::PublishDiagnosticsParams, code: &str, text: &str) -> bool {
    p.diagnostics.iter().any(|d| {
        d.code == Some(lsp_types::NumberOrString::String(code.to_owned()))
            && d.message.contains(text)
    })
}

/// Review round 29 (Opus, B2): an edit of a module in the middle of a
/// chain (`m1` now imports `m2`, which imports `lib.util`, so `b`'s
/// `m1.m2.util.json.dumps(1)` reads `json` of `lib.util`) re-publishes
/// `lib.util`, whose `import json` is now used, and no quick fix removes
/// it: after a change in the editor, and after a change on disk reported
/// by `didChangeWatchedFiles`.
#[test]
fn an_edit_in_the_middle_of_a_chain_updates_the_module_at_its_end() {
    for on_disk in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        let manifest = MANIFEST.replace("view.json", "code.py");
        let module = |name: &str, code: &str| {
            write(&root, &format!("{LIBRARY}/{name}/resource.json"), &manifest);
            write(&root, &format!("{LIBRARY}/{name}/code.py"), code)
        };
        let util_text = "import json\ndef f():\n    return 1\n";
        let util = module("lib/util", util_text);
        module("m2", "from lib import util\n");
        let m1_text = "def x():\n    return 1\n";
        let m1 = module("m1", m1_text);
        let b_text = "def g(n):\n    return m1.m2.util.json.dumps(1)\n";
        let b = module("b", b_text);
        let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
        let util_uri = client.open(&util, util_text);
        let before = client.diagnostics(&util_uri, Some(1));
        assert!(has(&before, "SCR102", "`json`"), "{before:#?}");
        let b_uri = client.open(&b, b_text);
        client.diagnostics(&b_uri, Some(1));
        let edited = "import m2\ndef x():\n    return 1\n";
        if on_disk {
            std::fs::write(&m1, edited).unwrap();
            client.notify(
                DidChangeWatchedFiles::METHOD,
                json!({"changes": [{"uri": Url::from_file_path(&m1).unwrap(), "type": 2}]}),
            );
        } else {
            let m1_uri = client.open(&m1, m1_text);
            client.diagnostics(&m1_uri, Some(1));
            client.replace_all(&m1_uri, 2, edited);
            client.diagnostics(&m1_uri, Some(2));
        }
        let after = settled(&mut client, &util_uri)
            .unwrap_or_else(|| panic!("lib.util was not published again (on disk: {on_disk})"));
        assert!(!has(&after, "SCR102", "`json`"), "{after:#?}");
        let actions = code_actions(&mut client, &util_uri, full_range(), None);
        assert!(
            !actions.iter().any(|a| a.title.contains("SCR102")),
            "{actions:#?}"
        );
        client.shutdown();
    }
}

/// Review round 29 (Opus, B2): `VIEW015` follows a component into a
/// library function that hands it to another one; an edit of the second
/// changes which custom property is read, and the view is published again.
#[test]
fn view015_through_two_library_calls_follows_an_edit_of_the_second() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    demo_project(&root);
    let manifest = MANIFEST.replace("view.json", "code.py");
    let module = |name: &str, code: &str| {
        write(&root, &format!("{LIBRARY}/{name}/resource.json"), &manifest);
        write(&root, &format!("{LIBRARY}/{name}/code.py"), code)
    };
    module("lib/a", "def f(c):\n    return lib.two.k(c)\n");
    let two_text = "def k(c):\n    return c.custom.foo\n";
    let two = module("lib/two", two_text);
    let view = r#"{"params": {},
  "root": {"meta": {"name": "root"}, "type": "ia.container.flex", "children": [{"meta": {"name": "Btn"}, "type": "ia.input.button", "custom": {"foo": 1, "bar": 2}, "events": {"component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\tlib.a.f(self)"}}}}}]}}
"#;
    write(&root, &format!("{VIEWS}/A/resource.json"), MANIFEST);
    let path = write(&root, &format!("{VIEWS}/A/view.json"), view);
    let mut client = Client::start(Some(&root), json!({"debounceMs": 0}));
    let uri = client.open(&path, view);
    let before = client.diagnostics(&uri, Some(1));
    assert!(has(&before, "VIEW015", "`bar`"), "{before:#?}");
    assert!(!has(&before, "VIEW015", "`foo`"), "{before:#?}");
    let two_uri = client.open(&two, two_text);
    client.diagnostics(&two_uri, Some(1));
    client.replace_all(&two_uri, 2, "def k(c):\n    return c.custom.bar\n");
    client.diagnostics(&two_uri, Some(2));
    let after = settled(&mut client, &uri).expect("the view is published again");
    assert!(has(&after, "VIEW015", "`foo`"), "{after:#?}");
    assert!(!has(&after, "VIEW015", "`bar`"), "{after:#?}");
    client.shutdown();
}

/// Review round 30 (Codex, B2): fixes are computed from the editor's text
/// of every open document, whatever the run trigger and debounce. With
/// `run: onSave`, or `onType` with a long debounce, an edit of `m1` (now
/// `import m2`, so `b`'s `m1.m2.util.json.dumps(1)` reads `json` of
/// `lib.util`) has not been analysed when a code action of `lib.util` is
/// asked for: no quick fix, fix-all edit, resolved fix-all or fix-all
/// command removes `import json`. Closing `m1` without saving brings the
/// disk text back, and the removal is offered again.
#[test]
fn fixes_see_pending_edits_of_other_documents() {
    for init in [
        json!({"run": "onSave"}),
        json!({"run": "onType", "debounceMs": 60000}),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        demo_project(&root);
        let manifest = MANIFEST.replace("view.json", "code.py");
        let module = |name: &str, code: &str| {
            write(&root, &format!("{LIBRARY}/{name}/resource.json"), &manifest);
            write(&root, &format!("{LIBRARY}/{name}/code.py"), code)
        };
        let util_text = "import json\ndef f():\n    return 1\n";
        let util = module("lib/util", util_text);
        module("m2", "from lib import util\n");
        let m1_text = "def x():\n    return 1\n";
        let m1 = module("m1", m1_text);
        module("b", "def g(n):\n    return m1.m2.util.json.dumps(1)\n");
        let mut client = Client::start(Some(&root), init.clone());
        let util_uri = client.open(&util, util_text);
        let before = client.diagnostics(&util_uri, Some(1));
        assert!(has(&before, "SCR102", "`json`"), "{before:#?}");
        let m1_uri = client.open(&m1, m1_text);
        client.diagnostics(&m1_uri, Some(1));
        let removes_json = |client: &mut Client| -> Vec<String> {
            let mut found = Vec::new();
            for action in code_actions(
                client,
                &util_uri,
                Range::new(Position::new(0, 0), Position::new(0, 30)),
                None,
            ) {
                let action = if action.edit.is_none() && action.data.is_some() {
                    client.request::<CodeActionResolveRequest>(action)
                } else {
                    action
                };
                let edits = action
                    .edit
                    .as_ref()
                    .and_then(|e| e.changes.as_ref())
                    .and_then(|c| c.get(&util_uri))
                    .cloned()
                    .unwrap_or_default();
                if !apply(util_text, &edits).contains("import json") {
                    found.push(action.title.clone());
                }
            }
            found
        };
        // Before the edit: offered (also the preferred quick fix).
        assert!(!removes_json(&mut client).is_empty(), "{init}");
        client.replace_all(&m1_uri, 2, "import m2\ndef x():\n    return 1\n");
        let removals = removes_json(&mut client);
        assert!(removals.is_empty(), "{init}: {removals:?}");
        let applied = client.request_raw(
            "workspace/executeCommand",
            json!({"command": "iglint.applyFixAll", "arguments": [util_uri.as_str()]}),
        );
        assert_eq!(applied, json!(false), "{init}");
        // The fresh check of the editor's texts agrees.
        let after = settled(&mut client, &util_uri);
        assert!(after.is_none_or(|p| !has(&p, "SCR102", "`json`")), "{init}");
        // Closing `m1` unsaved: the disk text again.
        client.notify(
            lsp_types::notification::DidCloseTextDocument::METHOD,
            json!({"textDocument": {"uri": m1_uri}}),
        );
        assert!(!removes_json(&mut client).is_empty(), "{init}");
        client.shutdown();
    }
}

/// Review round 31: a project `child` that inherits from `base`, with
/// `lib.util` in `child` (`import json`, unused there) and `m1` in `base`.
/// Returns the paths of `lib/util` and `m1`.
fn parent_and_child(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    write(
        root,
        "projects/base/project.json",
        r#"{"title":"base","parent":"","enabled":true,"inheritable":true}"#,
    );
    write(
        root,
        "projects/child/project.json",
        r#"{"title":"child","parent":"base","enabled":true,"inheritable":false}"#,
    );
    write(
        root,
        "pyproject.toml",
        "[tool.iglint]\nselect = [\"SCR102\"]\n",
    );
    let manifest = r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#;
    write(
        root,
        "projects/child/ignition/script-python/lib/util/resource.json",
        manifest,
    );
    let util = write(
        root,
        "projects/child/ignition/script-python/lib/util/code.py",
        UTIL,
    );
    write(
        root,
        "projects/base/ignition/script-python/m1/resource.json",
        manifest,
    );
    let m1 = write(root, "projects/base/ignition/script-python/m1/code.py", M1);
    (util, m1)
}

const UTIL: &str = "import json\ndef f():\n    return 1\n";
const M1: &str = "def g():\n    return 1\n";
const M1_USES_JSON: &str = "def g():\n    return lib.util.json.dumps(1)\n";

/// The titles of the code actions of `uri` (line 0) whose edit removes
/// `import json` from `text`: quick fixes, and "fix all" resolved.
fn json_removals(client: &mut Client, uri: &Url, text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for action in code_actions(
        client,
        uri,
        Range::new(Position::new(0, 0), Position::new(0, 11)),
        None,
    ) {
        let action = if action.edit.is_none() && action.data.is_some() {
            client.request::<CodeActionResolveRequest>(action)
        } else {
            action
        };
        let edits = action
            .edit
            .as_ref()
            .and_then(|e| e.changes.as_ref())
            .and_then(|c| c.get(uri))
            .cloned()
            .unwrap_or_default();
        if !apply(text, &edits).contains("import json") {
            found.push(action.title.clone());
        }
    }
    let applied = client.request_raw(
        "workspace/executeCommand",
        json!({"command": "iglint.applyFixAll", "arguments": [uri.as_str()]}),
    );
    if applied == json!(true) {
        found.push("iglint.applyFixAll".to_owned());
    }
    found
}

/// Review round 31 (Opus, B1): VS Code opened on `projects/child`, whose
/// parent `base` lies outside the folder but inside its workspace. An
/// editor change of `base`'s `m1` (now reading `lib.util.json`) reaches
/// the child folder's live workspace, so no fix removes `import json`
/// from `lib/util`; neither after saving it, nor after closing it; and
/// closing an unsaved edit brings the removal back.
#[test]
fn a_parent_project_outside_the_folder_gets_the_editor_text() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (util, m1) = parent_and_child(&root);
    let child = root.join("projects/child");
    let mut client = Client::start(Some(&child), json!({"debounceMs": 0}));
    let util_uri = client.open(&util, UTIL);
    let before = client.diagnostics(&util_uri, Some(1));
    assert!(has(&before, "SCR102", "`json`"), "{before:#?}");
    assert!(!json_removals(&mut client, &util_uri, UTIL).is_empty());
    let m1_uri = client.open(&m1, M1);
    client.diagnostics(&m1_uri, Some(1));
    // Edited, then closed without saving: the disk text again.
    client.replace_all(&m1_uri, 2, M1_USES_JSON);
    client.diagnostics(&m1_uri, Some(2));
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    client.notify(
        DidCloseTextDocument::METHOD,
        json!({"textDocument": {"uri": m1_uri}}),
    );
    assert!(!json_removals(&mut client, &util_uri, UTIL).is_empty());
    // Edited and saved.
    let m1_uri = client.open(&m1, M1);
    client.replace_all(&m1_uri, 2, M1_USES_JSON);
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    std::fs::write(&m1, M1_USES_JSON).unwrap();
    client.notify(
        lsp_types::notification::DidSaveTextDocument::METHOD,
        json!({"textDocument": {"uri": m1_uri}}),
    );
    client.notify(
        DidChangeWatchedFiles::METHOD,
        json!({"changes": [{"uri": m1_uri, "type": 2}]}),
    );
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    client.notify(
        DidCloseTextDocument::METHOD,
        json!({"textDocument": {"uri": m1_uri}}),
    );
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    let after = settled(&mut client, &util_uri);
    assert!(after.is_none_or(|p| !has(&p, "SCR102", "`json`")));
    client.shutdown();
}

/// Review round 31 (Opus, B1, variant a): a change on disk that no
/// notification reports (`git pull`, another editor), of a file that was
/// never opened: of `m1` in the parent project, of a module in the folder
/// itself, a new module, and of the configuration (`SCR102` no longer
/// selected). Before a fix is computed the server compares
/// every file and directory of the workspace with what it read.
#[test]
fn changes_on_disk_without_notifications_reach_fixes() {
    for change in [
        "parent",
        "own",
        "new module",
        "new module, no watchers",
        "configuration",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        let (util, m1) = parent_and_child(&root);
        let child = root.join("projects/child");
        let own = write(
            &root,
            "projects/child/ignition/script-python/lib/b/resource.json",
            r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#,
        )
        .with_file_name("code.py");
        std::fs::write(&own, M1).unwrap();
        let capabilities = if change.ends_with("no watchers") {
            serde_json::from_value(json!({})).unwrap()
        } else {
            client_capabilities()
        };
        let mut client = Client::start_with(Some(&child), json!({"debounceMs": 0}), capabilities);
        let util_uri = client.open(&util, UTIL);
        client.diagnostics(&util_uri, Some(1));
        assert!(!json_removals(&mut client, &util_uri, UTIL).is_empty());
        match change {
            "parent" => std::fs::write(&m1, M1_USES_JSON).unwrap(),
            "own" => std::fs::write(&own, M1_USES_JSON).unwrap(),
            "configuration" => std::fs::write(
                root.join("pyproject.toml"),
                "[tool.iglint]\nselect = [\"REF002\"]\n",
            )
            .unwrap(),
            _ => {
                let manifest = r#"{"scope": "A", "version": 1, "files": ["code.py"]}"#;
                write(
                    &root,
                    "projects/base/ignition/script-python/m3/resource.json",
                    manifest,
                );
                write(
                    &root,
                    "projects/base/ignition/script-python/m3/code.py",
                    M1_USES_JSON,
                );
            }
        }
        assert_eq!(
            json_removals(&mut client, &util_uri, UTIL),
            Vec::<String>::new(),
            "{change}"
        );
        let after = settled(&mut client, &util_uri);
        assert!(
            after.is_none_or(|p| !has(&p, "SCR102", "`json`")),
            "{change}"
        );
        client.shutdown();
    }
}

/// Review round 31 (Opus, B1, variant b, and Codex): nested and
/// overlapping workspace folders. The folder that publishes `lib/util`
/// (the innermost) is not the one that publishes the edited module, and
/// the edit is not saved; with every run trigger. Also a folder on the
/// parent project and one on the child at once.
#[test]
fn every_live_workspace_gets_the_editor_text() {
    // Codex: folders R and R/.../users, `users/b` edited.
    for init in [
        json!({"run": "onSave"}),
        json!({"run": "onType", "debounceMs": 60000}),
        json!({"run": "onType", "debounceMs": 0}),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        write(
            &root,
            "projects/p/project.json",
            r#"{"enabled":true,"inheritable":false}"#,
        );
        let manifest = r#"{"scope":"A","files":["code.py"]}"#;
        let library = "projects/p/ignition/script-python";
        write(
            &root,
            &format!("{library}/lib/util/resource.json"),
            manifest,
        );
        let util = write(&root, &format!("{library}/lib/util/code.py"), UTIL);
        write(&root, &format!("{library}/users/b/resource.json"), manifest);
        let b = write(&root, &format!("{library}/users/b/code.py"), M1);
        let users = root.join(format!("{library}/users"));
        let lib = root.join(format!("{library}/lib"));
        for folders in [vec![&root, &users], vec![&root, &lib]] {
            let folders: Vec<&std::path::Path> = folders.iter().map(|p| p.as_path()).collect();
            let mut client = Client::start_folders(&folders, init.clone(), client_capabilities());
            let util_uri = client.open(&util, UTIL);
            let b_uri = client.open(&b, M1);
            let before = client.diagnostics(&util_uri, Some(1));
            assert!(has(&before, "SCR102", "`json`"), "{before:#?}");
            client.diagnostics(&b_uri, Some(1));
            assert!(!json_removals(&mut client, &util_uri, UTIL).is_empty());
            client.replace_all(&b_uri, 2, M1_USES_JSON);
            assert_eq!(
                json_removals(&mut client, &util_uri, UTIL),
                Vec::<String>::new(),
                "{init} {folders:?}"
            );
            client.notify(
                DidCloseTextDocument::METHOD,
                json!({"textDocument": {"uri": b_uri}}),
            );
            assert!(
                !json_removals(&mut client, &util_uri, UTIL).is_empty(),
                "{init} {folders:?}"
            );
            client.shutdown();
        }
    }
    // Folders on the parent and on the child project.
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (util, m1) = parent_and_child(&root);
    let base = root.join("projects/base");
    let child = root.join("projects/child");
    for folders in [[&base, &child], [&child, &base]] {
        let folders: Vec<&std::path::Path> = folders.iter().map(|p| p.as_path()).collect();
        let mut client =
            Client::start_folders(&folders, json!({"run": "onSave"}), client_capabilities());
        let util_uri = client.open(&util, UTIL);
        client.diagnostics(&util_uri, Some(1));
        let m1_uri = client.open(&m1, M1);
        assert!(!json_removals(&mut client, &util_uri, UTIL).is_empty());
        client.replace_all(&m1_uri, 2, M1_USES_JSON);
        assert_eq!(
            json_removals(&mut client, &util_uri, UTIL),
            Vec::<String>::new(),
            "{folders:?}"
        );
        client.shutdown();
    }
}

/// Review round 31 (Codex): a folder added after the documents were
/// opened gets the editor text of every open document its index holds.
#[test]
fn a_folder_added_later_gets_the_editor_text() {
    let dir = tempfile::tempdir().unwrap();
    let root = root(&dir);
    let (util, m1) = parent_and_child(&root);
    let base = root.join("projects/base");
    let child = root.join("projects/child");
    let mut client = Client::start(Some(&base), json!({"run": "onSave"}));
    let m1_uri = client.open(&m1, M1);
    client.diagnostics(&m1_uri, Some(1));
    client.replace_all(&m1_uri, 2, M1_USES_JSON);
    client.notify(
        lsp_types::notification::DidChangeWorkspaceFolders::METHOD,
        json!({"event": {"added": [{"uri": Url::from_file_path(&child).unwrap(), "name": "child"}], "removed": []}}),
    );
    let util_uri = client.open(&util, UTIL);
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    // Removing the base folder: `m1` is published by another folder, and
    // its editor text still counts.
    client.notify(
        lsp_types::notification::DidChangeWorkspaceFolders::METHOD,
        json!({"event": {"added": [], "removed": [{"uri": Url::from_file_path(&base).unwrap(), "name": "base"}]}}),
    );
    assert_eq!(
        json_removals(&mut client, &util_uri, UTIL),
        Vec::<String>::new()
    );
    client.shutdown();
}

/// Review round 31: the server watches the directories of a workspace
/// outside the folder (here the workspace root, which holds the parent
/// project), relative to their URI when
/// the client supports it, else with absolute patterns.
#[test]
fn watches_workspace_directories_outside_the_folder() {
    for relative in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let root = root(&dir);
        parent_and_child(&root);
        let child = root.join("projects/child");
        let mut capabilities = client_capabilities();
        capabilities
            .workspace
            .as_mut()
            .unwrap()
            .did_change_watched_files
            .as_mut()
            .unwrap()
            .relative_pattern_support = Some(relative);
        let mut client = Client::start_with(Some(&child), json!({}), capabilities);
        // The walk starts at the workspace root, which holds `base`.
        let base = Url::from_file_path(&root).unwrap();
        let registration = client.wait_for(|m| match m {
            Message::Request(r)
                if r.method == RegisterCapability::METHOD
                    && r.params.to_string().contains("outside") =>
            {
                Some(r.params.clone())
            }
            _ => None,
        });
        let watchers = &registration["registrations"][0]["registerOptions"]["watchers"];
        let text = watchers.to_string();
        if relative {
            assert!(text.contains(base.as_str()), "{text}");
        } else {
            let base = iglint_linter::discovery::slash_path(&root);
            assert!(text.contains(&format!("\"{base}/**/*.py\"")), "{text}");
        }
        client.shutdown();
    }
}
