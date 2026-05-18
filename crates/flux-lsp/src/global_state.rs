//! Connection-free server state.
//!
//! Everything the server does *except* read/write JSON-RPC messages lives here.
//! That makes the handlers directly unit-testable without wiring up an
//! in-memory `lsp_server::Connection` or a worker thread.

use std::collections::HashMap;

use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, InlayHint, InlayHintParams, Location,
    PublishDiagnosticsParams, ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams,
    SignatureHelp, SignatureHelpParams, TextDocumentPositionParams, TextEdit, Uri, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

use crate::analysis::{
    AnalysisGeneration, AnalysisJob, AnalysisReason, AnalysisResult, OpenDocumentData,
};
use crate::document::DocumentStore;
use crate::handlers;
use crate::line_index::PositionEncoding;
use crate::locator::find_at;
use crate::workspace::{Workspace, WorkspaceRoot};

pub struct GlobalState {
    pub docs: DocumentStore,
    pub workspace: Workspace,
    pub encoding: PositionEncoding,
    workspace_roots: Vec<WorkspaceRoot>,
    discover_on_first_open: bool,
    analysis_generation: AnalysisGeneration,
    open_documents: HashMap<Uri, OpenDocumentData>,
}

impl Default for GlobalState {
    fn default() -> Self {
        Self::new(PositionEncoding::Utf16)
    }
}

impl GlobalState {
    pub fn new(encoding: PositionEncoding) -> Self {
        Self::with_workspace_roots(encoding, Vec::new())
    }

    pub fn with_workspace_roots(
        encoding: PositionEncoding,
        workspace_roots: Vec<WorkspaceRoot>,
    ) -> Self {
        let workspace = Workspace::new(workspace_roots.clone(), encoding);
        Self {
            docs: DocumentStore::new(encoding),
            workspace,
            encoding,
            workspace_roots,
            discover_on_first_open: false,
            analysis_generation: AnalysisGeneration(0),
            open_documents: HashMap::new(),
        }
    }

    pub fn with_workspace_roots_and_first_open_discovery(
        encoding: PositionEncoding,
        workspace_roots: Vec<WorkspaceRoot>,
    ) -> Self {
        let mut state = Self::with_workspace_roots(encoding, workspace_roots);
        state.workspace.enable_first_open_discovery();
        state.discover_on_first_open = true;
        state
    }

    pub fn async_runtime(encoding: PositionEncoding, workspace_roots: Vec<WorkspaceRoot>) -> Self {
        Self {
            docs: DocumentStore::new(encoding),
            workspace: Workspace::empty(encoding),
            encoding,
            workspace_roots,
            discover_on_first_open: true,
            analysis_generation: AnalysisGeneration(0),
            open_documents: HashMap::new(),
        }
    }

    pub fn initial_analysis_job(&mut self) -> AnalysisJob {
        self.bump_generation();
        self.analysis_job(AnalysisReason::Startup)
    }

    pub fn record_did_open(&mut self, params: DidOpenTextDocumentParams) -> AnalysisJob {
        self.bump_generation();
        let doc = params.text_document;
        self.open_documents.insert(
            doc.uri.clone(),
            OpenDocumentData {
                uri: doc.uri,
                version: doc.version,
                text: doc.text,
            },
        );
        self.analysis_job(AnalysisReason::DidOpen)
    }

    pub fn record_did_change(&mut self, params: DidChangeTextDocumentParams) -> AnalysisJob {
        self.bump_generation();
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        if let Some(change) = params.content_changes.into_iter().next() {
            self.open_documents.insert(
                uri.clone(),
                OpenDocumentData {
                    uri,
                    version,
                    text: change.text,
                },
            );
        }
        self.analysis_job(AnalysisReason::DidChange)
    }

    pub fn record_did_save(&mut self, _params: DidSaveTextDocumentParams) -> AnalysisJob {
        self.bump_generation();
        self.analysis_job(AnalysisReason::DidSave)
    }

    pub fn record_did_close(&mut self, params: DidCloseTextDocumentParams) -> AnalysisJob {
        self.bump_generation();
        self.open_documents.remove(&params.text_document.uri);
        self.analysis_job(AnalysisReason::DidClose)
    }

    pub fn record_did_change_watched_files(
        &mut self,
        _params: DidChangeWatchedFilesParams,
    ) -> AnalysisJob {
        self.bump_generation();
        self.analysis_job(AnalysisReason::WatchedFiles)
    }

    pub fn accept_analysis_result(
        &mut self,
        result: AnalysisResult,
    ) -> Option<Vec<PublishDiagnosticsParams>> {
        if result.generation != self.analysis_generation {
            return None;
        }
        self.docs = result.snapshot.docs;
        self.workspace = result.snapshot.workspace;
        Some(self.workspace_diagnostics())
    }

    pub fn analysis_generation(&self) -> AnalysisGeneration {
        self.analysis_generation
    }

    fn bump_generation(&mut self) {
        self.analysis_generation.0 += 1;
    }

    fn analysis_job(&self, reason: AnalysisReason) -> AnalysisJob {
        AnalysisJob {
            generation: self.analysis_generation,
            reason,
            roots: self.workspace_roots.clone(),
            open_documents: self.open_documents.values().cloned().collect(),
            encoding: self.encoding,
            discover_on_first_open: self.discover_on_first_open,
        }
    }

    // ── notifications ──────────────────────────────────────────────────────

    pub fn handle_did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        self.open_documents.insert(
            uri.clone(),
            OpenDocumentData {
                uri: uri.clone(),
                version,
                text: params.text_document.text.clone(),
            },
        );
        self.docs
            .open(uri.clone(), version, params.text_document.text);
        self.workspace.open(
            &uri,
            version,
            self.docs.get(&uri)?.snapshot.text.to_string(),
        );
        self.diagnostics_for(&uri)
    }

    pub fn handle_did_open_all(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let _ = self.handle_did_open(params);
        self.workspace_diagnostics()
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
            let text = change.text;
            self.open_documents.insert(
                uri.clone(),
                OpenDocumentData {
                    uri: uri.clone(),
                    version,
                    text: text.clone(),
                },
            );
            self.docs.change(uri.clone(), version, text.clone());
            self.workspace.change(&uri, version, text);
        }
        self.diagnostics_for(&uri)
    }

    pub fn handle_did_change_all(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let _ = self.handle_did_change(params);
        self.workspace_diagnostics()
    }

    pub fn handle_did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> Option<PublishDiagnosticsParams> {
        self.diagnostics_for(&params.text_document.uri)
    }

    pub fn handle_did_save_all(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let _ = self.handle_did_save(params);
        self.workspace_diagnostics()
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) {
        self.open_documents.remove(&params.text_document.uri);
        self.docs.close(&params.text_document.uri);
        self.workspace.close(&params.text_document.uri);
    }

    pub fn handle_did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> Vec<PublishDiagnosticsParams> {
        for change in params.changes {
            self.workspace.rescan_path(&change.uri);
        }
        self.workspace_diagnostics()
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
        let nav =
            handlers::definition::goto_definition(&doc.snapshot, &text_document.uri, position)
                .or_else(|| self.workspace_definition(&text_document.uri, position))?;
        // Compute the source-side "from" span (the cursor word's range)
        // so VS Code can underline just that word in the originating
        // file when displaying the peek view. Returns `None` when the
        // cursor is on whitespace/punctuation; that's fine — clients
        // fall back to highlighting the cursor's full line.
        let origin = cursor_word_range(&doc.snapshot, position);
        Some(GotoDefinitionResponse::Link(vec![
            nav.into_location_link(origin),
        ]))
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
        if let Some(open_doc) = self.open_documents.get(&params.text_document.uri) {
            let mut docs = DocumentStore::new(self.encoding);
            docs.open(
                open_doc.uri.clone(),
                open_doc.version,
                open_doc.text.clone(),
            );
            return docs
                .get(&params.text_document.uri)
                .map(|doc| handlers::formatting::format(&doc.snapshot));
        }
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
        let local = handlers::references::find_references(
            &doc.snapshot,
            &params.text_document_position.text_document.uri,
            params.text_document_position.position,
        );
        let Some(name) = symbol_name_at(&doc.snapshot, params.text_document_position.position)
        else {
            return local;
        };
        let workspace = self.workspace.references_by_name(&name);
        if workspace.is_empty() {
            local
        } else {
            workspace
        }
    }

    pub fn handle_rename(&self, params: RenameParams) -> Option<WorkspaceEdit> {
        let doc = self
            .docs
            .get(&params.text_document_position.text_document.uri)?;
        if let Some(name) = symbol_name_at(&doc.snapshot, params.text_document_position.position)
            && let Some(edit) = self
                .workspace
                .rename_by_name(&name, params.new_name.clone())
        {
            return Some(edit);
        }
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
            return SemanticTokens {
                result_id: None,
                data: vec![],
            };
        };
        handlers::semantic_tokens::semantic_tokens(&doc.snapshot)
    }

    pub fn handle_workspace_symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Option<WorkspaceSymbolResponse> {
        Some(WorkspaceSymbolResponse::Flat(
            self.workspace.symbols(&params.query),
        ))
    }

    pub fn workspace_diagnostics(&self) -> Vec<PublishDiagnosticsParams> {
        if self.workspace.is_empty() {
            return vec![];
        }
        self.workspace.diagnostics()
    }

    // ── helpers ───────────────────────────────────────────────────────────

    fn diagnostics_for(&self, uri: &Uri) -> Option<PublishDiagnosticsParams> {
        if let Some(file) = self.workspace.file_by_uri(uri) {
            return self
                .workspace
                .diagnostics()
                .into_iter()
                .find(|params| params.uri == file.uri);
        }
        let doc = self.docs.get(uri)?;
        Some(handlers::diagnostics::build(
            uri,
            doc.version,
            &doc.snapshot,
        ))
    }

    fn workspace_definition(
        &self,
        uri: &Uri,
        position: lsp_types::Position,
    ) -> Option<crate::navigation_target::NavigationTarget> {
        let doc = self.docs.get(uri)?;
        let name = symbol_name_at(&doc.snapshot, position)?;
        self.workspace.definition_by_name(&name)
    }
}

fn symbol_name_at(
    snapshot: &crate::snapshot::Snapshot,
    position: lsp_types::Position,
) -> Option<String> {
    let target = snapshot.position_map.lsp_to_flux(position)?;
    let node = find_at(&snapshot.program, &snapshot.interner, target)?;
    let id = handlers::references::node_identifier(&node)?;
    snapshot.interner.try_resolve(id).map(ToString::to_string)
}

/// Return the LSP `Range` covering the identifier word at `position` in
/// the source buffer, or `None` if the cursor is on whitespace /
/// punctuation. Used as the `origin_selection_range` of a
/// `LocationLink` so VS Code's peek view underlines just the cursor
/// word, not the whole line. Reuses the same word-detection logic as
/// keyword hover.
fn cursor_word_range(
    snapshot: &crate::snapshot::Snapshot,
    position: lsp_types::Position,
) -> Option<lsp_types::Range> {
    let offset = snapshot.position_map.lsp_to_offset(position)?;
    let off: usize = offset.into();
    let text = snapshot.text.as_ref();
    let word = crate::keywords::word_at_offset(text, off)?;
    // Find the byte range of `word` in `text` around `off`. The same
    // shape `word_at_offset` computes internally — recomputed here so we
    // can produce an LSP `Range` rather than just the &str.
    let bytes = text.as_bytes();
    let pivot = if off < bytes.len() && is_ident_byte(bytes[off]) {
        off
    } else if off > 0 && is_ident_byte(bytes[off - 1]) {
        off - 1
    } else {
        return None;
    };
    let mut start = pivot;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let end = start + word.len();
    let start_pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(start).ok()?.into());
    let end_pos = snapshot
        .position_map
        .offset_to_lsp(u32::try_from(end).ok()?.into());
    Some(lsp_types::Range {
        start: start_pos,
        end: end_pos,
    })
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
