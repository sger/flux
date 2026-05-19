use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Initialized, Notification as _,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, InlayHintRequest,
    References, RegisterCapability, Rename, Request as _, SemanticTokensFullRequest,
    SignatureHelpRequest,
};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, Registration,
    RegistrationParams,
};

use crate::global_state::GlobalState;
use crate::line_index::PositionEncoding;

pub struct Server {
    pub connection: Connection,
    pub state: GlobalState,
}

impl Server {
    pub fn new(connection: Connection, encoding: PositionEncoding) -> Self {
        Self {
            connection,
            state: GlobalState::new(encoding),
        }
    }

    pub fn run(mut self) -> Result<()> {
        while let Ok(msg) = self.connection.receiver.recv() {
            match msg {
                Message::Request(req) => {
                    if self.connection.handle_shutdown(&req)? {
                        return Ok(());
                    }
                    self.on_request(req)?;
                }
                Message::Notification(note) => {
                    self.on_notification(note)?;
                }
                Message::Response(_) => {}
            }
        }
        Ok(())
    }

    fn on_request(&mut self, req: Request) -> Result<()> {
        let id = req.id.clone();
        let resp = match req.method.as_str() {
            m if m == HoverRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(id, serde_json::to_value(self.state.handle_hover(params))?)
            }
            m if m == DocumentSymbolRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_document_symbol(params))?,
                )
            }
            m if m == GotoDefinition::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_definition(params))?,
                )
            }
            m if m == Completion::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_completion(params))?,
                )
            }
            m if m == Formatting::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_formatting(params))?,
                )
            }
            m if m == InlayHintRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_inlay_hints(params))?,
                )
            }
            m if m == SignatureHelpRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_signature_help(params))?,
                )
            }
            m if m == References::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_references(params))?,
                )
            }
            m if m == Rename::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_rename(params))?,
                )
            }
            m if m == SemanticTokensFullRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_semantic_tokens_full(params))?,
                )
            }
            other => {
                tracing::debug!(method = %other, "unhandled request");
                Response::new_err(
                    id,
                    ErrorCode::MethodNotFound as i32,
                    format!("method not implemented: {other}"),
                )
            }
        };
        self.connection.sender.send(Message::Response(resp))?;
        Ok(())
    }

    fn on_notification(&mut self, note: Notification) -> Result<()> {
        let diagnostics = match note.method.as_str() {
            m if m == DidOpenTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                self.state.handle_did_open(params)
            }
            m if m == DidChangeTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                self.state.handle_did_change(params)
            }
            m if m == DidSaveTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                self.state.handle_did_save(params)
            }
            m if m == DidCloseTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                self.state.handle_did_close(params);
                Vec::new()
            }
            m if m == DidChangeWatchedFiles::METHOD => {
                let params = serde_json::from_value(note.params)?;
                self.state.handle_did_change_watched_files(params)
            }
            m if m == Initialized::METHOD => {
                // The client is ready — register a watcher for `.flx` files
                // so on-disk edits to unopened modules invalidate dependents.
                self.register_file_watchers()?;
                Vec::new()
            }
            _ => {
                tracing::trace!(method = %note.method, "notification ignored");
                Vec::new()
            }
        };
        // A single edit can refresh diagnostics for several files — when a
        // buffer pulls in user modules, every reachable module is republished.
        for params in diagnostics {
            self.publish_diagnostics(params)?;
        }
        Ok(())
    }

    /// Ask the client to watch every `.flx` file in the workspace and notify
    /// us via `workspace/didChangeWatchedFiles`. Sent once, after the client
    /// reports it is `initialized`. The client's response is not awaited —
    /// the main loop simply drops it.
    fn register_file_watchers(&self) -> Result<()> {
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*.flx".to_string()),
                kind: None,
            }],
        };
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "flux-lsp-watch-flx".to_string(),
                method: DidChangeWatchedFiles::METHOD.to_string(),
                register_options: Some(serde_json::to_value(options)?),
            }],
        };
        self.connection.sender.send(Message::Request(Request {
            id: RequestId::from("flux-lsp-register-watchers".to_string()),
            method: RegisterCapability::METHOD.to_string(),
            params: serde_json::to_value(params)?,
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
