//! The main loop: documents, folders, debounced analysis and publishing.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use iglint_linter::Diagnostic;
use iglint_linter::live::Dependents;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles,
    DidChangeWorkspaceFolders, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    LogMessage, Notification as _, PublishDiagnostics, ShowMessage,
};
use lsp_types::request::{
    CodeActionRequest, CodeActionResolveRequest, ExecuteCommand, HoverRequest, RegisterCapability,
    Request as _, Shutdown,
};
use lsp_types::{
    ClientCapabilities, CodeActionKind, CodeActionOptions, CodeActionProviderCapability,
    DidChangeWatchedFilesRegistrationOptions, ExecuteCommandOptions, FileChangeType,
    FileSystemWatcher, GlobPattern, HoverProviderCapability, InitializeParams, InitializeResult,
    LogMessageParams, MessageType, PublishDiagnosticsParams, Registration, RegistrationParams,
    SaveOptions, ServerCapabilities, ServerInfo, ShowMessageParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, Url,
    WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
};
use rustc_hash::FxHashMap;
use serde_json::Value;

use crate::convert::{self, path_of, uri_of};
use crate::folder::{CONFIG_NAMES, Folder};
use crate::options::{RunTrigger, Scope, ServerOptions};

/// `workspace/executeCommand`: apply every fix of a document. Argument: the
/// document URI, as a string or `{"uri": ...}`.
pub const COMMAND_FIX_ALL: &str = "iglint.applyFixAll";
/// `workspace/executeCommand`: the Markdown documentation of a rule.
/// Argument: a rule code or name.
pub const COMMAND_RULE_DOCS: &str = "iglint.ruleDocs";
/// Code action kind of "fix all".
pub const FIX_ALL_KIND: &str = "source.fixAll.iglint";

pub(crate) struct Document {
    pub path: PathBuf,
    pub version: i32,
    pub text: String,
    /// Changed since the last analysis (findings to publish, per the run
    /// trigger).
    pub dirty: bool,
    /// Identifies `text`: a new number for every change. A live workspace
    /// has the editor's text when its folder's `applied` holds this
    /// number (review round 30, B2, and round 31, B1: every fix is computed
    /// from the editor's text of every open document, whatever the run
    /// trigger, in every live workspace whose index holds the document).
    pub generation: u64,
}

/// What was last published for a file.
pub(crate) struct Published {
    pub diagnostics: Vec<Diagnostic>,
    pub lsp: Vec<lsp_types::Diagnostic>,
}

pub(crate) struct Server {
    sender: Sender<Message>,
    receiver: Receiver<Message>,
    pub options: ServerOptions,
    pub folders: Vec<Folder>,
    pub documents: FxHashMap<Url, Document>,
    /// Open documents iglint does not lint (configuration files, a
    /// whitelist): their path and editor text. A fix is not computed while
    /// one the check reads differs from the disk (review round 32).
    pub other_documents: FxHashMap<Url, (PathBuf, String)>,
    /// Counts the notifications received: fresh results are reused only
    /// between two notifications ([`crate::fresh::BurstCache`]).
    pub epoch: u64,
    pub burst: crate::fresh::BurstCache,
    pub results: FxHashMap<PathBuf, Published>,
    deadline: Option<Instant>,
    pub resolve_support: bool,
    watch_registration: bool,
    /// The client takes watchers relative to a base URI.
    relative_patterns: bool,
    /// The directories outside every workspace folder that are watched
    /// (the id of the registration and the directories).
    watched: Option<(String, Vec<PathBuf>)>,
    next_generation: u64,
    next_id: i32,
    shutdown: bool,
}

impl Server {
    pub fn start(connection: Connection) -> anyhow::Result<()> {
        let (id, params) = connection.initialize_start()?;
        let params: InitializeParams = serde_json::from_value(params).unwrap_or_default();
        let mut options = ServerOptions::default();
        if let Some(init) = &params.initialization_options {
            options = options.updated(init);
        }
        let result = InitializeResult {
            capabilities: capabilities(),
            server_info: Some(ServerInfo {
                name: "iglint".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        };
        connection.initialize_finish(id, serde_json::to_value(result)?)?;

        let mut server = Server {
            sender: connection.sender.clone(),
            receiver: connection.receiver.clone(),
            options,
            folders: Vec::new(),
            documents: FxHashMap::default(),
            other_documents: FxHashMap::default(),
            epoch: 0,
            burst: crate::fresh::BurstCache::default(),
            results: FxHashMap::default(),
            deadline: None,
            resolve_support: resolve_support(&params.capabilities),
            watch_registration: params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.did_change_watched_files.as_ref())
                .and_then(|w| w.dynamic_registration)
                .unwrap_or(false),
            relative_patterns: params
                .capabilities
                .workspace
                .as_ref()
                .and_then(|w| w.did_change_watched_files.as_ref())
                .and_then(|w| w.relative_pattern_support)
                .unwrap_or(false),
            watched: None,
            next_generation: 0,
            next_id: 0,
            shutdown: false,
        };
        server.initialized(&params);
        server.main_loop();
        Ok(())
    }

    fn initialized(&mut self, params: &InitializeParams) {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|f| path_of(&f.uri))
            .collect();
        #[allow(deprecated)]
        if roots.is_empty()
            && let Some(root) = params.root_uri.as_ref().and_then(path_of)
        {
            roots.push(root);
        }
        let start = Instant::now();
        for root in roots {
            self.add_folder(root, false);
        }
        if !self.folders.is_empty() {
            let resources: usize = self
                .folders
                .iter()
                .map(|f| f.live.workspace().resources.len())
                .sum();
            self.log(
                MessageType::INFO,
                format!(
                    "iglint: loaded {} folder(s), {resources} resource(s) in {} ms",
                    self.folders.len(),
                    start.elapsed().as_millis()
                ),
            );
        }
        if self.watch_registration {
            self.register_watchers();
            self.update_watchers();
        }
        if self.options.scope == Scope::Workspace {
            self.publish_all();
        }
    }

    fn main_loop(&mut self) {
        loop {
            let message = match self.deadline {
                Some(deadline) => match self.receiver.recv_deadline(deadline) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match self.receiver.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            let Some(message) = message else {
                self.deadline = None;
                self.guarded("analysis", |s| s.flush());
                continue;
            };
            match message {
                Message::Request(request) => {
                    let id = request.id.clone();
                    let method = request.method.clone();
                    let outcome = catch_unwind(AssertUnwindSafe(|| self.handle_request(request)));
                    let response = match outcome {
                        Ok(response) => response,
                        Err(panic) => {
                            let message = panic_message(&panic);
                            self.log(
                                MessageType::ERROR,
                                format!("iglint: {method} failed: {message}"),
                            );
                            Response::new_err(id, ErrorCode::InternalError as i32, message)
                        }
                    };
                    self.send(response.into());
                }
                Message::Notification(notification) => {
                    if notification.method == lsp_types::notification::Exit::METHOD {
                        return;
                    }
                    let method = notification.method.clone();
                    self.guarded(&method, |s| s.handle_notification(notification));
                }
                Message::Response(_) => {}
            }
        }
    }

    /// Runs `f`, logging instead of crashing when it panics.
    fn guarded(&mut self, what: &str, f: impl FnOnce(&mut Server)) {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| f(self))) {
            let message = panic_message(&panic);
            self.log(
                MessageType::ERROR,
                format!("iglint: {what} failed: {message}"),
            );
        }
    }

    // ---- messages ----

    fn handle_request(&mut self, request: Request) -> Response {
        let id = request.id.clone();
        if self.shutdown && request.method != Shutdown::METHOD {
            return Response::new_err(
                id,
                ErrorCode::InvalidRequest as i32,
                "the server is shutting down".to_owned(),
            );
        }
        match request.method.as_str() {
            Shutdown::METHOD => {
                self.shutdown = true;
                Response::new_ok(id, ())
            }
            CodeActionRequest::METHOD => respond(id, request.params, |p| self.code_actions(p)),
            CodeActionResolveRequest::METHOD => {
                respond(id, request.params, |p| self.resolve_code_action(p))
            }
            HoverRequest::METHOD => respond(id, request.params, |p| self.hover(p)),
            ExecuteCommand::METHOD => {
                // A command (fix all, rule docs) comes after every file
                // left for later is checked and published.
                self.finish_pending();
                respond(id, request.params, |p| self.execute_command(p))
            }
            method => Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request {method}"),
            ),
        }
    }

    fn handle_notification(&mut self, n: Notification) {
        self.epoch += 1;
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidOpenTextDocumentParams>(n.params) {
                    self.did_open(p);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidChangeTextDocumentParams>(n.params) {
                    self.did_change(p);
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidSaveTextDocumentParams>(n.params) {
                    self.did_save(&p.text_document.uri);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidCloseTextDocumentParams>(n.params) {
                    self.did_close(&p.text_document.uri);
                }
            }
            DidChangeConfiguration::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidChangeConfigurationParams>(n.params) {
                    self.did_change_configuration(&p.settings);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidChangeWatchedFilesParams>(n.params) {
                    self.did_change_watched_files(p);
                }
            }
            DidChangeWorkspaceFolders::METHOD => {
                if let Ok(p) = parse::<lsp_types::DidChangeWorkspaceFoldersParams>(n.params) {
                    self.did_change_workspace_folders(p);
                }
            }
            _ => {}
        }
    }

    fn did_open(&mut self, p: lsp_types::DidOpenTextDocumentParams) {
        let uri = p.text_document.uri;
        let Some(path) = path_of(&uri) else {
            return;
        };
        if !lintable(&path) {
            self.other_documents
                .insert(uri, (path, p.text_document.text));
            return;
        }
        let generation = self.generation();
        self.documents.insert(
            uri,
            Document {
                path,
                version: p.text_document.version,
                text: p.text_document.text,
                dirty: true,
                generation,
            },
        );
        self.schedule(Duration::ZERO);
    }

    fn did_change(&mut self, p: lsp_types::DidChangeTextDocumentParams) {
        let run = self.options.run;
        let generation = self.generation();
        if let Some((_, text)) = self.other_documents.get_mut(&p.text_document.uri) {
            for change in &p.content_changes {
                convert::apply_change(text, change);
            }
            return;
        }
        let Some(doc) = self.documents.get_mut(&p.text_document.uri) else {
            return;
        };
        for change in &p.content_changes {
            convert::apply_change(&mut doc.text, change);
        }
        doc.version = p.text_document.version;
        doc.generation = generation;
        if run == RunTrigger::OnType {
            doc.dirty = true;
            let debounce = Duration::from_millis(self.options.debounce_ms);
            self.schedule(debounce);
        }
    }

    fn did_save(&mut self, uri: &Url) {
        let Some(path) = self.documents.get(uri).map(|d| d.path.clone()) else {
            return;
        };
        let folder = self.folder_for(&path);
        if self.folders[folder].live.is_manifest(&path) {
            self.reload_folder(folder);
            return;
        }
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.dirty = true;
        }
        self.schedule(Duration::ZERO);
    }

    fn did_close(&mut self, uri: &Url) {
        self.other_documents.remove(uri);
        let Some(doc) = self.documents.remove(uri) else {
            return;
        };
        let folder = self.folder_for(&doc.path);
        // Every live workspace that had the editor's text goes back to
        // the disk's.
        for i in 0..self.folders.len() {
            let mut check = self.reconcile(i);
            if i == folder {
                if self.is_published(folder, &doc.path) {
                    check.insert(doc.path.clone());
                } else {
                    self.clear(&doc.path);
                }
            }
            self.check_and_publish(i, check);
        }
    }

    fn did_change_configuration(&mut self, settings: &Value) {
        let old = self.options.clone();
        self.options = self.options.updated(settings);
        if old == self.options {
            return;
        }
        self.log(MessageType::INFO, "iglint: settings changed".to_owned());
        if old.scope != self.options.scope || old.rule_docs_url != self.options.rule_docs_url {
            let published: Vec<PathBuf> = self.results.keys().cloned().collect();
            for path in published {
                self.clear(&path);
            }
            self.publish_all();
        }
    }

    fn did_change_watched_files(&mut self, p: lsp_types::DidChangeWatchedFilesParams) {
        let mut reload: BTreeSet<usize> = BTreeSet::new();
        let mut reread: Vec<PathBuf> = Vec::new();
        for change in p.changes {
            let Some(path) = path_of(&change.uri) else {
                continue;
            };
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if CONFIG_NAMES.contains(&name)
                || name == iglint_linter::baseline::DEFAULT_BASELINE_FILE
            {
                for (i, f) in self.folders.iter().enumerate() {
                    if f.config_affected_by(&path) || path.starts_with(&f.root) {
                        reload.insert(i);
                    }
                }
                continue;
            }
            if !lintable(&path) {
                continue;
            }
            // Every folder whose workspace the file is, or may be, part
            // of (a parent project outside the folder, review round 31).
            let owners: Vec<usize> = (0..self.folders.len())
                .filter(|i| {
                    self.folders[*i].surrounds(&path) || self.folders[*i].live.may_index(&path)
                })
                .collect();
            let structural = change.typ != FileChangeType::CHANGED
                || owners
                    .iter()
                    .any(|i| self.folders[*i].live.is_manifest(&path));
            if structural {
                reload.extend(owners);
            } else if !self.documents.values().any(|d| d.path == path) {
                reread.push(path);
            }
        }
        for i in &reload {
            self.reload_folder(*i);
        }
        let mut checks: FxHashMap<usize, BTreeSet<PathBuf>> = FxHashMap::default();
        for path in reread {
            for i in 0..self.folders.len() {
                if reload.contains(&i)
                    || !(self.folders[i].owns(&path) || self.folders[i].live.indexes(&path))
                {
                    continue;
                }
                let dependents = self.folders[i].live.set_text(&path, None);
                let check = checks.entry(i).or_default();
                self.add_dependents(i, dependents, check);
                if self.is_published(i, &path) {
                    check.insert(path.clone());
                }
            }
        }
        for (i, paths) in checks {
            self.check_and_publish(i, paths);
        }
    }

    fn did_change_workspace_folders(&mut self, p: lsp_types::DidChangeWorkspaceFoldersParams) {
        for removed in &p.event.removed {
            if let Some(root) = path_of(&removed.uri) {
                let gone: Vec<PathBuf> = self
                    .results
                    .keys()
                    .filter(|path| path.starts_with(&root))
                    .cloned()
                    .collect();
                for path in gone {
                    self.clear(&path);
                }
                self.folders.retain(|f| f.root != root);
            }
        }
        if !p.event.removed.is_empty() {
            // Documents of a removed folder are published by another one.
            for doc in self.documents.values_mut() {
                doc.dirty = true;
            }
            self.schedule(Duration::ZERO);
        }
        for added in &p.event.added {
            if let Some(root) = path_of(&added.uri) {
                let i = self.add_folder(root, false);
                let paths: BTreeSet<PathBuf> = self.published_paths(i).into_iter().collect();
                self.check_and_publish(i, paths);
            }
        }
        self.update_watchers();
    }

    // ---- folders ----

    fn add_folder(&mut self, root: PathBuf, adhoc: bool) -> usize {
        let mut folder = Folder::load(&root);
        folder.adhoc = adhoc;
        let notices = std::mem::take(&mut folder.notices);
        self.folders.push(folder);
        for notice in notices {
            self.show(MessageType::WARNING, notice);
        }
        let i = self.folders.len() - 1;
        // The new live workspace gets the editor text of every open
        // document its index holds.
        self.reapply_documents(i);
        if adhoc {
            self.update_watchers();
        }
        i
    }

    /// The folder that publishes a file, if there is one: the innermost
    /// workspace folder that contains it, else a folder made for it.
    fn owner_of(&self, path: &Path) -> Option<usize> {
        self.folders
            .iter()
            .enumerate()
            .filter(|(_, f)| f.owns(path))
            .max_by_key(|(_, f)| (!f.adhoc, f.root.components().count()))
            .map(|(i, _)| i)
    }

    /// The folder a file belongs to: the innermost workspace folder that
    /// contains it, or a folder made for it.
    pub fn folder_for(&mut self, path: &Path) -> usize {
        let best = self.owner_of(path);
        match best {
            Some(i) => i,
            None => {
                let root = adhoc_root(path);
                self.log(
                    MessageType::INFO,
                    format!("iglint: linting {} on its own", root.display()),
                );
                self.add_folder(root, true)
            }
        }
    }

    /// Discovers the folder again (configuration or files changed) and
    /// re-publishes its files.
    pub(crate) fn reload_folder(&mut self, i: usize) {
        let start = Instant::now();
        let root = self.folders[i].root.clone();
        let adhoc = self.folders[i].adhoc;
        let mut folder = Folder::load(&root);
        folder.adhoc = adhoc;
        for notice in std::mem::take(&mut folder.notices) {
            self.show(MessageType::WARNING, notice);
        }
        self.folders[i] = folder;
        self.reapply_documents(i);
        self.update_watchers();
        let paths: BTreeSet<PathBuf> = self.published_paths(i).into_iter().collect();
        let stale: Vec<PathBuf> = self
            .results
            .keys()
            .filter(|p| !paths.contains(*p) && self.folders[i].owns(p))
            .cloned()
            .collect();
        for path in stale {
            self.clear(&path);
        }
        self.log(
            MessageType::INFO,
            format!(
                "iglint: reloaded {} in {} ms",
                root.display(),
                start.elapsed().as_millis()
            ),
        );
        self.check_and_publish(i, paths);
    }

    /// Gives a (new) live workspace the editor text of the open documents
    /// its index holds or it publishes. The documents it publishes are
    /// published with the rest of the folder.
    fn reapply_documents(&mut self, i: usize) {
        self.reconcile(i);
        let owned: Vec<Url> = self
            .documents
            .iter()
            .filter(|(_, d)| self.owner_of(&d.path) == Some(i))
            .map(|(u, _)| u.clone())
            .collect();
        for uri in owned {
            if let Some(doc) = self.documents.get_mut(&uri) {
                doc.dirty = false;
            }
        }
    }

    /// Whether the live workspace of folder `i` must have the editor text
    /// of `path`: its index holds the file, or the folder publishes it.
    fn holds(&self, i: usize, path: &Path) -> bool {
        self.folders[i].live.indexes(path) || self.owner_of(path) == Some(i)
    }

    /// Brings the editor texts of the live workspace of folder `i` up to
    /// date with the open documents (review round 31, B1): the text of
    /// every open document it holds, the disk text again for documents
    /// closed since. Returns the published files whose findings may have
    /// changed.
    fn reconcile(&mut self, i: usize) -> BTreeSet<PathBuf> {
        let mut out = BTreeSet::new();
        let open: Vec<(PathBuf, u64)> = self
            .documents
            .values()
            .map(|d| (d.path.clone(), d.generation))
            .collect();
        let mut stale: Vec<(PathBuf, Option<u64>)> = Vec::new();
        for (path, generation) in &open {
            if self.folders[i].applied.get(path) != Some(generation) && self.holds(i, path) {
                stale.push((path.clone(), Some(*generation)));
            }
        }
        for path in self.folders[i].applied.keys() {
            if !open.iter().any(|(p, _)| p == path) {
                stale.push((path.clone(), None));
            }
        }
        for (path, generation) in stale {
            let text: Option<Arc<str>> = generation.and_then(|g| {
                self.documents
                    .values()
                    .find(|d| d.path == path && d.generation == g)
                    .map(|d| Arc::from(d.text.as_str()))
            });
            let dependents = self.folders[i].live.set_text(&path, text);
            match generation {
                Some(g) => {
                    self.folders[i].applied.insert(path.clone(), g);
                }
                None => {
                    self.folders[i].applied.remove(&path);
                    if self.is_published(i, &path) {
                        out.insert(path.clone());
                    }
                }
            }
            self.add_dependents(i, dependents, &mut out);
        }
        out
    }

    /// Before a fix is computed by folder `i`: anything that changed on
    /// disk without a notification (review round 31, B1) is read again,
    /// and when what discovery found may have changed, or the
    /// configuration did, the folder is discovered again.
    pub fn refresh_disk(&mut self, i: usize) {
        if self.folders[i].config_changed() {
            self.reload_folder(i);
            return;
        }
        let start = Instant::now();
        match self.folders[i].live.refresh_from_disk() {
            iglint_linter::live::DiskChange::Unchanged => {}
            iglint_linter::live::DiskChange::Files {
                changed,
                dependents,
            } => {
                self.log(
                    MessageType::LOG,
                    format!(
                        "iglint: {} file(s) changed on disk, read again in {:.1} ms",
                        changed.len(),
                        start.elapsed().as_secs_f64() * 1000.0
                    ),
                );
                for path in changed {
                    if self.is_published(i, &path) {
                        self.folders[i].pending.push(path);
                    }
                }
                self.add_pending(i, dependents);
                self.schedule(Duration::ZERO);
            }
            iglint_linter::live::DiskChange::Structure(path) => {
                self.log(
                    MessageType::INFO,
                    format!("iglint: {} changed on disk", path.display()),
                );
                self.reload_folder(i);
            }
        }
    }

    /// Files of folder `i` that get diagnostics.
    fn published_paths(&mut self, i: usize) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = Vec::new();
        if self.options.scope == Scope::Workspace && !self.folders[i].adhoc {
            paths.extend(self.folders[i].workspace_files().iter().cloned());
        }
        let open: Vec<PathBuf> = self.documents.values().map(|d| d.path.clone()).collect();
        for path in open {
            if self.folder_for(&path) == i {
                paths.push(path);
            }
        }
        paths.sort();
        paths.dedup();
        paths
    }

    fn is_published(&mut self, i: usize, path: &Path) -> bool {
        self.documents.values().any(|d| d.path == path)
            || (self.options.scope == Scope::Workspace
                && !self.folders[i].adhoc
                && self.folders[i]
                    .workspace_files()
                    .binary_search_by(|p| p.as_path().cmp(path))
                    .is_ok())
    }

    fn add_dependents(&mut self, i: usize, dependents: Dependents, out: &mut BTreeSet<PathBuf>) {
        match dependents {
            Dependents::None => {}
            Dependents::Files(files) => {
                for f in files {
                    if self.is_published(i, &f) {
                        out.insert(f);
                    }
                }
            }
            Dependents::All => out.extend(self.published_paths(i)),
        }
    }

    // ---- analysis ----

    fn schedule(&mut self, delay: Duration) {
        let at = Instant::now() + delay;
        self.deadline = Some(match self.deadline {
            Some(d) if delay.is_zero() => d.min(at),
            _ => at,
        });
    }

    /// Analyses every changed document, publishes it, then checks again
    /// what depends on it (and what is pending), stopping when a message
    /// arrives (review round 30, Opus minor 1: the rest is checked once
    /// the messages are handled, or the next change's run comes first).
    pub fn flush(&mut self) {
        let dirty: Vec<Url> = self
            .documents
            .iter()
            .filter(|(_, d)| d.dirty)
            .map(|(u, _)| u.clone())
            .collect();
        if dirty.is_empty() && !self.folders.iter().any(Folder::has_pending) {
            return;
        }
        // The edited documents first, then what depends on them.
        for uri in dirty {
            let Some(doc) = self.documents.get_mut(&uri) else {
                continue;
            };
            doc.dirty = false;
            let generation = doc.generation;
            let path = doc.path.clone();
            let text: Arc<str> = Arc::from(doc.text.as_str());
            let i = self.folder_for(&path);
            let start = Instant::now();
            let (result, dependents) = if self.folders[i].applied.get(&path) == Some(&generation) {
                let result = self.folders[i]
                    .live
                    .check(std::slice::from_ref(&path))
                    .remove(0);
                (result, Dependents::None)
            } else {
                self.folders[i].applied.insert(path.clone(), generation);
                self.folders[i].live.update(&path, text)
            };
            let elapsed = start.elapsed();
            self.publish_result(i, result);
            self.log(
                MessageType::LOG,
                format!(
                    "iglint: checked {} in {:.1} ms",
                    path.display(),
                    elapsed.as_secs_f64() * 1000.0
                ),
            );
            self.add_pending(i, dependents);
        }
        // The other live workspaces whose index holds an edited document.
        self.sync_documents();
        if self.deadline.is_some_and(|d| d <= Instant::now()) {
            self.deadline = None;
        }
        for i in 0..self.folders.len() {
            if self.folders[i].has_pending() {
                self.check_pending(i, true);
            }
        }
    }

    /// Analyses `uri` now if it changed since its last analysis, or if
    /// its findings may have changed and it waits to be checked again.
    pub fn flush_document(&mut self, uri: &Url) {
        if self.documents.get(uri).is_some_and(|d| d.dirty) {
            self.flush();
        }
        let Some(path) = path_of(uri) else {
            return;
        };
        let i = self.folder_for(&path);
        let folder = &self.folders[i];
        if (folder.pending_all && self.documents.contains_key(uri))
            || folder.pending.contains(&path)
        {
            self.check_paths(i, vec![path], false);
        }
    }

    /// Gives the live workspaces the editor text of every open document
    /// that changed since they last got it, whatever the run trigger and
    /// its debounce (review round 30, B2): a fix is computed by a fresh
    /// check, and that check must see what the editor has. Findings are
    /// published per the run trigger; files whose findings may have
    /// changed wait in [`Folder::pending`](crate::folder::Folder).
    pub fn sync_documents(&mut self) {
        // Every open document has a folder that publishes it.
        let paths: Vec<PathBuf> = self.documents.values().map(|d| d.path.clone()).collect();
        for path in paths {
            self.folder_for(&path);
        }
        for i in 0..self.folders.len() {
            let changed = self.reconcile(i);
            self.folders[i].pending.extend(changed);
        }
    }

    fn generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Checks and publishes every published file.
    fn publish_all(&mut self) {
        for i in 0..self.folders.len() {
            self.folders[i].pending_all = true;
            self.check_pending(i, true);
        }
    }

    /// Checks everything pending now, without stopping for messages.
    fn finish_pending(&mut self) {
        for i in 0..self.folders.len() {
            if self.folders[i].has_pending() {
                self.check_pending(i, false);
            }
        }
    }

    /// Adds the published files among `dependents` to those pending.
    fn add_pending(&mut self, i: usize, dependents: Dependents) {
        match dependents {
            Dependents::None => {}
            Dependents::Files(files) => {
                for f in files {
                    if self.is_published(i, &f) {
                        self.folders[i].pending.push(f);
                    }
                }
            }
            Dependents::All => self.folders[i].pending_all = true,
        }
    }

    /// Checks and publishes what is pending in folder `i`.
    fn check_pending(&mut self, i: usize, interruptible: bool) {
        let mut paths = if std::mem::take(&mut self.folders[i].pending_all) {
            self.folders[i].pending.clear();
            self.published_paths(i)
        } else {
            std::mem::take(&mut self.folders[i].pending)
        };
        // Usually sorted already (what a check left).
        paths.sort();
        paths.dedup();
        self.check_paths(i, paths, interruptible);
    }

    /// Checks and publishes `paths`, stopping when a message arrives: what
    /// is left becomes pending, checked on the next run.
    fn check_and_publish(&mut self, i: usize, paths: BTreeSet<PathBuf>) {
        self.check_paths(i, paths.into_iter().collect(), true);
    }

    /// Checks and publishes `paths` (sorted), when `interruptible` stopping
    /// when a message arrives: the rest is pending.
    fn check_paths(&mut self, i: usize, paths: Vec<PathBuf>, interruptible: bool) {
        if paths.is_empty() {
            return;
        }
        let start = Instant::now();
        let receiver = self.receiver.clone();
        let stop = move || interruptible && !receiver.is_empty();
        let (results, rest) = self.folders[i].live.check_until(&paths, &stop);
        let elapsed = start.elapsed();
        let count = results.len();
        for result in results {
            self.publish_result(i, result);
        }
        self.log(
            MessageType::LOG,
            format!(
                "iglint: checked {count} file(s) in {:.1} ms{}",
                elapsed.as_secs_f64() * 1000.0,
                if rest.is_empty() {
                    String::new()
                } else {
                    format!(", {} left for later", rest.len())
                }
            ),
        );
        if !rest.is_empty() {
            self.folders[i].pending.extend(rest);
            self.schedule(Duration::ZERO);
        }
    }

    pub(crate) fn publish_result(
        &mut self,
        i: usize,
        result: iglint_linter::live::FileDiagnostics,
    ) {
        if let Some(error) = &result.error {
            self.log(
                MessageType::WARNING,
                format!("iglint: {}: {error}", result.path.display()),
            );
        }
        let diagnostics = self.folders[i].filter_baseline(result.diagnostics);
        self.publish(&result.path, diagnostics);
    }

    pub(crate) fn publish(&mut self, path: &Path, diagnostics: Vec<Diagnostic>) {
        // An open document keeps the client's own URI: on Windows it may
        // spell the drive differently (`file:///c%3A/...`) from ours.
        let open = self
            .documents
            .iter()
            .find(|(_, d)| d.path == path)
            .map(|(uri, d)| (uri.clone(), d.version));
        let (uri, version) = match open {
            Some((uri, version)) => (uri, Some(version)),
            None => match uri_of(path) {
                Some(uri) => (uri, None),
                None => return,
            },
        };
        let lsp: Vec<lsp_types::Diagnostic> = diagnostics
            .iter()
            .map(|d| convert::diagnostic(d, &self.options))
            .collect();
        self.notify::<PublishDiagnostics>(PublishDiagnosticsParams {
            uri,
            diagnostics: lsp.clone(),
            version,
        });
        self.results
            .insert(path.to_path_buf(), Published { diagnostics, lsp });
    }

    fn clear(&mut self, path: &Path) {
        if self.results.remove(path).is_some()
            && let Some(uri) = uri_of(path)
        {
            self.notify::<PublishDiagnostics>(PublishDiagnosticsParams {
                uri,
                diagnostics: Vec::new(),
                version: None,
            });
        }
    }

    // ---- client communication ----

    fn send(&self, message: Message) {
        // The client went away: the main loop ends on the next receive.
        let _ = self.sender.send(message);
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.send(Notification::new(N::METHOD.to_owned(), params).into());
    }

    pub fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) {
        self.next_id += 1;
        let id = RequestId::from(format!("iglint-{}", self.next_id));
        self.send(Request::new(id, R::METHOD.to_owned(), params).into());
    }

    pub fn log(&self, typ: MessageType, message: String) {
        eprintln!("{message}");
        self.notify::<LogMessage>(LogMessageParams { typ, message });
    }

    fn show(&self, typ: MessageType, message: String) {
        self.log(typ, message.clone());
        self.notify::<ShowMessage>(ShowMessageParams { typ, message });
    }

    /// Watches the directories of every live workspace outside the
    /// workspace folders (a parent project next to the folder's project,
    /// the rest of a gateway; review round 31, B1), relative to their URI
    /// when the client can, else with absolute glob patterns.
    /// Notifications can still be missing or late; fixes do not depend on
    /// them ([`Server::refresh_disk`]).
    fn update_watchers(&mut self) {
        if !self.watch_registration {
            return;
        }
        let roots: Vec<PathBuf> = self
            .folders
            .iter()
            .filter(|f| !f.adhoc)
            .map(|f| f.root.clone())
            .collect();
        let mut dirs: Vec<PathBuf> = self
            .folders
            .iter()
            .flat_map(|f| f.live.watched_directories())
            .filter(|d| !roots.iter().any(|r| d.starts_with(r)))
            .collect();
        dirs.sort();
        dirs.dedup();
        let mut outer: Vec<PathBuf> = Vec::new();
        for d in dirs {
            if !outer.last().is_some_and(|o| d.starts_with(o)) {
                outer.push(d);
            }
        }
        if self.watched.as_ref().map(|(_, d)| d) == Some(&outer)
            || (self.watched.is_none() && outer.is_empty())
        {
            return;
        }
        if let Some((id, _)) = self.watched.take() {
            self.request::<lsp_types::request::UnregisterCapability>(
                lsp_types::UnregistrationParams {
                    unregisterations: vec![lsp_types::Unregistration {
                        id,
                        method: DidChangeWatchedFiles::METHOD.to_owned(),
                    }],
                },
            );
        }
        let mut watchers = Vec::new();
        for dir in &outer {
            for glob in ["**/*.json", "**/*.py", "**/*.toml"] {
                let pattern = if self.relative_patterns {
                    let Some(uri) = uri_of(dir) else { continue };
                    GlobPattern::Relative(lsp_types::RelativePattern {
                        base_uri: lsp_types::OneOf::Right(uri),
                        pattern: glob.to_owned(),
                    })
                } else {
                    let base = iglint_linter::discovery::slash_path(dir);
                    // A directory whose name a glob would read as a
                    // pattern is not watched (fixes do not need it).
                    if base.contains(['*', '?', '[', ']', '{', '}']) {
                        continue;
                    }
                    GlobPattern::String(format!("{}/{glob}", base.trim_end_matches('/')))
                };
                watchers.push(FileSystemWatcher {
                    glob_pattern: pattern,
                    kind: None,
                });
            }
        }
        self.next_id += 1;
        let id = format!("iglint-watched-outside-{}", self.next_id);
        let options = DidChangeWatchedFilesRegistrationOptions { watchers };
        self.request::<RegisterCapability>(RegistrationParams {
            registrations: vec![Registration {
                id: id.clone(),
                method: DidChangeWatchedFiles::METHOD.to_owned(),
                register_options: serde_json::to_value(options).ok(),
            }],
        });
        self.watched = Some((id, outer));
    }

    fn register_watchers(&mut self) {
        let watchers = ["**/*.json", "**/*.py", "**/*.toml"]
            .into_iter()
            .map(|glob| FileSystemWatcher {
                glob_pattern: GlobPattern::String(glob.to_owned()),
                kind: None,
            })
            .collect();
        let options = DidChangeWatchedFilesRegistrationOptions { watchers };
        self.request::<RegisterCapability>(RegistrationParams {
            registrations: vec![Registration {
                id: "iglint-watched-files".to_owned(),
                method: DidChangeWatchedFiles::METHOD.to_owned(),
                register_options: serde_json::to_value(options).ok(),
            }],
        });
    }
}

fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: None,
                will_save_wait_until: None,
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
            },
        )),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_FIX_ALL,
                CodeActionKind::new(FIX_ALL_KIND),
            ]),
            work_done_progress_options: Default::default(),
            resolve_provider: Some(true),
        })),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![COMMAND_FIX_ALL.to_owned(), COMMAND_RULE_DOCS.to_owned()],
            work_done_progress_options: Default::default(),
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(lsp_types::OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..ServerCapabilities::default()
    }
}

fn resolve_support(capabilities: &ClientCapabilities) -> bool {
    capabilities
        .text_document
        .as_ref()
        .and_then(|t| t.code_action.as_ref())
        .and_then(|c| c.resolve_support.as_ref())
        .is_some_and(|r| r.properties.iter().any(|p| p == "edit"))
}

/// Whether iglint lints files like this one.
fn lintable(path: &Path) -> bool {
    iglint_linter::FileKind::classify(path).is_some()
}

/// Root for a file outside every workspace folder: its project or gateway
/// collection, else its directory.
fn adhoc_root(path: &Path) -> PathBuf {
    path.ancestors()
        .skip(1)
        .find(|dir| dir.join("project.json").is_file() || dir.join("config-mode.json").is_file())
        .or_else(|| path.parent())
        .unwrap_or(path)
        .to_path_buf()
}

fn parse<P: serde::de::DeserializeOwned>(params: Value) -> Result<P, serde_json::Error> {
    serde_json::from_value(params)
}

fn respond<P: serde::de::DeserializeOwned, R: serde::Serialize>(
    id: RequestId,
    params: Value,
    f: impl FnOnce(P) -> R,
) -> Response {
    match parse::<P>(params) {
        Ok(p) => Response::new_ok(id, f(p)),
        Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
    }
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "panic".to_owned())
}
