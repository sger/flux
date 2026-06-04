use lsp_types::{
    CallHierarchyServerCapability, CodeActionKind, CodeActionOptions, CodeActionProviderCapability,
    CodeLensOptions, CompletionOptions, DefinitionOptions, DiagnosticOptions,
    DiagnosticServerCapabilities, DocumentLinkOptions, DocumentOnTypeFormattingOptions,
    FileOperationFilter, FileOperationPattern, FileOperationPatternKind,
    FileOperationRegistrationOptions, FoldingRangeProviderCapability, HoverProviderCapability,
    ImplementationProviderCapability, InlayHintOptions, InlayHintServerCapabilities,
    LinkedEditingRangeServerCapabilities, OneOf, RenameOptions, SelectionRangeProviderCapability,
    SemanticTokensFullOptions, SemanticTokensOptions, SemanticTokensServerCapabilities,
    ServerCapabilities, SignatureHelpOptions, TextDocumentSyncCapability, TextDocumentSyncKind,
    TypeDefinitionProviderCapability, WorkspaceFileOperationsServerCapabilities,
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
        // Parameter hints (handlers::signature_help). `(` / `,` open the popup;
        // `,` and `)` *retrigger* it while it's already showing so the active
        // parameter advances (and the popup dismisses when the call closes)
        // without reopening from scratch.
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: Some(vec![",".to_string(), ")".to_string()]),
            work_done_progress_options: Default::default(),
        }),
        // Diagnostic quick fixes plus the `source.organizeImports` source
        // action (handlers::code_action). Advertising `code_action_kinds` lets
        // VS Code surface the "Organize Imports" command and run it on save.
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
            ]),
            work_done_progress_options: Default::default(),
            resolve_provider: None,
        })),
        // "▶ Run" / "▶ Run Test" runnables (handlers::code_lens). The command
        // is fully specified by the server, so no lazy `resolve` is needed.
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        // Clickable `import` module paths (handlers::document_link). Targets are
        // resolved eagerly, so no `documentLink/resolve` is offered.
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        // Format just the selection (handlers::formatting::format_range) — the
        // whole-file formatter diffed down to the hunks inside the range.
        document_range_formatting_provider: Some(OneOf::Left(true)),
        // Auto-indent as you type (handlers::on_type_formatting): a newline
        // indents the new line to the brace depth, `}` dedents its own line.
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "\n".to_string(),
            more_trigger_character: Some(vec!["}".to_string()]),
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        // Project-wide symbol search (handlers::workspace_symbol).
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Highlight every occurrence of the symbol under the cursor within
        // the current file (handlers::document_highlight).
        document_highlight_provider: Some(OneOf::Left(true)),
        // Jump from a `class` to its `instance` blocks
        // (handlers::implementation).
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        // Jump from an expression to the declaration of its inferred type's
        // ADT/alias (handlers::definition::goto_type_definition).
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        // "Who calls this / what does this call" navigation
        // (handlers::call_hierarchy): prepare + incoming/outgoing calls.
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        // Pull-model diagnostics: `textDocument/diagnostic` for one document
        // (handlers::diagnostics::report) and `workspace/diagnostic` for the
        // whole project (handlers::diagnostics::workspace_report). Push
        // diagnostics (`textDocument/publishDiagnostics`) still ship; the server
        // suppresses them per-client when the client opts into pulling, so the
        // two never double up (see `Server::report_diagnostics`).
        diagnostic_provider: Some(DiagnosticServerCapabilities::Options(DiagnosticOptions {
            identifier: Some("flux".to_string()),
            // Editing one module changes diagnostics in its dependents.
            inter_file_dependencies: true,
            // Also answer project-wide pulls (`workspace/diagnostic`).
            workspace_diagnostics: true,
            work_done_progress_options: Default::default(),
        })),
        // Inferred-type inlay hints (handlers::inlay_hints). `resolve_provider:
        // true` defers each hint's tooltip and its "insert the inferred type as
        // an annotation" text edit to `inlayHint/resolve`, keeping the initial
        // response small.
        inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
            InlayHintOptions {
                resolve_provider: Some(true),
                work_done_progress_options: Default::default(),
            },
        ))),
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
        // Edit every same-file occurrence of an identifier in lockstep
        // (handlers::linked_editing) — type once, all update — without a rename.
        linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(true)),
        // Semantic highlighting (handlers::semantic_tokens). `full.delta` lets a
        // re-highlight after an edit ship as a minimal splice (`…/full/delta`)
        // instead of the whole token stream; `range` answers `…/range` for just
        // the viewport so opening a large file colours the visible region first.
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: semantic_tokens_legend(),
                full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                range: Some(true),
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
            // `workspace/willRenameFiles` (handlers::rename_files): when a `.flx`
            // module file is renamed/moved, return an edit that rewrites the
            // `import`s in dependents so the rename doesn't break the project.
            file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                will_rename: Some(FileOperationRegistrationOptions {
                    filters: vec![FileOperationFilter {
                        scheme: Some("file".to_string()),
                        pattern: FileOperationPattern {
                            glob: "**/*.flx".to_string(),
                            matches: Some(FileOperationPatternKind::File),
                            options: None,
                        },
                    }],
                }),
                ..Default::default()
            }),
        }),
        ..Default::default()
    }
}

/// [`server_capabilities`] serialized to JSON with `typeHierarchyProvider` added.
///
/// `lsp-types` 0.97's [`ServerCapabilities`] predates the type-hierarchy
/// capability and has no typed field for it, so the flag is injected after
/// serialization. VS Code only enables `textDocument/prepareTypeHierarchy` (and
/// the supertypes/subtypes follow-ups) when this top-level flag is advertised.
pub fn server_capabilities_json(encoding: PositionEncoding) -> serde_json::Value {
    let mut value = serde_json::to_value(server_capabilities(encoding))
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "typeHierarchyProvider".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_json_advertises_hierarchy_providers() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        assert_eq!(value["typeHierarchyProvider"], serde_json::json!(true));
        // The typed capabilities still serialize alongside the injected flag.
        assert_eq!(value["callHierarchyProvider"], serde_json::json!(true));
    }

    #[test]
    fn capabilities_advertise_pull_diagnostics() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let provider = &value["diagnosticProvider"];
        assert_eq!(provider["identifier"], serde_json::json!("flux"));
        assert_eq!(provider["interFileDependencies"], serde_json::json!(true));
        assert_eq!(provider["workspaceDiagnostics"], serde_json::json!(true));
    }

    #[test]
    fn capabilities_advertise_range_formatting() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        assert_eq!(
            value["documentRangeFormattingProvider"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn capabilities_advertise_on_type_formatting() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let provider = &value["documentOnTypeFormattingProvider"];
        assert_eq!(provider["firstTriggerCharacter"], serde_json::json!("\n"));
        assert_eq!(provider["moreTriggerCharacter"], serde_json::json!(["}"]));
    }

    #[test]
    fn capabilities_advertise_organize_imports() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let kinds = &value["codeActionProvider"]["codeActionKinds"];
        assert_eq!(
            kinds,
            &serde_json::json!(["quickfix", "source.organizeImports"])
        );
    }

    #[test]
    fn capabilities_advertise_linked_editing() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        assert_eq!(value["linkedEditingRangeProvider"], serde_json::json!(true));
    }

    #[test]
    fn capabilities_advertise_signature_help_retrigger() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let provider = &value["signatureHelpProvider"];
        assert_eq!(provider["triggerCharacters"], serde_json::json!(["(", ","]));
        assert_eq!(
            provider["retriggerCharacters"],
            serde_json::json!([",", ")"])
        );
    }

    #[test]
    fn capabilities_advertise_semantic_tokens_range_and_delta() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let provider = &value["semanticTokensProvider"];
        assert_eq!(provider["range"], serde_json::json!(true));
        assert_eq!(provider["full"]["delta"], serde_json::json!(true));
    }

    #[test]
    fn capabilities_advertise_inlay_hint_resolve() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        assert_eq!(
            value["inlayHintProvider"]["resolveProvider"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn capabilities_advertise_document_links() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        assert_eq!(
            value["documentLinkProvider"]["resolveProvider"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn capabilities_advertise_will_rename_files() {
        let value = server_capabilities_json(PositionEncoding::Utf16);
        let glob =
            &value["workspace"]["fileOperations"]["willRename"]["filters"][0]["pattern"]["glob"];
        assert_eq!(glob, &serde_json::json!("**/*.flx"));
    }
}
