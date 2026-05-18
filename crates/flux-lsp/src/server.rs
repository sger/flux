use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::RecvTimeoutError;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Notification as _,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, InlayHintRequest,
    References, Rename, Request as _, SemanticTokensFullRequest, SignatureHelpRequest,
    WorkspaceSymbolRequest,
};

use crate::analysis::{AnalysisJob, AnalysisWorker};
use crate::global_state::GlobalState;
use crate::line_index::PositionEncoding;
use crate::workspace::WorkspaceRoot;

pub struct Server {
    pub connection: Connection,
    pub state: GlobalState,
}

impl Server {
    pub fn new(
        connection: Connection,
        encoding: PositionEncoding,
        workspace_roots: Vec<WorkspaceRoot>,
    ) -> Self {
        Self {
            connection,
            state: GlobalState::async_runtime(encoding, workspace_roots),
        }
    }

    pub fn run(mut self) -> Result<()> {
        let worker = AnalysisWorker::start();
        worker.send(self.state.initial_analysis_job());
        let mut pending_debounced: Option<(Instant, AnalysisJob)> = None;

        loop {
            while let Some(result) = worker.try_recv() {
                if let Some(diagnostics) = self.state.accept_analysis_result(result) {
                    for diagnostics in diagnostics {
                        self.publish_diagnostics(diagnostics)?;
                    }
                }
            }

            if pending_debounced
                .as_ref()
                .is_some_and(|(deadline, _)| *deadline <= Instant::now())
                && let Some((_, job)) = pending_debounced.take()
            {
                worker.send(job);
                continue;
            }

            let timeout = pending_debounced
                .as_ref()
                .map(|(deadline, _)| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|| Duration::from_millis(50));

            match self.connection.receiver.recv_timeout(timeout) {
                Ok(Message::Request(req)) => {
                    if self.connection.handle_shutdown(&req)? {
                        worker.shutdown();
                        return Ok(());
                    }
                    self.on_request(req)?;
                }
                Ok(Message::Notification(note)) => match self.on_notification(note)? {
                    AnalysisSchedule::None => {}
                    AnalysisSchedule::Immediate(job) => {
                        pending_debounced = None;
                        worker.send(job);
                    }
                    AnalysisSchedule::Debounced(job) => {
                        pending_debounced =
                            Some((Instant::now() + Duration::from_millis(150), job));
                    }
                },
                Ok(Message::Response(_)) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    worker.shutdown();
                    return Ok(());
                }
            }
        }
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
                Response::new_ok(id, serde_json::to_value(self.state.handle_rename(params))?)
            }
            m if m == SemanticTokensFullRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_semantic_tokens_full(params))?,
                )
            }
            m if m == WorkspaceSymbolRequest::METHOD => {
                let params = serde_json::from_value(req.params)?;
                Response::new_ok(
                    id,
                    serde_json::to_value(self.state.handle_workspace_symbol(params))?,
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

    fn on_notification(&mut self, note: Notification) -> Result<AnalysisSchedule> {
        let schedule = match note.method.as_str() {
            m if m == DidOpenTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                AnalysisSchedule::Immediate(self.state.record_did_open(params))
            }
            m if m == DidChangeTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                AnalysisSchedule::Debounced(self.state.record_did_change(params))
            }
            m if m == DidSaveTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                AnalysisSchedule::Immediate(self.state.record_did_save(params))
            }
            m if m == DidCloseTextDocument::METHOD => {
                let params = serde_json::from_value(note.params)?;
                AnalysisSchedule::Immediate(self.state.record_did_close(params))
            }
            m if m == DidChangeWatchedFiles::METHOD => {
                let params = serde_json::from_value(note.params)?;
                AnalysisSchedule::Immediate(self.state.record_did_change_watched_files(params))
            }
            _ => {
                tracing::trace!(method = %note.method, "notification ignored");
                AnalysisSchedule::None
            }
        };
        Ok(schedule)
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

enum AnalysisSchedule {
    None,
    Immediate(AnalysisJob),
    Debounced(AnalysisJob),
}
