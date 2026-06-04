//! Connection-free server state.
//!
//! Everything the server does *except* read/write JSON-RPC messages lives here.
//! That makes the handlers directly unit-testable without wiring up an
//! in-memory `lsp_server::Connection` or a worker thread.

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    CodeActionParams, CodeActionResponse, CodeLens, CodeLensParams, CompletionItem,
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentDiagnosticParams, DocumentDiagnosticReportResult, DocumentFormattingParams,
    DocumentHighlight, DocumentHighlightParams, DocumentLink, DocumentLinkParams,
    DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, DocumentSymbolParams,
    DocumentSymbolResponse, FoldingRange, FoldingRangeParams, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverParams, InlayHint, InlayHintParams,
    LinkedEditingRangeParams, LinkedEditingRanges, Location, PrepareRenameResponse,
    PublishDiagnosticsParams, ReferenceParams, RenameFilesParams, RenameParams, SelectionRange,
    SelectionRangeParams, SemanticTokens, SemanticTokensDeltaParams, SemanticTokensFullDeltaResult,
    SemanticTokensParams, SemanticTokensRangeParams, SignatureHelp, SignatureHelpParams,
    TextDocumentPositionParams, TextEdit, TypeHierarchyItem, TypeHierarchyPrepareParams,
    TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, WorkspaceDiagnosticParams,
    WorkspaceDiagnosticReportResult, WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
};

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::handlers;
use crate::handlers::semantic_tokens::SemanticTokenCache;
use crate::line_index::PositionEncoding;
use crate::task::Job;
use crate::vfs::FileId;
use crate::workspace::Workspace;

pub struct GlobalState {
    pub workspace: Workspace,
    pub encoding: PositionEncoding,
    /// `flux.workspaceDiagnostics.scanAllFiles` — when set, `workspace/diagnostic`
    /// force-analyzes every discovered `.flx` instead of just the analyzed
    /// working set. Off by default (see `handlers::diagnostics::workspace_gather`).
    workspace_diagnostics_scan_all: bool,
    /// Last semantic-token stream per document, so `…/full/delta` can answer
    /// with a splice. Shared (`Arc<Mutex<…>>`) because the worker-thread
    /// dispatch jobs update it off the main thread.
    semantic_token_cache: Arc<Mutex<SemanticTokenCache>>,
    /// Files whose edits have been staged ([`Workspace::stage_change`]) but not
    /// yet re-analyzed. Drained by [`flush_analysis`](Self::flush_analysis) when
    /// the server's debounce timer fires, coalescing a keystroke burst into a
    /// single component re-analysis.
    dirty: HashSet<FileId>,
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
            workspace_diagnostics_scan_all: false,
            semantic_token_cache: Arc::new(Mutex::new(SemanticTokenCache::default())),
            dirty: HashSet::new(),
        }
    }

    /// Set the `flux.workspaceDiagnostics.scanAllFiles` flag (from the client's
    /// `initializationOptions`). Called once after `initialize`.
    pub fn set_workspace_diagnostics_scan_all(&mut self, scan_all: bool) {
        self.workspace_diagnostics_scan_all = scan_all;
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

    /// Handle `textDocument/didChange` synchronously — stage the edit and
    /// immediately flush its analysis. The server's event loop drives editor
    /// typing through [`stage_did_change`](Self::stage_did_change) +
    /// [`flush_analysis`](Self::flush_analysis) instead, so the expensive
    /// re-analysis is debounced; this method keeps the one-shot, return-the-
    /// diagnostics contract that the tests (and any non-debouncing caller) rely
    /// on.
    pub fn handle_did_change(
        &mut self,
        params: DidChangeTextDocumentParams,
    ) -> Vec<PublishDiagnosticsParams> {
        self.stage_did_change(params);
        self.flush_analysis()
    }

    /// Apply the *cheap* half of a `textDocument/didChange`: update the buffer
    /// content and symbol index and invalidate stale snapshots, then mark the
    /// file dirty so the next [`flush_analysis`](Self::flush_analysis) re-runs
    /// inference for its component. Publishes nothing — diagnostics follow on
    /// flush. textDocumentSync = Full, so each change carries the whole document.
    pub fn stage_did_change(&mut self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;
        let Some(change) = params.content_changes.into_iter().next() else {
            return;
        };
        if let Some(id) = self.workspace.stage_change(&uri, version, change.text) {
            self.dirty.insert(id);
        }
    }

    /// Whether any staged edit is awaiting re-analysis.
    pub fn has_pending_analysis(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Re-analyze every staged (dirty) file and its dependents, then return the
    /// freshly computed diagnostics to publish. A no-op (`vec![]`) when nothing
    /// is pending, so it is safe to call on a spurious timer wake-up.
    pub fn flush_analysis(&mut self) -> Vec<PublishDiagnosticsParams> {
        if self.dirty.is_empty() {
            return vec![];
        }
        let mut rebuilt: Vec<FileId> = Vec::new();
        for id in self.dirty.drain().collect::<Vec<_>>() {
            for fid in self.workspace.reanalyze(id) {
                if !rebuilt.contains(&fid) {
                    rebuilt.push(fid);
                }
            }
        }
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
        self.semantic_token_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget(uri.as_str());
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

    pub fn handle_implementation(
        &mut self,
        params: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::implementation::gather(&mut self.workspace, file, position)?;
        handlers::implementation::goto_implementation(&bundle)
    }

    pub fn handle_type_definition(
        &mut self,
        params: GotoDefinitionParams,
    ) -> Option<GotoDefinitionResponse> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&text_document.uri)?;
        let nav =
            handlers::definition::goto_type_definition(snapshot, &text_document.uri, position)?;
        let origin = cursor_word_range(snapshot, position);
        Some(GotoDefinitionResponse::Link(vec![
            nav.into_location_link(origin),
        ]))
    }

    pub fn handle_prepare_call_hierarchy(
        &mut self,
        params: CallHierarchyPrepareParams,
    ) -> Option<Vec<CallHierarchyItem>> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::call_hierarchy::prepare_gather(&mut self.workspace, file, position)?;
        let items = handlers::call_hierarchy::prepare_items(&bundle);
        if items.is_empty() { None } else { Some(items) }
    }

    pub fn handle_incoming_calls(
        &mut self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Option<Vec<CallHierarchyIncomingCall>> {
        let bundle = handlers::call_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(handlers::call_hierarchy::incoming_calls(&bundle))
    }

    pub fn handle_outgoing_calls(
        &mut self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let bundle = handlers::call_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(handlers::call_hierarchy::outgoing_calls(
            &bundle,
            &params.item,
        ))
    }

    pub fn handle_prepare_type_hierarchy(
        &mut self,
        params: TypeHierarchyPrepareParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::type_hierarchy::prepare_gather(&mut self.workspace, file, position)?;
        let items = handlers::type_hierarchy::prepare_items(&bundle);
        if items.is_empty() { None } else { Some(items) }
    }

    pub fn handle_supertypes(
        &mut self,
        params: TypeHierarchySupertypesParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let bundle = handlers::type_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(handlers::type_hierarchy::supertypes(&bundle))
    }

    pub fn handle_subtypes(
        &mut self,
        params: TypeHierarchySubtypesParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let bundle = handlers::type_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(handlers::type_hierarchy::subtypes(&bundle))
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

    /// `completionItem/resolve` — fill in an item's documentation. Keyword /
    /// effect / type docs come from the item's `data`; a module-member item's
    /// `///` doc comment is fetched here from the module source.
    pub fn handle_completion_resolve(&mut self, item: CompletionItem) -> CompletionItem {
        let member_doc = handlers::completion::member_ref(&item)
            .and_then(|(module, member)| self.workspace.member_doc(&module, &member));
        handlers::completion::resolve(item, member_doc)
    }

    pub fn handle_document_highlight(
        &mut self,
        params: DocumentHighlightParams,
    ) -> Vec<DocumentHighlight> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let Some(snapshot) = self.workspace.ensure_snapshot_for_uri(&text_document.uri) else {
            return vec![];
        };
        handlers::document_highlight::document_highlights(snapshot, position)
    }

    pub fn handle_linked_editing_range(
        &mut self,
        params: LinkedEditingRangeParams,
    ) -> Option<LinkedEditingRanges> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&text_document.uri)?;
        handlers::linked_editing::linked_editing_ranges(snapshot, position)
    }

    pub fn handle_workspace_symbol(
        &mut self,
        params: WorkspaceSymbolParams,
    ) -> Option<WorkspaceSymbolResponse> {
        let files = self.workspace.workspace_symbol_files();
        let symbols = handlers::workspace_symbol::query(&files, &params.query);
        Some(WorkspaceSymbolResponse::Nested(symbols))
    }

    pub fn handle_code_action(&mut self, params: CodeActionParams) -> Option<CodeActionResponse> {
        let uri = params.text_document.uri;
        let only = params.context.only;
        let modules = self.workspace.workspace_module_full_names();
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri)?;
        Some(handlers::code_action::code_actions(
            snapshot,
            &uri,
            params.range,
            &modules,
            only.as_deref(),
        ))
    }

    pub fn handle_code_lens(&mut self, params: CodeLensParams) -> Option<Vec<CodeLens>> {
        let uri = params.text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri)?;
        Some(handlers::code_lens::code_lenses(snapshot, &uri))
    }

    pub fn handle_document_link(
        &mut self,
        params: DocumentLinkParams,
    ) -> Option<Vec<DocumentLink>> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        Some(handlers::document_link::document_links(snapshot))
    }

    pub fn handle_formatting(&mut self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        Some(handlers::formatting::format(snapshot))
    }

    pub fn handle_formatting_range(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> Option<Vec<TextEdit>> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        Some(handlers::formatting::format_range(snapshot, params.range))
    }

    pub fn handle_on_type_formatting(
        &mut self,
        params: DocumentOnTypeFormattingParams,
    ) -> Option<Vec<TextEdit>> {
        let position = params.text_document_position.position;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document_position.text_document.uri)?;
        Some(handlers::on_type_formatting::on_type_format(
            snapshot,
            position,
            &params.ch,
            params.options.tab_size,
            params.options.insert_spaces,
        ))
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

    /// `inlayHint/resolve` — fill in a hint's tooltip and insert-annotation
    /// edit. Stateless (everything rides on the hint), so no snapshot is needed.
    pub fn handle_inlay_hint_resolve(&mut self, hint: InlayHint) -> InlayHint {
        handlers::inlay_hints::resolve(hint)
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

    pub fn handle_prepare_rename(
        &mut self,
        params: TextDocumentPositionParams,
    ) -> Option<PrepareRenameResponse> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)?;
        handlers::rename::prepare_rename(snapshot, params.position)
    }

    pub fn handle_folding_range(&mut self, params: FoldingRangeParams) -> Vec<FoldingRange> {
        match self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
        {
            Some(snapshot) => handlers::folding::folding_ranges(snapshot),
            None => vec![],
        }
    }

    pub fn handle_selection_range(&mut self, params: SelectionRangeParams) -> Vec<SelectionRange> {
        match self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
        {
            Some(snapshot) => {
                handlers::selection_range::selection_ranges(snapshot, &params.positions)
            }
            None => vec![],
        }
    }

    pub fn handle_semantic_tokens_full(&mut self, params: SemanticTokensParams) -> SemanticTokens {
        let uri = params.text_document.uri;
        let Some(snapshot) = self.workspace.ensure_snapshot_for_uri(&uri).cloned() else {
            return SemanticTokens {
                result_id: None,
                data: vec![],
            };
        };
        handlers::semantic_tokens::full(&snapshot, uri.as_str(), &self.semantic_token_cache)
    }

    /// `textDocument/semanticTokens/full/delta` — re-highlight after an edit as
    /// a minimal splice against the client's last result id.
    pub fn handle_semantic_tokens_full_delta(
        &mut self,
        params: SemanticTokensDeltaParams,
    ) -> SemanticTokensFullDeltaResult {
        let uri = params.text_document.uri;
        let previous = params.previous_result_id;
        let Some(snapshot) = self.workspace.ensure_snapshot_for_uri(&uri).cloned() else {
            return SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
                result_id: None,
                data: vec![],
            });
        };
        handlers::semantic_tokens::full_delta(
            &snapshot,
            uri.as_str(),
            &previous,
            &self.semantic_token_cache,
        )
    }

    /// `textDocument/semanticTokens/range` — colour just the requested span.
    pub fn handle_semantic_tokens_range(
        &mut self,
        params: SemanticTokensRangeParams,
    ) -> SemanticTokens {
        let Some(snapshot) = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()
        else {
            return SemanticTokens {
                result_id: None,
                data: vec![],
            };
        };
        handlers::semantic_tokens::range(&snapshot, params.range)
    }

    /// `textDocument/diagnostic` (pull model). Always answers with a concrete
    /// report — an unknown document yields an empty `Full` report rather than
    /// `null` (a pull result can't be null).
    pub fn handle_document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> DocumentDiagnosticReportResult {
        match self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
        {
            Some(snapshot) => {
                handlers::diagnostics::report(snapshot, params.previous_result_id.as_deref())
            }
            None => handlers::diagnostics::empty_report(),
        }
    }

    /// `workspace/diagnostic` (project-wide pull). Reports problems for every
    /// file the workspace knows, short-circuiting unchanged ones via the result
    /// ids the client round-trips back.
    pub fn handle_workspace_diagnostic(
        &mut self,
        params: WorkspaceDiagnosticParams,
    ) -> WorkspaceDiagnosticReportResult {
        let scan_all = self.workspace_diagnostics_scan_all;
        let files = handlers::diagnostics::workspace_gather(
            &mut self.workspace,
            &params.previous_result_ids,
            scan_all,
        );
        handlers::diagnostics::workspace_report(&files)
    }

    /// `workspace/willRenameFiles` — rewrite `import`s so a renamed/moved module
    /// file keeps resolving. `None` when the rename touches no module reference.
    pub fn handle_will_rename_files(&mut self, params: RenameFilesParams) -> Option<WorkspaceEdit> {
        handlers::rename_files::will_rename_files(&mut self.workspace, &params.files)
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

    pub fn dispatch_implementation(&mut self, params: GotoDefinitionParams) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::implementation::gather(&mut self.workspace, file, position)?;
        Some(Box::new(
            move || match handlers::implementation::goto_implementation(&bundle) {
                Some(resp) => to_value(resp),
                None => serde_json::Value::Null,
            },
        ))
    }

    pub fn dispatch_type_definition(&mut self, params: GotoDefinitionParams) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let uri = text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(
            move || match handlers::definition::goto_type_definition(&snapshot, &uri, position) {
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

    /// Custom `flux/view*` request: render a compiler-stage dump (tokens / Core
    /// IR / bytecode) for the document, off-thread, and return it as a string.
    pub fn dispatch_view(
        &mut self,
        params: lsp_types::TextDocumentIdentifier,
        kind: handlers::view::ViewKind,
    ) -> Option<Job> {
        let snapshot = self.workspace.ensure_snapshot_for_uri(&params.uri)?;
        let source = snapshot.text.as_ref().to_string();
        let path = params.uri.as_str().to_string();
        Some(Box::new(move || {
            serde_json::Value::String(handlers::view::render(kind, &source, &path))
        }))
    }

    pub fn dispatch_prepare_call_hierarchy(
        &mut self,
        params: CallHierarchyPrepareParams,
    ) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::call_hierarchy::prepare_gather(&mut self.workspace, file, position)?;
        Some(Box::new(move || {
            let items = handlers::call_hierarchy::prepare_items(&bundle);
            if items.is_empty() {
                serde_json::Value::Null
            } else {
                to_value(items)
            }
        }))
    }

    pub fn dispatch_incoming_calls(
        &mut self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Option<Job> {
        let bundle = handlers::call_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(Box::new(move || {
            to_value(handlers::call_hierarchy::incoming_calls(&bundle))
        }))
    }

    pub fn dispatch_outgoing_calls(
        &mut self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Option<Job> {
        let item = params.item.clone();
        let bundle = handlers::call_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(Box::new(move || {
            to_value(handlers::call_hierarchy::outgoing_calls(&bundle, &item))
        }))
    }

    pub fn dispatch_prepare_type_hierarchy(
        &mut self,
        params: TypeHierarchyPrepareParams,
    ) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let file = self.workspace.file_id(&text_document.uri)?;
        let bundle = handlers::type_hierarchy::prepare_gather(&mut self.workspace, file, position)?;
        Some(Box::new(move || {
            let items = handlers::type_hierarchy::prepare_items(&bundle);
            if items.is_empty() {
                serde_json::Value::Null
            } else {
                to_value(items)
            }
        }))
    }

    pub fn dispatch_supertypes(&mut self, params: TypeHierarchySupertypesParams) -> Option<Job> {
        let bundle = handlers::type_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(Box::new(move || {
            to_value(handlers::type_hierarchy::supertypes(&bundle))
        }))
    }

    pub fn dispatch_subtypes(&mut self, params: TypeHierarchySubtypesParams) -> Option<Job> {
        let bundle = handlers::type_hierarchy::item_gather(&mut self.workspace, &params.item)?;
        Some(Box::new(move || {
            to_value(handlers::type_hierarchy::subtypes(&bundle))
        }))
    }

    pub fn dispatch_completion(&mut self, params: CompletionParams) -> Option<Job> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::completion::complete(&snapshot, position))
        }))
    }

    /// `completionItem/resolve` worker job. The module-member doc lookup needs
    /// the workspace, so it runs here on the main thread; the (cheap) markdown
    /// assembly happens in the job. Always produces a response.
    pub fn dispatch_completion_resolve(&mut self, item: CompletionItem) -> Option<Job> {
        let member_doc = handlers::completion::member_ref(&item)
            .and_then(|(module, member)| self.workspace.member_doc(&module, &member));
        Some(Box::new(move || {
            to_value(handlers::completion::resolve(item, member_doc))
        }))
    }

    pub fn dispatch_document_highlight(&mut self, params: DocumentHighlightParams) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::document_highlight::document_highlights(
                &snapshot, position,
            ))
        }))
    }

    pub fn dispatch_linked_editing_range(
        &mut self,
        params: LinkedEditingRangeParams,
    ) -> Option<Job> {
        let TextDocumentPositionParams {
            text_document,
            position,
        } = params.text_document_position_params;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::linked_editing::linked_editing_ranges(
                &snapshot, position,
            ))
        }))
    }

    pub fn dispatch_workspace_symbol(&mut self, params: WorkspaceSymbolParams) -> Option<Job> {
        // Gather the cached per-file declaration index on the main thread —
        // cheap `Arc` clones — then filter off the main thread (no re-parse).
        let files = self.workspace.workspace_symbol_files();
        let query = params.query;
        Some(Box::new(move || {
            let symbols = handlers::workspace_symbol::query(&files, &query);
            to_value(WorkspaceSymbolResponse::Nested(symbols))
        }))
    }

    pub fn dispatch_code_action(&mut self, params: CodeActionParams) -> Option<Job> {
        let uri = params.text_document.uri;
        let range = params.range;
        let only = params.context.only;
        let modules = self.workspace.workspace_module_full_names();
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::code_action::code_actions(
                &snapshot,
                &uri,
                range,
                &modules,
                only.as_deref(),
            ))
        }))
    }

    pub fn dispatch_code_lens(&mut self, params: CodeLensParams) -> Option<Job> {
        let uri = params.text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        Some(Box::new(move || {
            to_value(handlers::code_lens::code_lenses(&snapshot, &uri))
        }))
    }

    pub fn dispatch_document_link(&mut self, params: DocumentLinkParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::document_link::document_links(&snapshot))
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

    pub fn dispatch_formatting_range(
        &mut self,
        params: DocumentRangeFormattingParams,
    ) -> Option<Job> {
        let range = params.range;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::formatting::format_range(&snapshot, range))
        }))
    }

    pub fn dispatch_on_type_formatting(
        &mut self,
        params: DocumentOnTypeFormattingParams,
    ) -> Option<Job> {
        let position = params.text_document_position.position;
        let ch = params.ch;
        let tab_size = params.options.tab_size;
        let insert_spaces = params.options.insert_spaces;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document_position.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::on_type_formatting::on_type_format(
                &snapshot,
                position,
                &ch,
                tab_size,
                insert_spaces,
            ))
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

    /// `inlayHint/resolve` worker job. Stateless — the resolved fields are
    /// derived from the hint itself — so it always produces a response.
    pub fn dispatch_inlay_hint_resolve(&mut self, hint: InlayHint) -> Option<Job> {
        Some(Box::new(move || {
            to_value(handlers::inlay_hints::resolve(hint))
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

    pub fn dispatch_prepare_rename(&mut self, params: TextDocumentPositionParams) -> Option<Job> {
        let position = params.position;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            match handlers::rename::prepare_rename(&snapshot, position) {
                Some(resp) => to_value(resp),
                None => serde_json::Value::Null,
            }
        }))
    }

    pub fn dispatch_folding_range(&mut self, params: FoldingRangeParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::folding::folding_ranges(&snapshot))
        }))
    }

    pub fn dispatch_selection_range(&mut self, params: SelectionRangeParams) -> Option<Job> {
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        let positions = params.positions;
        Some(Box::new(move || {
            to_value(handlers::selection_range::selection_ranges(
                &snapshot, &positions,
            ))
        }))
    }

    pub fn dispatch_semantic_tokens_full(&mut self, params: SemanticTokensParams) -> Option<Job> {
        let uri = params.text_document.uri;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        let cache = self.semantic_token_cache.clone();
        Some(Box::new(move || {
            to_value(handlers::semantic_tokens::full(
                &snapshot,
                uri.as_str(),
                &cache,
            ))
        }))
    }

    pub fn dispatch_semantic_tokens_full_delta(
        &mut self,
        params: SemanticTokensDeltaParams,
    ) -> Option<Job> {
        let uri = params.text_document.uri;
        let previous = params.previous_result_id;
        let snapshot = self.workspace.ensure_snapshot_for_uri(&uri).cloned()?;
        let cache = self.semantic_token_cache.clone();
        Some(Box::new(move || {
            to_value(handlers::semantic_tokens::full_delta(
                &snapshot,
                uri.as_str(),
                &previous,
                &cache,
            ))
        }))
    }

    pub fn dispatch_semantic_tokens_range(
        &mut self,
        params: SemanticTokensRangeParams,
    ) -> Option<Job> {
        let range = params.range;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned()?;
        Some(Box::new(move || {
            to_value(handlers::semantic_tokens::range(&snapshot, range))
        }))
    }

    /// Worker-thread `textDocument/diagnostic`. Always returns a job (never
    /// `None`) so the dispatcher never falls back to a `null` response — a pull
    /// result must be a report.
    pub fn dispatch_document_diagnostic(
        &mut self,
        params: DocumentDiagnosticParams,
    ) -> Option<Job> {
        let previous = params.previous_result_id;
        let snapshot = self
            .workspace
            .ensure_snapshot_for_uri(&params.text_document.uri)
            .cloned();
        Some(Box::new(move || match snapshot {
            Some(snapshot) => to_value(handlers::diagnostics::report(
                &snapshot,
                previous.as_deref(),
            )),
            None => to_value(handlers::diagnostics::empty_report()),
        }))
    }

    /// Worker-thread `workspace/diagnostic`. Snapshots are built/collected on
    /// the main thread (the gather needs `&mut Workspace`); the per-file report
    /// rendering runs off-thread. Always returns a job — a pull result can't be
    /// null.
    pub fn dispatch_workspace_diagnostic(
        &mut self,
        params: WorkspaceDiagnosticParams,
    ) -> Option<Job> {
        let scan_all = self.workspace_diagnostics_scan_all;
        let files = handlers::diagnostics::workspace_gather(
            &mut self.workspace,
            &params.previous_result_ids,
            scan_all,
        );
        Some(Box::new(move || {
            to_value(handlers::diagnostics::workspace_report(&files))
        }))
    }

    /// Worker-thread `workspace/willRenameFiles`. The edit needs `&mut Workspace`
    /// (it ensures dependents' snapshots), so it's computed on the main thread;
    /// `None` (no edit) lets the dispatcher reply `null`, which is valid here.
    pub fn dispatch_will_rename_files(&mut self, params: RenameFilesParams) -> Option<Job> {
        let edit = handlers::rename_files::will_rename_files(&mut self.workspace, &params.files)?;
        Some(Box::new(move || to_value(edit)))
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
pub(crate) fn cursor_word_range(
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
