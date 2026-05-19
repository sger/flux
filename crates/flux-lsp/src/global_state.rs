//! Connection-free server state.
//!
//! Everything the server does *except* read/write JSON-RPC messages lives here.
//! That makes the handlers directly unit-testable without wiring up an
//! in-memory `lsp_server::Connection` or a worker thread.

use lsp_types::{
    CodeActionParams, CodeActionResponse, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse,
    Hover, HoverParams, InlayHint, InlayHintParams, Location, PublishDiagnosticsParams,
    ReferenceParams, RenameParams, SemanticTokens, SemanticTokensParams, SignatureHelp,
    SignatureHelpParams, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};

use crate::handlers;
use crate::line_index::PositionEncoding;
use crate::task::Job;
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

    pub fn handle_did_close(
        &mut self,
        params: DidCloseTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        let uri = params.text_document.uri;
        let rebuilt = self.workspace.close(&uri);
        let mut diagnostics = self.diagnostics_for_files(&rebuilt);
        // Clear the closed file's own squiggles — its snapshot was dropped
        // and a later query will rebuild it lazily from disk.
        diagnostics.push(PublishDiagnosticsParams {
            uri,
            diagnostics: vec![],
            version: None,
        });
        diagnostics
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

    /// Apply on-disk `.flx` changes reported by the server-side `notify`
    /// watcher (path-keyed sibling of [`handle_did_change_watched_files`]
    /// (Self::handle_did_change_watched_files), which is uri-keyed). Both
    /// funnel into `Workspace::on_disk_changed`.
    pub fn handle_disk_changes(
        &mut self,
        paths: &[std::path::PathBuf],
    ) -> Vec<PublishDiagnosticsParams> {
        let mut rebuilt: Vec<FileId> = Vec::new();
        for path in paths {
            let Some(uri) = crate::vfs::path_to_uri(path) else {
                continue;
            };
            for fid in self.workspace.on_disk_changed(&uri) {
                if !rebuilt.contains(&fid) {
                    rebuilt.push(fid);
                }
            }
        }
        self.diagnostics_for_files(&rebuilt)
    }

    // ── requests ──────────────────────────────────────────────────────────

    pub fn handle_hover(&mut self, params: HoverParams) -> Option<Hover> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&text_document.uri)?;
        handlers::hover::hover_at(snapshot, position)
    }

    pub fn handle_document_symbol(
        &mut self,
        params: DocumentSymbolParams,
    ) -> Option<DocumentSymbolResponse> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        let symbols = handlers::symbols::document_symbols(
            &snapshot.program,
            &snapshot.interner,
            &snapshot.position_map,
        );
        Some(DocumentSymbolResponse::Nested(symbols))
    }

    pub fn handle_definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&text_document.uri)?;
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

    pub fn handle_completion(&mut self, params: CompletionParams) -> Option<CompletionResponse> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document_position.text_document.uri)?;
        Some(handlers::completion::complete(
            snapshot,
            params.text_document_position.position,
        ))
    }

    pub fn handle_code_action(&mut self, params: CodeActionParams) -> Option<CodeActionResponse> {
        let uri = params.text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri)?;
        Some(handlers::code_action::code_actions(
            snapshot,
            &uri,
            params.range,
        ))
    }

    pub fn handle_formatting(&mut self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        Some(handlers::formatting::format(snapshot))
    }

    pub fn handle_inlay_hints(&mut self, params: InlayHintParams) -> Vec<InlayHint> {
        let Some(snapshot) = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
        else {
            return vec![];
        };
        handlers::inlay_hints::inlay_hints(snapshot)
    }

    pub fn handle_signature_help(&mut self, params: SignatureHelpParams) -> Option<SignatureHelp> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document_position_params.text_document.uri)?;
        handlers::signature_help::signature_help(
            snapshot,
            params.text_document_position_params.position,
        )
    }

    pub fn handle_references(&mut self, params: ReferenceParams) -> Vec<Location> {
        let Some(id) = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)
        else {
            return vec![];
        };
        handlers::references::find_references(
            &mut self.workspace,
            id,
            params.text_document_position.position,
        )
    }

    pub fn handle_rename(&mut self, params: RenameParams) -> Option<WorkspaceEdit> {
        let id = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)?;
        handlers::rename::rename(
            &mut self.workspace,
            id,
            params.text_document_position.position,
            params.new_name,
        )
    }

    pub fn handle_semantic_tokens_full(&mut self, params: SemanticTokensParams) -> SemanticTokens {
        let Some(snapshot) = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
        else {
            return SemanticTokens {
                result_id: None,
                data: vec![],
            };
        };
        handlers::semantic_tokens::semantic_tokens(snapshot)
    }

    // ── request dispatch (worker thread) ───────────────────────────────────
    //
    // Each `dispatch_*` runs the main-thread part — lazy snapshot building —
    // and returns a `Send` closure that performs the actual read off-thread.
    // The `handle_*` methods above stay for synchronous/test use.

    pub fn dispatch_hover(&mut self, params: HoverParams) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::hover::hover_at(&snapshot, position))
        }))
    }

    pub fn dispatch_document_symbol(&mut self, params: DocumentSymbolParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            let symbols = handlers::symbols::document_symbols(
                &snapshot.program,
                &snapshot.interner,
                &snapshot.position_map,
            );
            to_value(DocumentSymbolResponse::Nested(symbols))
        }))
    }

    pub fn dispatch_definition(&mut self, params: GotoDefinitionParams) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let uri = text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(
            move || match handlers::definition::goto_definition(&snapshot, &uri, position) {
                Some(nav) => {
                    let origin = cursor_word_range(&snapshot, position);
                    to_value(GotoDefinitionResponse::Link(vec![
                        nav.into_location_link(origin),
                    ]))
                }
                None => serde_json::Value::Null,
            },
        ))
    }

    pub fn dispatch_completion(&mut self, params: CompletionParams) -> Option<Job> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::completion::complete(&snapshot, position))
        }))
    }

    pub fn dispatch_code_action(&mut self, params: CodeActionParams) -> Option<Job> {
        let uri = params.text_document.uri;
        let range = params.range;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::code_action::code_actions(&snapshot, &uri, range))
        }))
    }

    pub fn dispatch_formatting(&mut self, params: DocumentFormattingParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::formatting::format(&snapshot))
        }))
    }

    pub fn dispatch_inlay_hints(&mut self, params: InlayHintParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::inlay_hints::inlay_hints(&snapshot))
        }))
    }

    pub fn dispatch_signature_help(&mut self, params: SignatureHelpParams) -> Option<Job> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::signature_help::signature_help(
                &snapshot, position,
            ))
        }))
    }

    pub fn dispatch_semantic_tokens_full(&mut self, params: SemanticTokensParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::semantic_tokens::semantic_tokens(&snapshot))
        }))
    }

    pub fn dispatch_references(&mut self, params: ReferenceParams) -> Option<Job> {
        let id = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)?;
        let bundle = handlers::references::gather(
            &mut self.workspace,
            id,
            params.text_document_position.position,
        )?;
        Some(Box::new(move || {
            to_value(handlers::references::compute_locations(&bundle))
        }))
    }

    pub fn dispatch_rename(&mut self, params: RenameParams) -> Option<Job> {
        let id = self
            .workspace
            .file_id(&params.text_document_position.text_document.uri)?;
        let bundle = handlers::references::gather(
            &mut self.workspace,
            id,
            params.text_document_position.position,
        )?;
        let new_name = params.new_name;
        Some(Box::new(move || {
            to_value(handlers::rename::compute_workspace_edit(&bundle, &new_name))
        }))
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

/// Serialize a handler result to a JSON value for a `WorkItem` response,
/// falling back to `null` if serialization somehow fails.
fn to_value<T: serde::Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
