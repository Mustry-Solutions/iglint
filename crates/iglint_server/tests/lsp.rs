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
        Position::new(line_no as u32, utf16(&line[..at]))
    );
    assert_eq!(
        d.range.end,
        Position::new(line_no as u32, utf16(&line[..at]) + 2)
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
    let rejected = fixes(&mut client, line(1));
    assert!(rejected.is_empty(), "{rejected:#?}");
    let unsafe_fix = fixes(&mut client, line(2));
    assert_eq!(unsafe_fix.len(), 1, "{unsafe_fix:#?}");
    assert!(unsafe_fix[0].title.ends_with("(unsafe)"));
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
        .filter(|i| base.is_char_boundary(*i))
        .map(|i| base[..i].to_owned())
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
