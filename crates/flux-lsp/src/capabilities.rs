use lsp_types::{
    CompletionOptions, DefinitionOptions, HoverProviderCapability, OneOf, RenameOptions,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensServerCapabilities,
    ServerCapabilities, SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
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
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(false),
            work_done_progress_options: Default::default(),
        })),
        semantic_tokens_provider: Some(
            SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                legend: semantic_tokens_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: None,
                work_done_progress_options: Default::default(),
            }),
        ),
        ..Default::default()
    }
}
