use std::path::PathBuf;
use std::thread::JoinHandle;

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Progress;
use lsp_types::notification::{
    Cancel, DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument,
    DidOpenTextDocument, DidSaveTextDocument, Initialized, Notification as _,
};
use lsp_types::request::WorkDoneProgressCreate;
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
    CodeActionRequest, CodeLensRequest, Completion, DocumentDiagnosticRequest,
    DocumentHighlightRequest, DocumentLinkRequest, DocumentSymbolRequest, FoldingRangeRequest,
    Formatting, GotoDefinition, GotoImplementation, HoverRequest, InlayHintRequest,
    PrepareRenameRequest, References, Rename, Request as _, ResolveCompletionItem,
    SelectionRangeRequest, SemanticTokensFullRequest, SignatureHelpRequest, TypeHierarchyPrepare,
    TypeHierarchySubtypes, TypeHierarchySupertypes, WillRenameFiles, WorkspaceDiagnosticRefresh,
    WorkspaceDiagnosticRequest, WorkspaceSymbolRequest,
};
use lsp_types::{
    CancelParams, NumberOrString, ProgressParams, ProgressParamsValue, ProgressToken,
    WorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressCreateParams, WorkDoneProgressEnd,
};

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
    /// Whether the client advertised `window.workDoneProgress` — gates the
    /// `$/progress` notifications around prelude warm-up.
    client_supports_progress: bool,
    /// Whether the client pulls diagnostics (`textDocument/diagnostic`). When
    /// it does, push (`publishDiagnostics`) is suppressed so the two diagnostic
    /// sources don't double up in the editor — see [`Server::report_diagnostics`].
    client_supports_pull_diagnostics: bool,
    /// Whether the client honours `workspace/diagnostic/refresh`. Lets the
    /// server nudge a pulling client to re-pull dependents after a cross-file
    /// edit (a plain pull only re-fetches the focused document).
    client_supports_diagnostic_refresh: bool,
    /// Monotonic id suffix for the fire-and-forget refresh requests.
    refresh_seq: u64,
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
            client_supports_progress: false,
            client_supports_pull_diagnostics: false,
            client_supports_diagnostic_refresh: false,
            refresh_seq: 0,
            loader,
            loader_rx,
        }
    }

    /// Record whether the client supports server-initiated work-done
    /// progress (`window.workDoneProgress`). Called once after `initialize`.
    pub fn set_progress_support(&mut self, supported: bool) {
        self.client_supports_progress = supported;
    }

    /// Record the client's pull-diagnostic support, from `initialize`:
    /// `pull` = the client issues `textDocument/diagnostic`; `refresh` = it
    /// honours `workspace/diagnostic/refresh`. Called once after `initialize`.
    pub fn set_diagnostic_support(&mut self, pull: bool, refresh: bool) {
        self.client_supports_pull_diagnostics = pull;
        self.client_supports_diagnostic_refresh = refresh;
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
        self.report_diagnostics(diagnostics)?;
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
            m if m == GotoImplementation::METHOD => self
                .state
                .dispatch_implementation(serde_json::from_value(req.params)?),
            m if m == CallHierarchyPrepare::METHOD => self
                .state
                .dispatch_prepare_call_hierarchy(serde_json::from_value(req.params)?),
            m if m == CallHierarchyIncomingCalls::METHOD => self
                .state
                .dispatch_incoming_calls(serde_json::from_value(req.params)?),
            m if m == CallHierarchyOutgoingCalls::METHOD => self
                .state
                .dispatch_outgoing_calls(serde_json::from_value(req.params)?),
            m if m == TypeHierarchyPrepare::METHOD => self
                .state
                .dispatch_prepare_type_hierarchy(serde_json::from_value(req.params)?),
            m if m == TypeHierarchySupertypes::METHOD => self
                .state
                .dispatch_supertypes(serde_json::from_value(req.params)?),
            m if m == TypeHierarchySubtypes::METHOD => self
                .state
                .dispatch_subtypes(serde_json::from_value(req.params)?),
            m if m == Completion::METHOD => self
                .state
                .dispatch_completion(serde_json::from_value(req.params)?),
            m if m == ResolveCompletionItem::METHOD => self
                .state
                .dispatch_completion_resolve(serde_json::from_value(req.params)?),
            m if m == CodeActionRequest::METHOD => self
                .state
                .dispatch_code_action(serde_json::from_value(req.params)?),
            m if m == CodeLensRequest::METHOD => self
                .state
                .dispatch_code_lens(serde_json::from_value(req.params)?),
            m if m == DocumentLinkRequest::METHOD => self
                .state
                .dispatch_document_link(serde_json::from_value(req.params)?),
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
            m if m == DocumentHighlightRequest::METHOD => self
                .state
                .dispatch_document_highlight(serde_json::from_value(req.params)?),
            m if m == Rename::METHOD => self
                .state
                .dispatch_rename(serde_json::from_value(req.params)?),
            m if m == PrepareRenameRequest::METHOD => self
                .state
                .dispatch_prepare_rename(serde_json::from_value(req.params)?),
            m if m == FoldingRangeRequest::METHOD => self
                .state
                .dispatch_folding_range(serde_json::from_value(req.params)?),
            m if m == SelectionRangeRequest::METHOD => self
                .state
                .dispatch_selection_range(serde_json::from_value(req.params)?),
            m if m == SemanticTokensFullRequest::METHOD => self
                .state
                .dispatch_semantic_tokens_full(serde_json::from_value(req.params)?),
            m if m == WorkspaceSymbolRequest::METHOD => self
                .state
                .dispatch_workspace_symbol(serde_json::from_value(req.params)?),
            m if m == DocumentDiagnosticRequest::METHOD => self
                .state
                .dispatch_document_diagnostic(serde_json::from_value(req.params)?),
            m if m == WorkspaceDiagnosticRequest::METHOD => self
                .state
                .dispatch_workspace_diagnostic(serde_json::from_value(req.params)?),
            m if m == WillRenameFiles::METHOD => self
                .state
                .dispatch_will_rename_files(serde_json::from_value(req.params)?),
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
                // Warm the Flow prelude up front (rather than lazily on the
                // first edit), reporting it via `$/progress` so the startup
                // cost is visible instead of a silent hang.
                if !roots.is_empty() {
                    self.warm_prelude_with_progress()?;
                }
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
        self.report_diagnostics(diagnostics)?;
        Ok(())
    }

    /// Load the Flow prelude, bracketing the work in `$/progress`
    /// begin/end when the client supports it and the load will actually
    /// happen (the prelude is not yet loaded).
    fn warm_prelude_with_progress(&mut self) -> Result<()> {
        const TOKEN: &str = "flux-lsp/prelude";
        let report = self.client_supports_progress && !self.state.workspace.prelude_loaded();
        if report {
            self.progress_begin(TOKEN, "Flux: indexing standard library")?;
        }
        self.state.workspace.warm_prelude();
        if report {
            self.progress_end(TOKEN, "Standard library ready")?;
        }
        Ok(())
    }

    /// Create a progress token and emit its `begin`. `window/workDoneProgress/create`
    /// is sent fire-and-forget — its response is ignored by the main loop.
    fn progress_begin(&self, token: &str, title: &str) -> Result<()> {
        let token = ProgressToken::String(token.to_string());
        self.connection.sender.send(Message::Request(Request {
            id: RequestId::from(format!("flux-progress/{title}")),
            method: WorkDoneProgressCreate::METHOD.to_string(),
            params: serde_json::to_value(WorkDoneProgressCreateParams {
                token: token.clone(),
            })?,
        }))?;
        self.send_progress(
            token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: title.to_string(),
                cancellable: Some(false),
                message: None,
                percentage: None,
            }),
        )
    }

    fn progress_end(&self, token: &str, message: &str) -> Result<()> {
        self.send_progress(
            ProgressToken::String(token.to_string()),
            WorkDoneProgress::End(WorkDoneProgressEnd {
                message: Some(message.to_string()),
            }),
        )
    }

    fn send_progress(&self, token: ProgressToken, value: WorkDoneProgress) -> Result<()> {
        self.connection
            .sender
            .send(Message::Notification(Notification {
                method: Progress::METHOD.to_string(),
                params: serde_json::to_value(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(value),
                })?,
            }))?;
        Ok(())
    }

    /// Deliver freshly computed diagnostics. A pull-capable client owns its
    /// diagnostics through `textDocument/diagnostic`, so pushing them too would
    /// paint every squiggle twice — instead we stay quiet and, when the client
    /// honours it, fire one `workspace/diagnostic/refresh` so it re-pulls the
    /// dependents a cross-file edit touched (a plain pull only re-fetches the
    /// focused document). Clients that don't pull get the push, unchanged.
    fn report_diagnostics(
        &mut self,
        diagnostics: Vec<lsp_types::PublishDiagnosticsParams>,
    ) -> Result<()> {
        if self.client_supports_pull_diagnostics {
            if !diagnostics.is_empty() && self.client_supports_diagnostic_refresh {
                self.request_diagnostic_refresh()?;
            }
            return Ok(());
        }
        for params in diagnostics {
            self.publish_diagnostics(params)?;
        }
        Ok(())
    }

    /// Ask a pulling client to re-pull all diagnostics. Fire-and-forget: the
    /// response is ignored by the main loop, so each carries a unique id.
    fn request_diagnostic_refresh(&mut self) -> Result<()> {
        self.refresh_seq += 1;
        self.connection.sender.send(Message::Request(Request {
            id: RequestId::from(format!("flux-diagnostic-refresh/{}", self.refresh_seq)),
            method: WorkspaceDiagnosticRefresh::METHOD.to_string(),
            params: serde_json::Value::Null,
        }))?;
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
