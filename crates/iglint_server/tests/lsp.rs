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

/// The VIEW015 quick fixes (and fix-all) of `view` in a workspace where
/// another view runs `script`: the titles and whether each is preferred.
fn view015_fixes(script: &str) -> Vec<(String, Option<bool>)> {
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
    write(&root, &format!("{views}/B/view.json"), &view("{}", script));
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
