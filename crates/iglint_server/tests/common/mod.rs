//! An in-process LSP client driving the server over an in-memory connection.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Exit, Initialized, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Initialize, Request as _, Shutdown};
use lsp_types::{
    ClientCapabilities, DidChangeTextDocumentParams, DidOpenTextDocumentParams, InitializeParams,
    PublishDiagnosticsParams, TextDocumentContentChangeEvent, TextDocumentItem, Url,
    VersionedTextDocumentIdentifier, WorkspaceFolder,
};
use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_secs(60);

pub struct Client {
    conn: Connection,
    server: Option<JoinHandle<anyhow::Result<()>>>,
    next_id: i32,
    /// Notifications and server requests not consumed yet.
    pub inbox: Vec<Message>,
}

impl Client {
    pub fn start(root: Option<&Path>, init: Value) -> Client {
        Client::start_with(root, init, client_capabilities())
    }

    pub fn start_with(
        root: Option<&Path>,
        init: Value,
        capabilities: ClientCapabilities,
    ) -> Client {
        let roots: Vec<&Path> = root.into_iter().collect();
        Client::start_folders(&roots, init, capabilities)
    }

    /// A client with one workspace folder for each of `roots` (none: no
    /// folder).
    pub fn start_folders(roots: &[&Path], init: Value, capabilities: ClientCapabilities) -> Client {
        let root: Option<&Path> = roots.first().copied();
        let (server, client) = Connection::memory();
        let handle = std::thread::spawn(move || iglint_server::run_connection(server));
        let mut c = Client {
            conn: client,
            server: Some(handle),
            next_id: 0,
            inbox: Vec::new(),
        };
        #[allow(deprecated)]
        let params = InitializeParams {
            workspace_folders: root.map(|_| {
                roots
                    .iter()
                    .enumerate()
                    .map(|(i, r)| WorkspaceFolder {
                        uri: Url::from_file_path(r).unwrap(),
                        name: format!("test{i}"),
                    })
                    .collect()
            }),
            initialization_options: Some(init),
            capabilities,
            ..InitializeParams::default()
        };
        let result = c.request_raw(Initialize::METHOD, serde_json::to_value(params).unwrap());
        assert!(result.get("capabilities").is_some(), "{result}");
        c.notify(Initialized::METHOD, json!({}));
        c
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.conn
            .sender
            .send(Notification::new(method.to_owned(), params).into())
            .unwrap();
    }

    pub fn request_raw(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Request::new(id.clone(), method.to_owned(), params).into())
            .unwrap();
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let message = self
                .conn
                .receiver
                .recv_deadline(deadline)
                .unwrap_or_else(|_| panic!("no response to {method}"));
            match message {
                Message::Response(Response {
                    id: rid,
                    result,
                    error,
                }) if rid == id => {
                    if let Some(error) = error {
                        panic!("{method} failed: {error:?}");
                    }
                    return result.unwrap_or(Value::Null);
                }
                other => self.inbox.push(other),
            }
        }
    }

    pub fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> R::Result {
        let value = self.request_raw(R::METHOD, serde_json::to_value(params).unwrap());
        serde_json::from_value(value).unwrap()
    }

    /// Waits for the next message matching `pick`, keeping the others.
    pub fn wait_for<T>(&mut self, mut pick: impl FnMut(&Message) -> Option<T>) -> T {
        if let Some(i) = self.inbox.iter().position(|m| pick(m).is_some()) {
            let m = self.inbox.remove(i);
            return pick(&m).unwrap();
        }
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let message = self
                .conn
                .receiver
                .recv_deadline(deadline)
                .expect("timed out waiting for a message");
            if let Some(found) = pick(&message) {
                return found;
            }
            self.inbox.push(message);
        }
    }

    /// The next diagnostics published for `uri` (with `version`, when given).
    pub fn diagnostics(&mut self, uri: &Url, version: Option<i32>) -> PublishDiagnosticsParams {
        self.wait_for(|m| match m {
            Message::Notification(n) if n.method == PublishDiagnostics::METHOD => {
                let p: PublishDiagnosticsParams = serde_json::from_value(n.params.clone()).ok()?;
                (p.uri == *uri && (version.is_none() || p.version == version)).then_some(p)
            }
            _ => None,
        })
    }

    /// Drops every queued message.
    pub fn drain(&mut self) {
        self.inbox.clear();
        while self.conn.receiver.try_recv().is_ok() {}
    }

    pub fn open(&mut self, path: &Path, text: &str) -> Url {
        let uri = Url::from_file_path(path).unwrap();
        let language = if path.extension().is_some_and(|e| e == "py") {
            "python"
        } else {
            "json"
        };
        self.notify(
            DidOpenTextDocument::METHOD,
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: language.to_owned(),
                    version: 1,
                    text: text.to_owned(),
                },
            })
            .unwrap(),
        );
        uri
    }

    pub fn change(
        &mut self,
        uri: &Url,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
    ) {
        self.notify(
            DidChangeTextDocument::METHOD,
            serde_json::to_value(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: changes,
            })
            .unwrap(),
        );
    }

    pub fn replace_all(&mut self, uri: &Url, version: i32, text: &str) {
        self.change(
            uri,
            version,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        );
    }

    pub fn shutdown(mut self) {
        self.request_raw(Shutdown::METHOD, Value::Null);
        self.notify(Exit::METHOD, Value::Null);
        let handle = self.server.take().unwrap();
        handle.join().unwrap().unwrap();
    }
}

pub fn client_capabilities() -> ClientCapabilities {
    serde_json::from_value(json!({
        "workspace": {
            "didChangeWatchedFiles": {"dynamicRegistration": true},
            "workspaceFolders": true
        },
        "textDocument": {
            "codeAction": {
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "source.fixAll"]}},
                "resolveSupport": {"properties": ["edit"]},
                "dataSupport": true
            },
            "publishDiagnostics": {"relatedInformation": true, "versionSupport": true, "codeDescriptionSupport": true}
        }
    }))
    .unwrap()
}

pub fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

pub const VIEWS: &str = "projects/demo/com.inductiveautomation.perspective/views";

pub const MANIFEST: &str = r#"{"scope": "G", "version": 1, "restricted": false, "overridable": true, "files": ["view.json"], "attributes": {}}"#;

/// A project with views `Popup` (one input parameter) and `Caller` (embeds
/// `Popup`, passing `speed`).
pub fn demo_project(dir: &Path) -> (PathBuf, PathBuf) {
    write(
        dir,
        "projects/demo/project.json",
        r#"{"title": "demo", "enabled": true}"#,
    );
    write(dir, &format!("{VIEWS}/Popup/resource.json"), MANIFEST);
    let popup = write(
        dir,
        &format!("{VIEWS}/Popup/view.json"),
        &popup_view("speed"),
    );
    write(dir, &format!("{VIEWS}/Caller/resource.json"), MANIFEST);
    let caller = write(dir, &format!("{VIEWS}/Caller/view.json"), CALLER);
    (popup, caller)
}

pub fn popup_view(param: &str) -> String {
    format!(
        r#"{{
  "params": {{"{param}": 1}},
  "propConfig": {{"params.{param}": {{"paramDirection": "input"}}}},
  "root": {{
    "type": "ia.container.flex",
    "meta": {{"name": "root"}},
    "children": [
      {{
        "type": "ia.display.label",
        "meta": {{"name": "Label"}},
        "propConfig": {{"props.text": {{"binding": {{"type": "property", "config": {{"path": "view.params.{param}"}}}}}}}}
      }}
    ]
  }}
}}"#
    )
}

pub const CALLER: &str = r#"{
  "root": {
    "type": "ia.container.flex",
    "meta": {"name": "root"},
    "children": [
      {
        "type": "ia.display.view",
        "meta": {"name": "Embedded"},
        "props": {"path": "Popup", "params": {"speed": 2}}
      }
    ]
  }
}"#;

pub fn codes(p: &PublishDiagnosticsParams) -> Vec<String> {
    let mut codes: Vec<String> = p
        .diagnostics
        .iter()
        .filter_map(|d| match &d.code {
            Some(lsp_types::NumberOrString::String(s)) => Some(s.clone()),
            _ => None,
        })
        .collect();
    codes.sort();
    codes
}

/// Applies LSP text edits (from one file) to `text`.
pub fn apply(text: &str, edits: &[lsp_types::TextEdit]) -> String {
    let index = iglint_text::LineIndex::new(text);
    let mut edits: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let at = |p: lsp_types::Position| {
                index
                    .offset_of_utf16(
                        iglint_text::Utf16Position {
                            line: p.line,
                            character: p.character,
                        },
                        text,
                    )
                    .to_usize()
            };
            (at(e.range.start), at(e.range.end), e.new_text.as_str())
        })
        .collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_owned();
    for (start, end, new) in edits {
        out.replace_range(start..end, new);
    }
    out
}
