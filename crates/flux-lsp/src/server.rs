use anyhow::Result;
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, Request as _,
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
                None
            }
            _ => {
                tracing::trace!(method = %note.method, "notification ignored");
                None
            }
        };
        if let Some(diagnostics) = diagnostics {
            self.publish_diagnostics(diagnostics)?;
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
