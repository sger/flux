use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    Cancel, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, DidSaveTextDocument, Initialized, Notification as _,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest,
    InlayHintRequest, References, Rename, Request as _, SemanticTokensFullRequest,
    SignatureHelpRequest,
};
use lsp_types::{CancelParams, NumberOrString};

use crate::global_state::GlobalState;
use crate::line_index::PositionEncoding;
use crate::loader;
use crate::task::{Cancellation, Job, WorkItem, worker_loop};

pub struct Server {
    pub connection: Connection,
    pub state: GlobalState,
    /// Channel to the read-request worker. `None` once the loop has shut down.
    work_tx: Option<Sender<WorkItem>>,
    /// Shared generation counter + `$/cancelRequest` set.
    cancel: Cancellation,
    /// Worker thread handle, joined on shutdown.
    worker: Option<JoinHandle<()>>,
    /// File-change detection backend (client registration or `notify`).
    loader: Box<dyn loader::Handle>,
    /// On-disk `.flx` change batches from the `notify` backend. The
    /// `ClientHandle` keeps the matching `Sender` alive but never sends on it —
    /// it delivers changes via `didChangeWatchedFiles` instead — so this
    /// receiver never disconnects while the server runs.
    loader_rx: Receiver<loader::Message>,
}

/// One unit of work pulled from the main-loop `select!`: an LSP message from
/// the client, a file-change batch from the loader, or a closed channel.
enum Incoming {
    Lsp(Message),
    Loader(loader::Message),
    Disconnected,
}

impl Server {
    pub fn new(
        connection: Connection,
        encoding: PositionEncoding,
        watcher: loader::WatcherKind,
    ) -> Self {
        let (work_tx, work_rx) = crossbeam_channel::unbounded::<WorkItem>();
        let cancel = Cancellation::new();
        let worker = {
            let sender = connection.sender.clone();
            let cancel = cancel.clone();
            std::thread::spawn(move || worker_loop(work_rx, sender, cancel))
        };
        let (loader_tx, loader_rx) = crossbeam_channel::unbounded::<loader::Message>();
        let loader: Box<dyn loader::Handle> = match watcher {
            loader::WatcherKind::Client => Box::new(loader::ClientHandle::new(
                connection.sender.clone(),
                loader_tx,
            )),
            loader::WatcherKind::Notify => Box::new(loader::NotifyHandle::new(loader_tx)),
        };
        Self {
            connection,
            state: GlobalState::new(encoding),
            work_tx: Some(work_tx),
            cancel,
            worker: Some(worker),
            loader,
            loader_rx,
        }
    }

    pub fn run(mut self) -> Result<()> {
        loop {
            match self.next_incoming() {
                Incoming::Lsp(Message::Request(req)) => {
                    if self.connection.handle_shutdown(&req)? {
                        break;
                    }
                    self.on_request(req)?;
                }
                Incoming::Lsp(Message::Notification(note)) => {
                    self.on_notification(note)?;
                }
                Incoming::Lsp(Message::Response(_)) => {}
                Incoming::Loader(loader::Message::Changed { files }) => {
                    self.on_loader_changed(files)?;
                }
                Incoming::Disconnected => break,
            }
        }
        // Dropping the sender ends the worker's `recv` loop; then join it.
        self.work_tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Ok(())
    }

    /// Block until the LSP client or the file-watch loader has a message.
    /// Borrows `&self` only for the duration of the `select!`, so the
    /// `&mut self` message handlers run without a borrow conflict.
    fn next_incoming(&self) -> Incoming {
        crossbeam_channel::select! {
            recv(self.connection.receiver) -> msg => match msg {
                Ok(msg) => Incoming::Lsp(msg),
                Err(_) => Incoming::Disconnected,
            },
            recv(self.loader_rx) -> msg => match msg {
                Ok(msg) => Incoming::Loader(msg),
                Err(_) => Incoming::Disconnected,
            },
        }
    }

    /// Apply a batch of on-disk `.flx` changes reported by the `notify`
    /// backend, coalescing any further batches already queued, then re-analyze
    /// and republish diagnostics for the affected files.
    fn on_loader_changed(&mut self, mut files: Vec<PathBuf>) -> Result<()> {
        // Drain a burst of filesystem events into one re-analysis pass.
        while let Ok(loader::Message::Changed { files: more }) = self.loader_rx.try_recv() {
            files.extend(more);
        }
        files.sort();
        files.dedup();
        let diagnostics = self.state.handle_disk_changes(&files);
        // Re-analysis changes content — bump so stale worker jobs are dropped.
        self.cancel.bump_generation();
        for params in diagnostics {
            self.publish_diagnostics(params)?;
        }
        Ok(())
    }

    /// Parse a read request, run its main-thread `dispatch_*` (lazy snapshot
    /// build), and hand the resulting job to the worker. Requests with no
    /// snapshot answer immediately with `null`.
    fn on_request(&mut self, req: Request) -> Result<()> {
        let id = req.id.clone();
        let job: Option<Job> = match req.method.as_str() {
            m if m == HoverRequest::METHOD => self
                .state
                .dispatch_hover(serde_json::from_value(req.params)?),
            m if m == DocumentSymbolRequest::METHOD => self
                .state
                .dispatch_document_symbol(serde_json::from_value(req.params)?),
            m if m == GotoDefinition::METHOD => self
                .state
                .dispatch_definition(serde_json::from_value(req.params)?),
            m if m == Completion::METHOD => self
                .state
                .dispatch_completion(serde_json::from_value(req.params)?),
            m if m == CodeActionRequest::METHOD => self
                .state
                .dispatch_code_action(serde_json::from_value(req.params)?),
            m if m == Formatting::METHOD => self
                .state
                .dispatch_formatting(serde_json::from_value(req.params)?),
            m if m == InlayHintRequest::METHOD => self
                .state
                .dispatch_inlay_hints(serde_json::from_value(req.params)?),
            m if m == SignatureHelpRequest::METHOD => self
                .state
                .dispatch_signature_help(serde_json::from_value(req.params)?),
            m if m == References::METHOD => self
                .state
                .dispatch_references(serde_json::from_value(req.params)?),
            m if m == Rename::METHOD => self
                .state
                .dispatch_rename(serde_json::from_value(req.params)?),
            m if m == SemanticTokensFullRequest::METHOD => self
                .state
                .dispatch_semantic_tokens_full(serde_json::from_value(req.params)?),
            other => {
                tracing::debug!(method = %other, "unhandled request");
                let resp = Response::new_err(
                    id,
                    ErrorCode::MethodNotFound as i32,
                    format!("method not implemented: {other}"),
                );
                self.connection.sender.send(Message::Response(resp))?;
                return Ok(());
            }
        };
        match job {
            Some(compute) => {
                let item = WorkItem {
                    id,
                    generation: self.cancel.generation(),
                    compute,
                };
                if let Some(tx) = &self.work_tx {
                    let _ = tx.send(item);
                }
            }
            // No snapshot for the document — nothing to compute.
            None => {
                let resp = Response::new_ok(id, serde_json::Value::Null);
                self.connection.sender.send(Message::Response(resp))?;
            }
        }
        Ok(())
    }

    fn on_notification(&mut self, note: Notification) -> Result<()> {
        let mut content_changed = false;
        let diagnostics = match note.method.as_str() {
            m if m == DidOpenTextDocument::METHOD => {
                content_changed = true;
                self.state
                    .handle_did_open(serde_json::from_value(note.params)?)
            }
            m if m == DidChangeTextDocument::METHOD => {
                content_changed = true;
                self.state
                    .handle_did_change(serde_json::from_value(note.params)?)
            }
            m if m == DidSaveTextDocument::METHOD => self
                .state
                .handle_did_save(serde_json::from_value(note.params)?),
            m if m == DidCloseTextDocument::METHOD => {
                content_changed = true;
                self.state
                    .handle_did_close(serde_json::from_value(note.params)?)
            }
            m if m == DidChangeWatchedFiles::METHOD => {
                content_changed = true;
                self.state
                    .handle_did_change_watched_files(serde_json::from_value(note.params)?)
            }
            m if m == Initialized::METHOD => {
                // The client is ready — start watching `.flx` files so on-disk
                // edits to unopened modules invalidate dependents. Bind `roots`
                // first: `self.loader.watch(self.state.workspace.roots())` would
                // borrow `self` mutably and immutably at once.
                let roots: Vec<PathBuf> = self.state.workspace.roots().to_vec();
                self.loader.watch(&roots);
                Vec::new()
            }
            m if m == Cancel::METHOD => {
                let params: CancelParams = serde_json::from_value(note.params)?;
                self.cancel.cancel(request_id(params.id));
                Vec::new()
            }
            _ => {
                tracing::trace!(method = %note.method, "notification ignored");
                Vec::new()
            }
        };
        if content_changed {
            // Bump *after* handling so a read request already queued with the
            // previous generation is detected as stale by the worker.
            self.cancel.bump_generation();
        }
        // A single edit can refresh diagnostics for several files — when a
        // buffer pulls in user modules, every reachable module is republished.
        for params in diagnostics {
            self.publish_diagnostics(params)?;
        }
        Ok(())
    }

    fn publish_diagnostics(&self, params: lsp_types::PublishDiagnosticsParams) -> Result<()> {
        self.connection
            .sender
            .send(Message::Notification(Notification {
                method: lsp_types::notification::PublishDiagnostics::METHOD.to_string(),
                params: serde_json::to_value(params)?,
            }))?;
        Ok(())
    }
}

/// Convert an LSP `$/cancelRequest` id into an `lsp_server::RequestId`.
fn request_id(id: NumberOrString) -> RequestId {
    match id {
        NumberOrString::Number(n) => RequestId::from(n),
        NumberOrString::String(s) => RequestId::from(s),
    }
}
