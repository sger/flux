use anyhow::Result;
use lsp_server::{Connection, ErrorCode, ExtractError, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, DidSaveTextDocument,
    Notification as _,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, Request as _,
};
use lsp_types::{
    CompletionParams, CompletionResponse, DocumentFormattingParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, HoverParams, TextDocumentPositionParams,
    TextEdit,
};

use crate::document::DocumentStore;
use crate::handlers;

pub struct Server {
    pub connection: Connection,
    pub docs: DocumentStore,
}

impl Server {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            docs: DocumentStore::new(),
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
                let params: HoverParams = serde_json::from_value(req.params)?;
                let result = self.handle_hover(params);
                Response::new_ok(id, serde_json::to_value(result)?)
            }
            m if m == DocumentSymbolRequest::METHOD => {
                let params: lsp_types::DocumentSymbolParams = serde_json::from_value(req.params)?;
                let result = self.handle_document_symbol(params);
                Response::new_ok(id, serde_json::to_value(result)?)
            }
            m if m == GotoDefinition::METHOD => {
                let params: GotoDefinitionParams = serde_json::from_value(req.params)?;
                let result = self.handle_definition(params);
                Response::new_ok(id, serde_json::to_value(result)?)
            }
            m if m == Completion::METHOD => {
                let params: CompletionParams = serde_json::from_value(req.params)?;
                let result = self.handle_completion(params);
                Response::new_ok(id, serde_json::to_value(result)?)
            }
            m if m == Formatting::METHOD => {
                let params: DocumentFormattingParams = serde_json::from_value(req.params)?;
                let result = self.handle_formatting(params);
                Response::new_ok(id, serde_json::to_value(result)?)
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

    fn handle_hover(&self, params: HoverParams) -> Option<lsp_types::Hover> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let doc = self.docs.get(&text_document.uri)?;
        handlers::hover::hover_at(&doc.snapshot, position)
    }

    fn handle_document_symbol(
        &self,
        params: lsp_types::DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let doc = self.docs.get(&params.text_document.uri)?;
        let symbols =
            handlers::symbols::document_symbols(&doc.snapshot.program, &doc.snapshot.interner);
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    fn handle_definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let doc = self.docs.get(&text_document.uri)?;
        let location =
            handlers::definition::goto_definition(&doc.snapshot, &text_document.uri, position)?;
        Some(GotoDefinitionResponse::Scalar(location))
    }

    fn handle_completion(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let doc = self
            .docs
            .get(&params.text_document_position.text_document.uri)?;
        Some(handlers::completion::complete(&doc.snapshot))
    }

    fn handle_formatting(&self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let doc = self.docs.get(&params.text_document.uri)?;
        Some(handlers::formatting::format(&doc.snapshot))
    }

    fn on_notification(&mut self, note: Notification) -> Result<()> {
        match note.method.as_str() {
            m if m == DidOpenTextDocument::METHOD => {
                let params: lsp_types::DidOpenTextDocumentParams =
                    serde_json::from_value(note.params)?;
                let uri = params.text_document.uri;
                let version = params.text_document.version;
                self.docs
                    .open(uri.clone(), version, params.text_document.text);
                if let Some(doc) = self.docs.get(&uri) {
                    handlers::diagnostics::publish(
                        &self.connection,
                        &uri,
                        doc.version,
                        &doc.snapshot,
                    )?;
                }
            }
            m if m == DidChangeTextDocument::METHOD => {
                let params: lsp_types::DidChangeTextDocumentParams =
                    serde_json::from_value(note.params)?;
                let uri = params.text_document.uri;
                let version = params.text_document.version;
                // textDocumentSync = Full means we get one change with the
                // entire document text.
                if let Some(change) = params.content_changes.into_iter().next() {
                    self.docs.change(uri.clone(), version, change.text);
                }
                if let Some(doc) = self.docs.get(&uri) {
                    handlers::diagnostics::publish(
                        &self.connection,
                        &uri,
                        doc.version,
                        &doc.snapshot,
                    )?;
                }
            }
            m if m == DidSaveTextDocument::METHOD => {
                let params: lsp_types::DidSaveTextDocumentParams =
                    serde_json::from_value(note.params)?;
                if let Some(doc) = self.docs.get(&params.text_document.uri) {
                    handlers::diagnostics::publish(
                        &self.connection,
                        &params.text_document.uri,
                        doc.version,
                        &doc.snapshot,
                    )?;
                }
            }
            m if m == DidCloseTextDocument::METHOD => {
                let params: lsp_types::DidCloseTextDocumentParams =
                    serde_json::from_value(note.params)?;
                self.docs.close(&params.text_document.uri);
            }
            _ => {
                tracing::trace!(method = %note.method, "notification ignored");
            }
        }
        Ok(())
    }
}

#[allow(dead_code)]
fn cast_req<R>(req: Request) -> Result<(lsp_server::RequestId, R::Params), ExtractError<Request>>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    req.extract(R::METHOD)
}

#[allow(dead_code)]
fn cast_note<N>(note: Notification) -> Result<N::Params, ExtractError<Notification>>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    note.extract(N::METHOD)
}
