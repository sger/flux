use lsp_types::{
    CodeActionProviderCapability, CompletionOptions, DefinitionOptions,
    FoldingRangeProviderCapability, HoverProviderCapability, ImplementationProviderCapability,
    OneOf, RenameOptions, SelectionRangeProviderCapability, SemanticTokensFullOptions,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    WorkspaceFoldersServerCapabilities, WorkspaceServerCapabilities,
};

use crate::handlers::semantic_tokens::semantic_tokens_legend;
use crate::line_index::PositionEncoding;

pub fn server_capabilities(encoding: PositionEncoding) -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        position_encoding: Some(encoding.as_lsp()),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        // `OneOf::Right(DefinitionOptions { .. })` opts into the
        // "with options" shape of `definitionProvider`. The actual
        // `LocationLink` vs `Location` shape is negotiated by the
        // client via the `textDocument.definition.linkSupport` client
        // capability — VS Code advertises it, so our handler
        // (`global_state::handle_definition`) returns
        // `GotoDefinitionResponse::Link(Vec<LocationLink>)` carrying
        // both `target_range` and `target_selection_range`.
        definition_provider: Some(OneOf::Right(DefinitionOptions {
            work_done_progress_options: Default::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            // `completionItem/resolve` fills in documentation lazily
            // (handlers::completion::resolve) so the initial list stays light.
            resolve_provider: Some(true),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        // Quick fixes derived from diagnostics (handlers::code_action).
        // `Simple(true)` — the server does not pre-filter by
        // `CodeActionKind`; every action it returns is a `QuickFix`.
        code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        // Project-wide symbol search (handlers::workspace_symbol).
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Highlight every occurrence of the symbol under the cursor within
        // the current file (handlers::document_highlight).
        document_highlight_provider: Some(OneOf::Left(true)),
        // Jump from a `class` to its `instance` blocks
        // (handlers::implementation).
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        // `prepare_provider: true` — the client sends `prepareRename` first
        // (handlers::rename::prepare_rename) to validate the cursor and get
        // the editable range before showing the rename input box.
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        // Structural code folding for declarations (handlers::folding).
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        // Smart expand-selection (handlers::selection_range).
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                work_done_progress_options: Default::default(),
            },
        )),
        // Advertise multi-root workspace support: the client sends
        // `workspaceFolders` at initialize and `didChangeWorkspaceFolders`
        // afterwards. The `Workspace` uses the roots to discover every
        // project `.flx` file for cross-file analysis.
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}
