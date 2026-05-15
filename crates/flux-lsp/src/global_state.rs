//! Connection-free server state.
//!
//! Everything the server does *except* read/write JSON-RPC messages lives here.
//! That makes the handlers directly unit-testable without wiring up an
//! in-memory `lsp_server::Connection` or a worker thread.

use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, InlayHint, InlayHintParams, Location, PublishDiagnosticsParams,
    ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams,
    SignatureHelp, SignatureHelpParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::DocumentStore;
use crate::handlers;
use crate::line_index::PositionEncoding;

pub struct GlobalState {
    pub docs: DocumentStore,
    pub encoding: PositionEncoding,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new(PositionEncoding::Utf16)
    }
}

impl GlobalState {
    pub fn new(encoding: PositionEncoding) -> Self {
        Self {
            docs: DocumentStore::new(encoding),
            encoding,
        }
    }

    // ── notifications ──────────────────────────────────────────────────────

    pub fn handle_did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        self.docs
            .open(uri.clone(), version, params.text_document.text);
        self.diagnostics_for(&uri)
    }

    pub fn handle_did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // textDocumentSync = Full means we get one change with the entire
        // document text.
        if let Some(change) = params.content_changes.into_iter().next() {
            self.docs.change(uri.clone(), version, change.text);
        }
        self.diagnostics_for(&uri)
    }

    pub fn handle_did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        self.diagnostics_for(&params.text_document.uri)
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) {
        self.docs.close(&params.text_document.uri);
    }

    // ── requests ──────────────────────────────────────────────────────────

    pub fn handle_hover(&self, params: HoverParams) -> Option<Hover> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let doc = self.docs.get(&text_document.uri)?;
        handlers::hover::hover_at(&doc.snapshot, position)
    }

    pub fn handle_document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let doc = self.docs.get(&params.text_document.uri)?;
        let symbols = handlers::symbols::document_symbols(
            &doc.snapshot.program,
            &doc.snapshot.interner,
            &doc.snapshot.position_map,
        );
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    pub fn handle_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let doc = self.docs.get(&text_document.uri)?;
        let location =
            handlers::definition::goto_definition(&doc.snapshot, &text_document.uri, position)?;
        Some(GotoDefinitionResponse::Scalar(location))
    }

    pub fn handle_completion(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let doc = self
            .docs
            .get(&params.text_document_position.text_document.uri)?;
        Some(handlers::completion::complete(
            &doc.snapshot,
            params.text_document_position.position,
        ))
    }

    pub fn handle_formatting(&self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let doc = self.docs.get(&params.text_document.uri)?;
        Some(handlers::formatting::format(&doc.snapshot))
    }

    pub fn handle_inlay_hints(&self, params: InlayHintParams) -> Vec<InlayHint> {
        let Some(doc) = self.docs.get(&params.text_document.uri) else {
            return vec![];
        };
        handlers::inlay_hints::inlay_hints(&doc.snapshot)
    }

    pub fn handle_signature_help(&self, params: SignatureHelpParams) -> Option<SignatureHelp> {
        let doc = self
            .docs
            .get(&params.text_document_position_params.text_document.uri)?;
        handlers::signature_help::signature_help(
            &doc.snapshot,
            params.text_document_position_params.position,
        )
    }

    pub fn handle_references(&self, params: ReferenceParams) -> Vec<Location> {
        let Some(doc) = self
            .docs
            .get(&params.text_document_position.text_document.uri)
        else {
            return vec![];
        };
        handlers::references::find_references(
            &doc.snapshot,
            &params.text_document_position.text_document.uri,
            params.text_document_position.position,
        )
    }

    pub fn handle_rename(&self, params: RenameParams) -> Option<WorkspaceEdit> {
        let doc = self
            .docs
            .get(&params.text_document_position.text_document.uri)?;
        handlers::rename::rename(
            &doc.snapshot,
            &params.text_document_position.text_document.uri,
            0,
            params.text_document_position.position,
            params.new_name,
        )
    }

    pub fn handle_semantic_tokens_full(&self, params: SemanticTokensParams) -> SemanticTokens {
        let Some(doc) = self.docs.get(&params.text_document.uri) else {
            return SemanticTokens { result_id: None, data: vec![] };
        };
        handlers::semantic_tokens::semantic_tokens(&doc.snapshot)
    }

    // ── helpers ───────────────────────────────────────────────────────────

    fn diagnostics_for(&self, uri: &Uri) -> Option<PublishDiagnosticsParams> {
        let doc = self.docs.get(uri)?;
        Some(handlers::diagnostics::build(
            uri,
            doc.version,
            &doc.snapshot,
        ))
    }
}
