//! Connection-free server state.
//!
//! Everything the server does *except* read/write JSON-RPC messages lives here.
//! That makes the handlers directly unit-testable without wiring up an
//! in-memory `lsp_server::Connection` or a worker thread.

use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, InlayHint, InlayHintParams, Location,
    PublishDiagnosticsParams, ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams,
    SignatureHelp, SignatureHelpParams, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};

use crate::handlers;
use crate::line_index::PositionEncoding;
use crate::vfs::FileId;
use crate::workspace::Workspace;

pub struct GlobalState {
    pub workspace: Workspace,
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
            workspace: Workspace::new(encoding),
            encoding,
        }
    }

    /// Adopt the client's workspace folders (called once from `main` after
    /// the `initialize` handshake). Triggers project-root discovery and an
    /// initial scan for `.flx` files.
    pub fn set_workspace_folders(&mut self, roots: Vec<std::path::PathBuf>) {
        self.workspace.set_roots(roots);
    }

    // ── notifications ──────────────────────────────────────────────────────

    pub fn handle_did_open(
        &mut self,
        params: DidOpenTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let rebuilt = self
            .workspace
            .open(&uri, version, params.text_document.text);
        self.diagnostics_for_files(&rebuilt)
    }

    pub fn handle_did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        // textDocumentSync = Full means we get one change with the entire
        // document text.
        let Some(change) = params.content_changes.into_iter().next() else {
            return vec![];
        };
        let rebuilt = self.workspace.change(&uri, version, change.text);
        self.diagnostics_for_files(&rebuilt)
    }

    pub fn handle_did_save(
        &mut self,
        params: DidSaveTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        match self.workspace.file_id(&params.text_document.uri) {
            Some(id) => self.diagnostics_for_files(&[id]),
            None => vec![],
        }
    }

    pub fn handle_did_close(&mut self, params: DidCloseTextDocumentParams) {
        self.workspace.close(&params.text_document.uri);
    }

    pub fn handle_did_change_watched_files(
        &mut self,
        params: DidChangeWatchedFilesParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let mut rebuilt: Vec<FileId> = Vec::new();
        for change in params.changes {
            for fid in self.workspace.on_disk_changed(&change.uri) {
                if !rebuilt.contains(&fid) {
                    rebuilt.push(fid);
                }
            }
        }
        self.diagnostics_for_files(&rebuilt)
    }

    // ── requests ──────────────────────────────────────────────────────────

    pub fn handle_hover(&self, params: HoverParams) -> Option<Hover> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self.workspace.snapshot_for_uri(&text_document.uri)?;
        handlers::hover::hover_at(snapshot, position)
    }

    pub fn handle_document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let snapshot = self.workspace.snapshot_for_uri(&params.text_document.uri)?;
        let symbols = handlers::symbols::document_symbols(
            &snapshot.program,
            &snapshot.interner,
            &snapshot.position_map,
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
        let snapshot = self.workspace.snapshot_for_uri(&text_document.uri)?;
        let nav = handlers::definition::goto_definition(snapshot, &text_document.uri, position)?;
        // Compute the source-side "from" span (the cursor word's range)
        // so VS Code can underline just that word in the originating
        // file when displaying the peek view. Returns `None` when the
        // cursor is on whitespace/punctuation; that's fine — clients
        // fall back to highlighting the cursor's full line.
        let origin = cursor_word_range(snapshot, position);
        Some(GotoDefinitionResponse::Link(vec![
            nav.into_location_link(origin),
        ]))
    }

    pub fn handle_completion(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let snapshot = self
            .workspace
            .snapshot_for_uri(&params.text_document_position.text_document.uri)?;
        Some(handlers::completion::complete(
            snapshot,
            params.text_document_position.position,
        ))
    }

    pub fn handle_formatting(&self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let snapshot = self.workspace.snapshot_for_uri(&params.text_document.uri)?;
        Some(handlers::formatting::format(snapshot))
    }

    pub fn handle_inlay_hints(&self, params: InlayHintParams) -> Vec<InlayHint> {
        let Some(snapshot) = self.workspace.snapshot_for_uri(&params.text_document.uri) else {
            return vec![];
        };
        handlers::inlay_hints::inlay_hints(snapshot)
    }

    pub fn handle_signature_help(&self, params: SignatureHelpParams) -> Option<SignatureHelp> {
        let snapshot = self
            .workspace
            .snapshot_for_uri(&params.text_document_position_params.text_document.uri)?;
        handlers::signature_help::signature_help(
            snapshot,
            params.text_document_position_params.position,
        )
    }

    pub fn handle_references(&self, params: ReferenceParams) -> Vec<Location> {
        let Some(id) = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)
        else {
            return vec![];
        };
        handlers::references::find_references(
            &self.workspace,
            id,
            params.text_document_position.position,
        )
    }

    pub fn handle_rename(&self, params: RenameParams) -> Option<WorkspaceEdit> {
        let id = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)?;
        handlers::rename::rename(
            &self.workspace,
            id,
            params.text_document_position.position,
            params.new_name,
        )
    }

    pub fn handle_semantic_tokens_full(&self, params: SemanticTokensParams) -> SemanticTokens {
        let Some(snapshot) = self.workspace.snapshot_for_uri(&params.text_document.uri) else {
            return SemanticTokens {
                result_id: None,
                data: vec![],
            };
        };
        handlers::semantic_tokens::semantic_tokens(snapshot)
    }

    // ── helpers ───────────────────────────────────────────────────────────

    fn diagnostics_for_files(&self, ids: &[FileId]) -> Vec<PublishDiagnosticsParams> {
        ids.iter()
            .filter_map(|&id| self.diagnostics_for(id))
            .collect()
    }

    fn diagnostics_for(&self, id: FileId) -> Option<PublishDiagnosticsParams> {
        let snapshot = self.workspace.snapshot(id)?;
        let uri = self.workspace.uri_of(id)?;
        let version = self.workspace.version(id).unwrap_or(0);
        Some(handlers::diagnostics::build(&uri, version, snapshot))
    }
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
