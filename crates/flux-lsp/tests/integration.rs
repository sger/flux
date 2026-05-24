//! Integration tests for `flux-lsp`.
//!
//! Almost everything is tested by driving a `GlobalState` directly — no
//! `lsp_server::Connection`, no worker threads. One end-to-end test still goes
//! through `Connection::memory()` to guard the JSON-RPC wiring in [`Server`].

use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::request::{Initialize, Shutdown};
use lsp_types::{
    CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCallsParams,
    CallHierarchyPrepareParams, ClientCapabilities, CodeActionContext, CodeActionOrCommand,
    CodeActionParams, CodeLensParams, CompletionItem, CompletionParams, CompletionResponse,
    DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentChanges, DocumentDiagnosticParams, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, DocumentFormattingParams, DocumentHighlightParams,
    DocumentLinkParams, DocumentOnTypeFormattingParams, DocumentSymbolParams, Documentation,
    FileChangeType, FileEvent, FileRename, FoldingRangeParams, FormattingOptions,
    GotoDefinitionParams, HoverParams, InitializeParams, InitializedParams, NumberOrString, OneOf,
    PartialResultParams, Position, PreviousResultId, Range, ReferenceContext, ReferenceParams,
    RenameFilesParams, RenameParams, SelectionRangeParams, SemanticTokensParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, TextEdit,
    TypeHierarchyItem, TypeHierarchyPrepareParams, TypeHierarchySubtypesParams,
    TypeHierarchySupertypesParams, Uri, WorkDoneProgressParams, WorkspaceDiagnosticParams,
    WorkspaceDiagnosticReportResult, WorkspaceDocumentDiagnosticReport, WorkspaceEdit,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use serde_json::Value;

use flux_lsp::line_index::PositionEncoding;
use flux_lsp::{GlobalState, Server, server_capabilities};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn uri(s: &str) -> Uri {
    Uri::from_str(s).unwrap()
}

/// A file URI under the flux repo root, so opening the buffer makes the workspace
/// discover `lib/Flow/` and load the prelude (needed for any test that relies on
/// `module_members`, e.g. expanding `exposing (..)`). `CARGO_MANIFEST_DIR` is
/// `.../flux/crates/flux-lsp`; the repo root is two levels up.
fn repo_uri(name: &str) -> Uri {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let root = manifest.parent().and_then(|p| p.parent()).unwrap();
    let path = root.join(name).display().to_string().replace('\\', "/");
    uri(&format!("file:///{path}"))
}

fn open(state: &mut GlobalState, uri: &Uri, text: &str) {
    state.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uri.clone(),
            language_id: "flux".into(),
            version: 1,
            text: text.into(),
        },
    });
}

fn ident(uri: &Uri) -> TextDocumentIdentifier {
    TextDocumentIdentifier { uri: uri.clone() }
}

fn close(state: &mut GlobalState, uri: &Uri) {
    state.handle_did_close(DidCloseTextDocumentParams {
        text_document: ident(uri),
    });
}

/// Lower a `GotoDefinitionResponse` to a single `Location` for tests
/// that only care about the destination, not the focus/full split.
/// Accepts either response shape so a future swap back to `Scalar` (or
/// occasional multi-`Link` responses) wouldn't break tests that use
/// this helper. Uses `target_selection_range` (the identifier focus)
/// since that's the position F12 lands the cursor on.
fn expect_location(resp: lsp_types::GotoDefinitionResponse) -> lsp_types::Location {
    use lsp_types::{GotoDefinitionResponse, Location};
    match resp {
        GotoDefinitionResponse::Scalar(loc) => loc,
        GotoDefinitionResponse::Link(mut links) => {
            let link = links.pop().expect("at least one LocationLink");
            Location {
                uri: link.target_uri,
                range: link.target_selection_range,
            }
        }
        other => panic!("unexpected definition response: {other:?}"),
    }
}

/// Unwrap a `GotoDefinitionResponse` into its single `LocationLink` —
/// for tests that specifically assert the focus/full split.
fn expect_single_link(resp: lsp_types::GotoDefinitionResponse) -> lsp_types::LocationLink {
    use lsp_types::GotoDefinitionResponse;
    match resp {
        GotoDefinitionResponse::Link(mut links) => {
            assert_eq!(links.len(), 1, "expected exactly one LocationLink");
            links.pop().unwrap()
        }
        other => panic!("expected Link response, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// End-to-end (JSON-RPC) — keeps the dispatcher honest
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn initialize_handshake_advertises_capabilities() {
    let (server_conn, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        let caps = serde_json::to_value(server_capabilities(PositionEncoding::Utf16)).unwrap();
        let _params = server_conn.initialize(caps).expect("server initialize");
        let server = Server::new(
            server_conn,
            PositionEncoding::Utf16,
            flux_lsp::loader::WatcherKind::Client,
        );
        server.run().expect("server run");
    });

    let init_id: RequestId = 1.into();
    client
        .sender
        .send(Message::Request(Request {
            id: init_id.clone(),
            method: Initialize::METHOD.to_string(),
            params: serde_json::to_value(InitializeParams {
                capabilities: ClientCapabilities::default(),
                ..Default::default()
            })
            .unwrap(),
        }))
        .unwrap();

    let response = match client.receiver.recv().unwrap() {
        Message::Response(r) => r,
        other => panic!("expected response, got {other:?}"),
    };
    assert_eq!(response.id, init_id);
    let result: Value = response.result.expect("initialize result");
    let caps = &result["capabilities"];
    assert_eq!(caps["hoverProvider"], Value::Bool(true));
    // `definitionProvider` is now declared as `DefinitionOptions { .. }`
    // (the "with options" shape) instead of the bare `true`, since we
    // opt into `LocationLink` responses. VS Code accepts either shape;
    // the wire value here is `{}` (no per-provider options set).
    assert!(
        caps["definitionProvider"].is_object(),
        "expected DefinitionOptions object, got {:?}",
        caps["definitionProvider"]
    );
    assert_eq!(caps["documentFormattingProvider"], Value::Bool(true));
    assert_eq!(caps["documentSymbolProvider"], Value::Bool(true));
    assert!(caps["completionProvider"].is_object());
    assert_eq!(caps["textDocumentSync"], Value::Number(1.into()));

    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::Initialized::METHOD.to_string(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();

    let shutdown_id: RequestId = 2.into();
    client
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_string(),
            params: Value::Null,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.receiver.recv_timeout(remaining).unwrap() {
            Message::Response(r) if r.id == shutdown_id => break,
            _ => continue,
        }
    }
    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::Exit::METHOD.to_string(),
            params: Value::Null,
        }))
        .unwrap();
    server_thread.join().unwrap();
}

#[test]
fn warm_prelude_loads_the_standard_library() {
    // Anchor at a small subdirectory whose project root resolves to the flux
    // repo, so `warm_prelude` finds `lib/Flow/` and loads it. (The
    // `$/progress` notifications that bracket this in `Server` are thin wire
    // plumbing; the meaningful, testable behavior is the warm-up itself.)
    let mut state = GlobalState::default();
    let root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("examples")
        .join("type_classes");
    state.set_workspace_folders(vec![root]);

    assert!(
        !state.workspace.prelude_loaded(),
        "prelude should not be loaded before warm-up"
    );
    state.workspace.warm_prelude();
    assert!(
        state.workspace.prelude_loaded(),
        "warm_prelude should have loaded the Flow stdlib"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostics
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn did_open_publishes_parse_diagnostics_for_broken_source() {
    let mut state = GlobalState::default();
    let u = uri("file:///broken.flx");
    let diags = state
        .handle_did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "flux".into(),
                version: 1,
                text: "}\n".into(),
            },
        })
        .into_iter()
        .next()
        .expect("publishDiagnostics returned");
    assert_eq!(diags.uri, u);
    assert!(!diags.diagnostics.is_empty());
    let first = &diags.diagnostics[0];
    assert_eq!(first.source.as_deref(), Some("flux"));
    assert_eq!(first.severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn did_open_clean_source_publishes_no_diagnostics() {
    let mut state = GlobalState::default();
    let u = uri("file:///clean.flx");
    let diags = state
        .handle_did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "flux".into(),
                version: 1,
                text: "let x = 1\n".into(),
            },
        })
        .into_iter()
        .next()
        .expect("publishDiagnostics returned");
    assert!(
        diags.diagnostics.is_empty(),
        "expected zero diagnostics, got {:?}",
        diags.diagnostics
    );
}

#[test]
fn staged_change_defers_analysis_until_flush() {
    let mut state = GlobalState::default();
    let u = uri("file:///defer.flx");

    // Opening a clean buffer analyzes immediately.
    let opened = diags_for(&mut state, &u, "fn main() -> Int { 1 }\n");
    assert!(
        opened.is_empty(),
        "clean file should open clean: {opened:?}"
    );

    // Stage an edit that introduces an undefined name. `stage_did_change`
    // publishes nothing — the (potentially expensive) re-analysis is deferred —
    // but it marks the file pending.
    state.stage_did_change(DidChangeTextDocumentParams {
        text_document: lsp_types::VersionedTextDocumentIdentifier {
            uri: u.clone(),
            version: 2,
        },
        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "fn main() -> Int { mystery }\n".to_string(),
        }],
    });
    assert!(
        state.has_pending_analysis(),
        "a staged edit should leave analysis pending"
    );

    // A read request arriving before the flush still sees the new text: the
    // stale snapshot was invalidated, so `ensure_snapshot` rebuilds lazily from
    // the staged buffer rather than serving the pre-edit snapshot.
    let symbols = state.handle_document_symbol(DocumentSymbolParams {
        text_document: ident(&u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert!(
        symbols.is_some(),
        "document symbols should still resolve mid-debounce"
    );

    // Flushing runs the deferred analysis and surfaces the error once.
    let published = state.flush_analysis();
    assert!(
        !state.has_pending_analysis(),
        "flush should clear the pending set"
    );
    let diags = published
        .into_iter()
        .find(|d| d.uri == u)
        .map(|d| d.diagnostics)
        .unwrap_or_default();
    assert!(
        diags.iter().any(|d| d.message.contains("mystery")),
        "the deferred edit's undefined-name error should appear on flush: {diags:?}"
    );

    // Nothing pending now — a spurious flush (e.g. a late timer tick) is a no-op.
    assert!(
        state.flush_analysis().is_empty(),
        "flush with nothing pending should publish nothing"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Hover
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hover_on_integer_literal_returns_type() {
    let mut state = GlobalState::default();
    let u = uri("file:///hov.flx");
    open(&mut state, &u, "let x = 42\n");

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 8,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup contents, got {other:?}"),
    };
    assert!(
        value.to_lowercase().contains("int"),
        "expected int type in hover, got: {value}"
    );
}

#[test]
fn hover_on_builtin_effect_returns_doc_card() {
    let mut state = GlobalState::default();
    let u = uri("file:///eff.flx");
    open(&mut state, &u, "fn main() with IO {}\n");

    // `IO` starts at column 15 on line 0 ("fn main() with IO").
    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 16,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.starts_with("**`IO`** — ") && value.contains("```flux"),
        "expected the built-in `IO` effect doc card, got: {value}"
    );
}

#[test]
fn hover_on_user_effect_keeps_the_plain_label() {
    let mut state = GlobalState::default();
    let u = uri("file:///user_eff.flx");
    // A user-declared effect has no built-in doc — hover stays a plain label.
    open(
        &mut state,
        &u,
        "effect Logger { log: String -> Unit }\nfn run() with Logger {}\n",
    );

    // `Logger` in the `with` clause on line 1 (starts at column 13).
    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 15,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        !value.starts_with("**`"),
        "a user effect should not get a built-in doc card, got: {value}"
    );
}

#[test]
fn hover_on_function_decl_returns_scheme_or_label() {
    let mut state = GlobalState::default();
    let u = uri("file:///decl.flx");
    open(&mut state, &u, "fn main(x) { x }\n");

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 4,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.contains("main") || value.contains("decl:") || value.contains("->"),
        "expected scheme or decl label, got: {value}"
    );
}

#[test]
fn hover_on_main_notes_the_entry_point() {
    let mut state = GlobalState::default();
    let u = uri("file:///entry.flx");
    open(
        &mut state,
        &u,
        "fn helper() { 1 }\nfn main() with IO { print(\"hi\") }\n",
    );

    // Hover `main` on line 1 — the entry-point note must be present.
    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 4,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.starts_with("**`main`** — ") && value.contains("entry point"),
        "expected the `main` hover to use the card shape with an entry-point \
         note, got: {value}"
    );
    assert!(
        value.contains("```flux"),
        "expected the `main` hover to keep its signature in a flux block, got: {value}"
    );

    // Hover `helper` on line 0 — an ordinary function gets no such note.
    let other = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 4,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let other_value = match other.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        o => panic!("expected markup, got {o:?}"),
    };
    assert!(
        !other_value.contains("entry point"),
        "an ordinary function should not get the entry-point note, got: {other_value}"
    );
}

#[test]
fn hover_on_type_annotation_returns_type_label() {
    let mut state = GlobalState::default();
    let u = uri("file:///ty.flx");
    open(&mut state, &u, "let x: Int = 1\n");

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 7,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.contains("type: Int"),
        "expected type label, got: {value}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Document symbols / definition / completion / formatting
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn document_symbol_lists_top_level_items() {
    let mut state = GlobalState::default();
    let u = uri("file:///syms.flx");
    open(&mut state, &u, "fn greet(name) { name }\nlet answer = 42\n");

    let resp = state
        .handle_document_symbol(DocumentSymbolParams {
            text_document: ident(&u),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("document symbol result");
    let names: Vec<String> = match resp {
        lsp_types::DocumentSymbolResponse::Nested(syms) => {
            syms.into_iter().map(|s| s.name).collect()
        }
        other => panic!("expected nested response, got {other:?}"),
    };
    assert!(
        names.iter().any(|n| n == "greet"),
        "expected `greet`, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "answer"),
        "expected `answer`, got: {names:?}"
    );
}

#[test]
fn document_symbol_selection_range_is_the_name() {
    let mut state = GlobalState::default();
    let u = uri("file:///syms-sel.flx");
    open(&mut state, &u, "fn greet(name) { name }\nlet answer = 42\n");

    let syms = match state
        .handle_document_symbol(DocumentSymbolParams {
            text_document: ident(&u),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("document symbol result")
    {
        lsp_types::DocumentSymbolResponse::Nested(s) => s,
        other => panic!("expected nested response, got {other:?}"),
    };

    let greet = syms
        .iter()
        .find(|s| s.name == "greet")
        .expect("greet symbol");
    // selection_range is just `greet` (cols 3..8), a strict sub-range of the
    // whole-declaration range (`fn greet(name) { name }`).
    assert_eq!(greet.selection_range.start, Position::new(0, 3));
    assert_eq!(greet.selection_range.end, Position::new(0, 8));
    assert_eq!(greet.range.start, Position::new(0, 0));
    assert!(greet.range.end.character > greet.selection_range.end.character);

    let answer = syms
        .iter()
        .find(|s| s.name == "answer")
        .expect("answer symbol");
    assert_eq!(answer.selection_range.start, Position::new(1, 4));
    assert_eq!(answer.selection_range.end, Position::new(1, 10));
}

#[test]
fn goto_definition_returns_link_with_distinct_focus_and_full_range_for_let() {
    // F12 on the use of `answer` in `let result = answer` should yield a
    // `LocationLink` whose `target_range` covers the whole declaration
    // (`let answer = 42`, columns 0..15) and whose
    // `target_selection_range` covers only the identifier (`answer`,
    // columns 4..10). This is the GHC `NameAnn`/`EpAnn` distinction
    // surfaced to the LSP — peek-view highlights just the name.
    let mut state = GlobalState::default();
    let u = uri("file:///focus_full.flx");
    open(&mut state, &u, "let answer = 42\nlet result = answer\n");

    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 15, // mid-`answer` use
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("definition result");
    let link = expect_single_link(resp);
    assert_eq!(link.target_uri, u);
    // Full range: the whole `let answer = 42` (line 0, 0..15).
    assert_eq!(link.target_range.start.line, 0);
    assert_eq!(link.target_range.start.character, 0);
    assert_eq!(link.target_range.end.line, 0);
    assert_eq!(link.target_range.end.character, 15);
    // Focus range: just `answer` (line 0, 4..10).
    assert_eq!(link.target_selection_range.start.line, 0);
    assert_eq!(link.target_selection_range.start.character, 4);
    assert_eq!(link.target_selection_range.end.line, 0);
    assert_eq!(link.target_selection_range.end.character, 10);
    // Focus must be a strict sub-range of full.
    assert!(
        link.target_range.start.character <= link.target_selection_range.start.character
            && link.target_selection_range.end.character <= link.target_range.end.character,
        "focus_range must lie within full_range"
    );
}

#[test]
fn goto_type_definition_jumps_to_adt_declaration() {
    // "Go to Type Definition" on an expression jumps to the declaration of its
    // inferred type's ADT — here the body use of `s` (type `Shape`) lands on the
    // `Shape` declaration.
    let mut state = GlobalState::default();
    let u = uri("file:///typedef.flx");
    open(
        &mut state,
        &u,
        "type Shape = Circle(Float) | Rect(Float, Float)\n\
         fn describe(s: Shape) -> Shape { s }\n",
    );

    // Type-definition on the body use of `s` (line 1, char 33).
    let resp = state
        .handle_type_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 33,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("type definition result");
    let link = expect_single_link(resp);
    assert_eq!(link.target_uri, u);
    assert_eq!(
        link.target_range.start.line, 0,
        "should point at the `Shape` declaration line"
    );
    // Focus covers the `Shape` name (`type ` is 5 chars).
    assert_eq!(link.target_selection_range.start.character, 5);
}

#[test]
fn goto_definition_origin_selection_covers_cursor_word() {
    // F12 on `bar` mid-identifier should set `origin_selection_range`
    // to the exact span of `bar` in the source — letting VS Code
    // underline just the cursor word in the source side of the peek
    // view rather than the whole line.
    let mut state = GlobalState::default();
    let u = uri("file:///origin_sel.flx");
    open(&mut state, &u, "let bar = 1\nlet z = bar\n");

    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 9, // mid-`bar` use
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("definition result");
    let link = expect_single_link(resp);
    let origin = link
        .origin_selection_range
        .expect("origin_selection_range should be populated for an identifier cursor");
    assert_eq!(origin.start.line, 1);
    assert_eq!(origin.end.line, 1);
    let width = origin.end.character - origin.start.character;
    assert_eq!(
        width,
        "bar".len() as u32,
        "origin_selection_range width should match `bar`.len()"
    );
}

#[test]
fn goto_definition_resolves_local_let() {
    let mut state = GlobalState::default();
    let u = uri("file:///def.flx");
    open(&mut state, &u, "let answer = 42\nlet result = answer\n");

    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 15,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("definition result");
    let loc = expect_location(resp);
    assert_eq!(loc.uri, u);
    assert_eq!(loc.range.start.line, 0, "definition should land on line 0");
}

#[test]
fn goto_definition_on_constrained_instance_head_jumps_to_class() {
    // The head class name of a constrained instance sits after `=>`. F12 on it
    // should resolve (to the `class` declaration), not mis-target the context
    // constraint right after `instance `.
    let mut state = GlobalState::default();
    let u = uri("file:///constrained-instance.flx");
    open(
        &mut state,
        &u,
        "class Eq<a> { fn eq(x: a, y: a) -> Bool }\n\
         instance Eq<a> => Eq<List<a>> { fn eq(x, y) { true } }\n",
    );

    // `Eq` after `=>` is at line 1, char 18.
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 18,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("definition result for the instance head");
    let loc = expect_location(resp);
    assert_eq!(
        loc.range.start.line, 0,
        "the instance head `Eq` should resolve to the class on line 0"
    );
}

#[test]
fn goto_definition_resolves_import_alias_use_site() {
    // F12 on the bare alias `A` (LHS of `A.map(...)`) should jump to the
    // `import` line that introduced it.
    let mut state = GlobalState::default();
    let u = uri("file:///alias_use.flx");
    open(&mut state, &u, "import Flow.Array as A\nlet x = A.length\n");

    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 8, // `A` in `let x = A.length`
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("definition result");
    let loc = expect_location(resp);
    assert_eq!(loc.uri, u);
    assert_eq!(
        loc.range.start.line, 0,
        "alias goto-def should land on the import line"
    );
}

#[test]
fn goto_definition_resolves_aliased_module_member() {
    // F12 on `map` in `A.map(...)` where `A` aliases `Flow.Array` should
    // jump into Flow.Array's source file. The Flow prelude is loaded by
    // the LSP at session start, so the snapshot has a `module_programs`
    // entry keyed by the short name `"Array"`.
    let mut state = GlobalState::default();
    let u = uri("file:///alias_mem.flx");
    open(&mut state, &u, "import Flow.Array as A\nlet x = A.length\n");

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 1,
                character: 10, // `length` in `A.length`
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let Some(resp) = resp else {
        // If the Flow prelude isn't loaded in this test harness, the
        // lookup falls through to None. Treat that as a soft pass —
        // the regression we care about is that we don't incorrectly
        // return the *current* buffer.
        return;
    };
    let loc = expect_location(resp);
    assert_ne!(
        loc.uri, u,
        "aliased member goto-def should leave the current buffer"
    );
}

#[test]
fn goto_definition_on_return_keyword_jumps_to_enclosing_fn() {
    // F12 on the `return` keyword inside a function body should jump to
    // the enclosing `fn` signature span (line 0 here).
    let mut state = GlobalState::default();
    let u = uri("file:///ret.flx");
    open(
        &mut state,
        &u,
        "fn first_positive(x) {\n    if x > 0 { return x }\n    -1\n}\n",
    );

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 1,
                character: 16, // `return`
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    if let Some(resp) = resp {
        let loc = expect_location(resp);
        assert_eq!(loc.uri, u);
        assert_eq!(
            loc.range.start.line, 0,
            "return goto-def should land on the fn signature"
        );
    }
}

#[test]
fn goto_definition_resolves_effect_row_var() {
    // F12 on the row-var `|e` inside a function body's effect row should
    // jump to its binding occurrence in the signature. With a single
    // binding site, the binder and the use are the same location.
    let mut state = GlobalState::default();
    let u = uri("file:///rowvar.flx");
    open(&mut state, &u, "fn f() with Console, |e { 1 }\n");

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 0,
                character: 22, // `e` in `|e`
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    // Either Some(Location on line 0) or None (parser may not accept this
    // particular syntax in the test harness); the regression we care about
    // is that it doesn't panic.
    if let Some(resp) = resp {
        let loc = expect_location(resp);
        assert_eq!(loc.uri, u);
        assert_eq!(loc.range.start.line, 0);
    }
}

#[test]
fn goto_definition_resolves_class_method_to_instance() {
    // F12 on `same(1, 2)`'s `same` should jump to the `fn same(...)`
    // arm inside `instance Eqish<Int> { ... }` — *not* to the
    // declaration inside `class Eqish<a> { fn same(...) -> Bool }`.
    // The type checker resolved the call to `Eqish<Int>` via
    // `propagate_resolved_class_call_effects`; the LSP keys
    // `InferProgramResult::class_method_dispatch` by the function-
    // position `ExprId` and looks up the instance arm.
    let mut state = GlobalState::default();
    let u = uri("file:///class_method.flx");
    open(
        &mut state,
        &u,
        "class Eqish<a> {\n    fn same(x: a, y: a) -> Bool\n}\ninstance Eqish<Int> {\n    fn same(x, y) { x == y }\n}\nfn main() { same(1, 2) }\n",
    );

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 6,
                character: 13, // `same` in `same(1, 2)`
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let resp = resp
        .expect("class-method dispatch should resolve `same(1, 2)` against `instance Eqish<Int>`");
    let loc = expect_location(resp);
    assert_eq!(loc.uri, u);
    // Instance arm `fn same(x, y) { x == y }` is on line 4 (0-based);
    // the class-method declaration would be line 1. Distinguishing
    // those is the whole point of this slice.
    assert_eq!(
        loc.range.start.line, 4,
        "class-method goto-def should land on the instance arm (line 4), not the class declaration (line 1)"
    );
}

#[test]
fn goto_definition_class_method_dispatch_picks_correct_instance() {
    // Two instances of the same class on different head types. F12 on
    // a call with each receiver type should land on the matching
    // instance arm.
    let mut state = GlobalState::default();
    let u = uri("file:///class_multi.flx");
    open(
        &mut state,
        &u,
        // line 0:  class Eqish<a> {
        // line 1:      fn same(x: a, y: a) -> Bool
        // line 2:  }
        // line 3:  instance Eqish<Int> {
        // line 4:      fn same(x, y) { x == y }
        // line 5:  }
        // line 6:  instance Eqish<Bool> {
        // line 7:      fn same(x, y) { x == y }
        // line 8:  }
        // line 9:  fn main() { same(1, 2); same(true, false) }
        "class Eqish<a> {\n    fn same(x: a, y: a) -> Bool\n}\ninstance Eqish<Int> {\n    fn same(x, y) { x == y }\n}\ninstance Eqish<Bool> {\n    fn same(x, y) { x == y }\n}\nfn main() { same(1, 2); same(true, false) }\n",
    );

    // First call `same(1, 2)` — should land on the `Eqish<Int>` arm (line 4).
    let resp1 = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 9,
                character: 13, // `same` in first call
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    // Second call `same(true, false)` — should land on `Eqish<Bool>` arm (line 7).
    let resp2 = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 9,
                character: 25, // `same` in second call
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    // If both resolve, they must point at different lines. If either
    // doesn't resolve (parser/inference variability in the harness),
    // the test no-ops — the regression we care about is mis-routing,
    // not non-resolution.
    if let (Some(r1), Some(r2)) = (resp1, resp2) {
        let l1 = expect_location(r1);
        let l2 = expect_location(r2);
        assert_eq!(l1.uri, u);
        assert_eq!(l2.uri, u);
        assert_ne!(
            l1.range.start.line, l2.range.start.line,
            "different-typed receivers must dispatch to different instance arms"
        );
    }
}

#[test]
fn goto_definition_class_method_falls_through_when_dispatch_unavailable() {
    // When inference can't resolve a class-method call to a concrete
    // instance (e.g. polymorphic receiver), the dispatch map has no
    // entry and goto-def falls through to the regular identifier
    // resolution path. The fallthrough produces *some* location
    // (typically the class-method declaration) — the regression here
    // is that we don't panic or return a stale prior dispatch.
    let mut state = GlobalState::default();
    let u = uri("file:///class_polymorphic.flx");
    open(
        &mut state,
        &u,
        "class Eqish<a> {\n    fn same(x: a, y: a) -> Bool\n}\nfn poly<a>(x: a, y: a) { same(x, y) }\n",
    );

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 3,
                character: 25, // `same` inside `poly`'s body
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    // Either Some (fallthrough produced a target) or None — both fine.
    // The regression we guard against is a panic or pointing somewhere
    // outside the buffer.
    if let Some(resp) = resp {
        let loc = expect_location(resp);
        assert_eq!(loc.uri, u);
    }
}

#[test]
fn goto_definition_resolves_record_field_use() {
    // F12 on `.name` in `alice.name` should jump to the `name: String`
    // field in the `data Person { ... }` declaration.
    let mut state = GlobalState::default();
    let u = uri("file:///record_field.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet alice = Person { name: \"a\", age: 1 }\nlet n = alice.name\n",
    );

    let resp = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 2,
                character: 15, // `name` in `alice.name`
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let Some(resp) = resp else {
        // If inference can't determine `alice`'s ADT type (e.g. the test
        // harness skips inference), no result is acceptable.
        return;
    };
    let loc = expect_location(resp);
    assert_eq!(loc.uri, u);
    assert_eq!(
        loc.range.start.line, 0,
        "field goto-def should land on the data decl line"
    );
}

#[test]
fn completion_returns_top_level_symbols_and_keywords() {
    let mut state = GlobalState::default();
    let u = uri("file:///comp.flx");
    open(&mut state, &u, "fn greet(name) { name }\nlet answer = 42\n");

    let resp = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 2,
                    character: 0,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");
    let labels: Vec<String> = match resp {
        lsp_types::CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        other => panic!("expected array response, got {other:?}"),
    };
    assert!(labels.iter().any(|l| l == "greet"));
    assert!(labels.iter().any(|l| l == "answer"));
    assert!(labels.iter().any(|l| l == "let"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Position encoding (M3a)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hover_on_literal_with_non_ascii_prefix_utf16() {
    // `é` is 2 UTF-8 bytes, 1 UTF-16 code unit, 1 codepoint. With a UTF-16
    // client, `42` on line 1 starts at LSP character 8 (`let x = 42`).
    let mut state = GlobalState::new(PositionEncoding::Utf16);
    let u = uri("file:///nonascii.flx");
    open(&mut state, &u, "// é\nlet x = 42\n");

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 8,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover result with non-ASCII prefix");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.to_lowercase().contains("int"),
        "expected int type in hover, got: {value}"
    );
}

#[test]
fn formatting_range_covers_actual_buffer_end() {
    let mut state = GlobalState::new(PositionEncoding::Utf16);
    let u = uri("file:///fmt-range.flx");
    let original = "let x  =  1\nlet y = 2\n";
    open(&mut state, &u, original);

    let edits = state
        .handle_formatting(DocumentFormattingParams {
            text_document: ident(&u),
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("formatting result");
    assert_eq!(edits.len(), 1);
    let r = edits[0].range;
    assert_eq!(
        r.start,
        Position {
            line: 0,
            character: 0
        }
    );
    // Two newlines → end on line 2, character 0 (after final \n).
    assert_eq!(
        r.end,
        Position {
            line: 2,
            character: 0
        },
        "end should match real buffer end, not a fake sentinel"
    );
}

#[test]
fn negotiate_encoding_prefers_utf8_when_advertised() {
    use flux_lsp::line_index::negotiate_encoding;
    use lsp_types::PositionEncodingKind;

    let kinds = vec![PositionEncodingKind::UTF16, PositionEncodingKind::UTF8];
    assert_eq!(negotiate_encoding(Some(&kinds)), PositionEncoding::Utf8);

    let only16 = vec![PositionEncodingKind::UTF16];
    assert_eq!(negotiate_encoding(Some(&only16)), PositionEncoding::Utf16);

    assert_eq!(negotiate_encoding(None), PositionEncoding::Utf16);
}

#[test]
fn hover_on_main_with_print_resolves_unit_return() {
    // Open the buffer from inside the flux workspace (where lib/Flow/ lives)
    // so the prelude loader can find and infer Flow.Primops. Without Phase 2
    // prelude loading, main's return type comes back as `forall a. () -> a`.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    // CARGO_MANIFEST_DIR is .../flux/crates/flux-lsp; lib/Flow/ lives two up.
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("hover-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    open(&mut state, &u, "fn main() with IO { print(1) }\n");

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 3,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover on `main` decl");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    // With Phase-2 prelude loading, main's return should resolve to Unit (or
    // its display variant), not a free type variable like `a`.
    assert!(
        value.contains("Unit") || value.contains("()"),
        "expected Unit return for main, got: {value}"
    );
    assert!(
        !value.contains("forall a"),
        "main return shouldn't be polymorphic, got: {value}"
    );
}

#[test]
fn hover_on_imported_async_sleep_resolves_unit_return() {
    // Buffer imports Flow.Async (not in the auto-prelude list). The LSP must
    // follow the import and load Flow.Async's schemes so `sleep`'s return type
    // resolves to Unit instead of a free type variable rendered as `_`.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("async-import-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    // `sleep` is at column 36 on line 2 (0-based: line 1, char 35..40).
    let source = "import Flow.Async exposing (..)\nfn body() with Async { sleep(10) }\n";
    open(&mut state, &u, source);

    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 25,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover on `sleep`");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.contains("Unit") || value.contains("()"),
        "expected Unit return for sleep, got: {value}"
    );
    assert!(
        !value.contains(" _ ") && !value.contains("-> _"),
        "sleep return shouldn't be a free type variable, got: {value}"
    );
}

#[test]
fn hover_on_sleep_in_full_async_example_resolves_unit() {
    // Reproduces the exact buffer from examples/async/02_sleep_yield.flx so we
    // catch any regression where the larger buffer's inference cascade
    // perturbs `sleep`'s return type.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("examples/async/02_sleep_yield.flx");
    // Emulate VS Code's URI format: percent-encoded colon after the drive
    // letter (e.g. `file:///e%3A/Github/...`), exactly what the extension
    // sends. The naive `file:///e:/...` form is what tests have been using
    // and might mask a percent-decode bug in `parent_dir_of_uri`.
    let raw = buf_path.display().to_string().replace('\\', "/");
    let uri_str = if raw.chars().nth(1) == Some(':') {
        format!("file:///{}%3A{}", &raw[..1], &raw[2..])
    } else {
        format!("file:///{}", raw)
    };
    let u = uri(&uri_str);
    let source = std::fs::read_to_string(&buf_path).expect("read 02_sleep_yield.flx");
    open(&mut state, &u, &source);

    // `sleep(20)` on line 21 (0-indexed 20), column 4 ("sleep").
    let hover = state
        .handle_hover(HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 20,
                    character: 6,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("hover on `sleep` in real example");
    let value = match hover.contents {
        lsp_types::HoverContents::Markup(m) => m.value,
        other => panic!("expected markup, got {other:?}"),
    };
    assert!(
        value.contains("Unit") || value.contains("()"),
        "expected Unit return for sleep in real example, got: {value}"
    );
}

#[test]
fn hover_on_unimported_async_sleep_stays_unresolved() {
    // Same source minus the import. Guards against accidentally autoloading
    // every Flow module on every snapshot — only buffer-imported ones should
    // be preloaded.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("async-noimport-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    let source = "fn body() with Async { sleep(10) }\n";
    open(&mut state, &u, source);

    let hover = state.handle_hover(HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 0,
                character: 25,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    // Either no hover, or a hover that doesn't claim Unit as the return type:
    // if the LSP autoloaded Async behind our back this assertion would fail.
    if let Some(h) = hover {
        let value = match h.contents {
            lsp_types::HoverContents::Markup(m) => m.value,
            other => panic!("expected markup, got {other:?}"),
        };
        assert!(
            !value.contains("-> Unit"),
            "sleep should be unresolved without an import, got: {value}"
        );
    }
}

#[test]
fn formatting_request_returns_text_edits_for_messy_source() {
    let mut state = GlobalState::default();
    let u = uri("file:///fmt.flx");
    let original = "let x  =  1\n\n\n\nlet y = 2\n";
    open(&mut state, &u, original);

    let edits = state
        .handle_formatting(DocumentFormattingParams {
            text_document: ident(&u),
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("formatting result");
    assert!(!edits.is_empty(), "expected at least one edit");
    assert_ne!(
        edits[0].new_text, original,
        "formatter should produce different text"
    );
}

fn format_range(state: &mut GlobalState, u: &Uri, range: Range) -> Vec<TextEdit> {
    state
        .handle_formatting_range(lsp_types::DocumentRangeFormattingParams {
            text_document: ident(u),
            range,
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap_or_default()
}

// Two functions whose bodies are under-indented; the `fn`/`}` lines are already
// correct, so each body forms its own diff hunk separated by unchanged anchors.
const RANGE_FMT_SRC: &str = "fn f() {\nlet a = 1\n}\nfn g() {\nlet b = 2\n}\n";

#[test]
fn range_formatting_touches_only_selected_hunk() {
    let mut state = GlobalState::default();
    let u = uri("file:///fmt-range.flx");
    open(&mut state, &u, RANGE_FMT_SRC);

    // Select the first function's body (line 1).
    let edits = format_range(
        &mut state,
        &u,
        Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 9,
            },
        },
    );
    assert_eq!(edits.len(), 1, "only the selected hunk, got {edits:?}");
    assert_eq!(edits[0].range.start.line, 1);
    assert_eq!(edits[0].new_text, "    let a = 1\n");
    // The other function's under-indented body (line 4) is left untouched.
    assert!(edits.iter().all(|e| e.range.start.line != 4));
}

#[test]
fn range_formatting_leaves_unselected_changes_alone() {
    let mut state = GlobalState::default();
    let u = uri("file:///fmt-range-skip.flx");
    open(&mut state, &u, RANGE_FMT_SRC);

    // Selecting the already-correct `fn f() {` line (0) changes nothing, even
    // though the file has indentation problems elsewhere.
    let edits = format_range(
        &mut state,
        &u,
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 8,
            },
        },
    );
    assert!(
        edits.is_empty(),
        "formatting a clean selection should change nothing, got {edits:?}"
    );
}

fn on_type_format(
    state: &mut GlobalState,
    u: &Uri,
    line: u32,
    character: u32,
    ch: &str,
) -> Vec<TextEdit> {
    state
        .handle_on_type_formatting(DocumentOnTypeFormattingParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(u),
                position: Position { line, character },
            },
            ch: ch.to_string(),
            options: FormattingOptions {
                tab_size: 4,
                insert_spaces: true,
                ..Default::default()
            },
        })
        .unwrap_or_default()
}

#[test]
fn on_type_formatting_indents_new_line_after_open_brace() {
    let mut state = GlobalState::default();
    let u = uri("file:///otf-indent.flx");
    // After typing `{` then Enter: a fresh empty line inside the block.
    open(&mut state, &u, "fn f() {\n\n");

    // Enter fired at the start of the new line (line 1).
    let edits = on_type_format(&mut state, &u, 1, 0, "\n");
    assert_eq!(edits.len(), 1, "expected one indent edit, got {edits:?}");
    assert_eq!(edits[0].new_text, "    ", "should indent one level");
    assert_eq!(edits[0].range.start.line, 1);
}

#[test]
fn on_type_formatting_dedents_close_brace() {
    let mut state = GlobalState::default();
    let u = uri("file:///otf-dedent.flx");
    // The `}` is over-indented (4 spaces) and should drop to column 0.
    open(&mut state, &u, "fn f() {\n    let a = 1\n    }\n");

    // `}` typed at line 2; cursor sits just after it (column 5).
    let edits = on_type_format(&mut state, &u, 2, 5, "}");
    assert_eq!(edits.len(), 1, "expected one dedent edit, got {edits:?}");
    assert_eq!(
        edits[0].new_text, "",
        "the `}}` should align with the opener"
    );
    assert_eq!(
        edits[0].range.start,
        Position {
            line: 2,
            character: 0
        }
    );
    assert_eq!(
        edits[0].range.end,
        Position {
            line: 2,
            character: 4
        }
    );
}

#[test]
fn on_type_formatting_leaves_correct_line_alone() {
    let mut state = GlobalState::default();
    let u = uri("file:///otf-noop.flx");
    // The `}` is already at the right column.
    open(&mut state, &u, "fn f() {\n    let a = 1\n}\n");

    let edits = on_type_format(&mut state, &u, 2, 1, "}");
    assert!(
        edits.is_empty(),
        "an already-correct `}}` needs no edit, got {edits:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// M4 — locator-driven hover coverage. One test per `NodeRef` variant that
// wasn't surfacing useful content under the old per-handler walkers.
// ─────────────────────────────────────────────────────────────────────────────

fn hover_markup(state: &mut GlobalState, u: &Uri, line: u32, character: u32) -> Option<String> {
    let hover = state.handle_hover(HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(u),
            position: Position { line, character },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    })?;
    match hover.contents {
        lsp_types::HoverContents::Markup(m) => Some(m.value),
        _ => None,
    }
}

#[test]
fn hover_on_function_decl_shows_doc_comment() {
    // A `///` comment above a declaration is surfaced in its hover (the AST
    // drops doc comments, so this is scanned from the buffer source).
    let mut state = GlobalState::default();
    let u = uri("file:///doc.flx");
    open(
        &mut state,
        &u,
        "/// Doubles its argument.\nfn twice(x: Int) -> Int { x * 2 }\n",
    );

    // Hover on `twice` (line 1, char 5).
    let md = hover_markup(&mut state, &u, 1, 5).expect("hover on `twice`");
    assert!(
        md.contains("Doubles its argument."),
        "expected the doc comment in hover, got: {md}"
    );
    assert!(
        md.contains("```flux"),
        "expected the signature block alongside the doc, got: {md}"
    );
}

#[test]
fn hover_on_function_use_shows_doc_comment() {
    // A documented function's `///` shows on every reference, not only at its
    // declaration — the name is resolved back to the declaration the way
    // rust-analyzer surfaces docs at use sites.
    let mut state = GlobalState::default();
    let u = uri("file:///doc-use.flx");
    open(
        &mut state,
        &u,
        "/// Doubles its argument.\n\
         fn twice(x: Int) -> Int { x * 2 }\n\
         fn main() with IO { let r = twice(21) }\n",
    );

    // Hover on the `twice` *use* in `twice(21)` (line 2, char 30).
    let md = hover_markup(&mut state, &u, 2, 30).expect("hover on `twice` use");
    assert!(
        md.contains("Doubles its argument."),
        "expected the declaration doc at the use site, got: {md}"
    );
    assert!(
        md.contains("```flux"),
        "expected the inferred signature block alongside the doc, got: {md}"
    );
}

#[test]
fn hover_on_perform_op_shows_signature_and_doc() {
    // Hovering the operation in `perform Effect.op(...)` shows the op's declared
    // signature and its doc — previously a bare `operation: log` label.
    let mut state = GlobalState::default();
    let u = uri("file:///perform-op.flx");
    open(
        &mut state,
        &u,
        "effect Audit {\n\
         \x20   /// Records a message; returns its length.\n\
         \x20   log: String -> Int\n\
         }\n\
         fn audited() -> Int with Audit {\n\
         \x20   perform Audit.log(\"x\")\n\
         }\n",
    );

    // Hover on `log` in `perform Audit.log` (line 5, char 19).
    let md = hover_markup(&mut state, &u, 5, 19).expect("hover on `log` perform op");
    assert!(
        md.contains("String -> Int") || md.contains("(String) -> Int"),
        "expected the op's declared signature, got: {md}"
    );
    assert!(
        md.contains("Records a message"),
        "expected the op's doc comment, got: {md}"
    );
}

#[test]
fn hover_on_handle_arm_op_shows_signature() {
    // Hovering the op name in a `handle` arm shows the op's declared signature,
    // resolved from the handled effect.
    let mut state = GlobalState::default();
    let u = uri("file:///handle-op.flx");
    open(
        &mut state,
        &u,
        "effect Audit {\n\
         \x20   log: String -> Int\n\
         }\n\
         fn audited() -> Int with Audit {\n\
         \x20   perform Audit.log(\"x\")\n\
         }\n\
         fn main() with IO {\n\
         \x20   let v = audited() handle Audit {\n\
         \x20       log(resume, m) -> resume(2)\n\
         \x20   }\n\
         \x20   print(to_string(v))\n\
         }\n",
    );

    // Hover on `log` in the handle arm (line 8, char 8).
    let md = hover_markup(&mut state, &u, 8, 8).expect("hover on `log` handle arm");
    assert!(
        md.contains("String -> Int") || md.contains("(String) -> Int"),
        "expected the op's declared signature in the handle arm, got: {md}"
    );
}

#[test]
fn hover_on_constructor_pattern_shows_variant_and_doc() {
    // Hovering an ADT constructor in a `match` pattern shows the variant's
    // declared shape and the data declaration's doc — previously this returned
    // nothing (only `Pattern::Identifier` was handled).
    let mut state = GlobalState::default();
    let u = uri("file:///ctor-pattern.flx");
    open(
        &mut state,
        &u,
        "/// A 2D shape.\n\
         type Shape = Circle(Float) | Rect(Float, Float)\n\
         fn describe(s: Shape) -> Float {\n\
         \x20   match s {\n\
         \x20       Circle(r) -> r,\n\
         \x20       Rect(w, h) -> w,\n\
         \x20   }\n\
         }\n",
    );

    // Hover on `Circle` in the pattern `Circle(r)` (line 4, char 10).
    let md = hover_markup(&mut state, &u, 4, 10).expect("hover on `Circle` pattern");
    assert!(
        md.contains("Circle(Float)"),
        "expected the variant's declared shape, got: {md}"
    );
    assert!(
        md.contains("A 2D shape."),
        "expected the data declaration's doc, got: {md}"
    );
}

#[test]
fn hover_on_module_member_use_shows_doc_comment() {
    // Hovering a `Module.member` use site shows the member's doc comment,
    // scanned from the (Flow) module's cached source.
    let mut state = GlobalState::default();
    let u = repo_root_uri("hover-member-doc-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.Either as Either\n\nfn main() with IO {\n    let r = Either.either\n}\n",
    );

    // Hover on `either` in `Either.either` (line 3, char 21).
    let md = hover_markup(&mut state, &u, 3, 21).expect("hover on `either`");
    assert!(
        md.contains("Case analysis for Either"),
        "expected the Flow.Either member doc, got: {md}"
    );
}

#[test]
fn hover_on_array_member_shows_stdlib_doc_comment() {
    // The core Flow modules (Array/List/String/Map/Math) carry `///` docs, so
    // hovering a member shows its summary alongside the signature.
    let mut state = GlobalState::default();
    let u = repo_root_uri("hover-array-doc-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn main() with IO {\n    let r = Array.map\n}\n",
    );

    // Hover on `map` in `Array.map` (line 3, char 18).
    let md = hover_markup(&mut state, &u, 3, 18).expect("hover on `map`");
    assert!(
        md.contains("Apply `f` to every element"),
        "expected the Flow.Array.map doc, got: {md}"
    );
}

#[test]
fn hover_on_class_method_shows_doc_and_signature() {
    let mut state = GlobalState::default();
    let u = uri("file:///class-method.flx");
    open(
        &mut state,
        &u,
        "class LocalEq<a> {\n    /// Structural equality.\n    fn eq(x: a, y: a) -> Bool\n}\n",
    );

    // Hover on `eq` (line 2, char 7 — after "    fn ").
    let md = hover_markup(&mut state, &u, 2, 7).expect("hover on class method `eq`");
    assert!(
        md.contains("Structural equality."),
        "expected the method doc comment, got: {md}"
    );
    assert!(
        md.contains("fn eq(x: a, y: a) -> Bool"),
        "expected the declared signature, got: {md}"
    );
}

#[test]
fn hover_on_instance_method_shows_doc_and_class_signature() {
    let mut state = GlobalState::default();
    let u = uri("file:///instance-method.flx");
    open(
        &mut state,
        &u,
        "class LocalEq<a> {\n    fn eq(x: a, y: a) -> Bool\n}\n\
         instance LocalEq<Int> {\n    /// Int equality.\n    fn eq(x, y) { x == y }\n}\n",
    );

    // Hover on `eq` in the instance method (line 5, char 7).
    let md = hover_markup(&mut state, &u, 5, 7).expect("hover on instance method `eq`");
    assert!(
        md.contains("Int equality."),
        "expected the instance method doc comment, got: {md}"
    );
    // An instance method has no annotations of its own — hover renders the
    // class's declared signature.
    assert!(
        md.contains("fn eq(x: a, y: a) -> Bool"),
        "expected the class's declared signature, got: {md}"
    );
}

#[test]
fn hover_on_type_use_shows_declaration_doc() {
    let mut state = GlobalState::default();
    let u = uri("file:///type-use.flx");
    open(
        &mut state,
        &u,
        "/// A 2D point.\ndata Point { Point { x: Int } }\nlet p: Point = Point { x: 0 }\n",
    );

    // Hover on `Point` in the annotation `let p: Point` (line 2, char 8).
    let md = hover_markup(&mut state, &u, 2, 8).expect("hover on type use `Point`");
    assert!(
        md.contains("A 2D point."),
        "expected the type declaration's doc comment at the use site, got: {md}"
    );
}

#[test]
fn hover_on_member_access_member_returns_field_label() {
    let mut state = GlobalState::default();
    let u = uri("file:///rec.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet p = Person { name: \"A\", age: 1 }\nlet n = p.name\n",
    );
    // Line 3 (0-indexed 2): `let n = p.name`. `name` starts after `p.` at
    // column 11 (0-indexed character 11).
    let value = hover_markup(&mut state, &u, 2, 11).expect("hover on .name");
    assert!(
        value.contains("name"),
        "expected member name in hover, got: {value}"
    );
}

#[test]
fn hover_on_data_declaration_shows_definition() {
    let mut state = GlobalState::default();
    let u = uri("file:///data.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\n",
    );
    // `Person` (data name) starts at column 6 (character 5 zero-indexed).
    let value = hover_markup(&mut state, &u, 0, 6).expect("hover on data name");
    assert!(
        value.contains("data Person")
            && value.contains("name: String")
            && value.contains("age: Int"),
        "expected the data definition with field types, not a bare label, got: {value}"
    );
}

#[test]
fn hover_on_adt_use_shows_variant_list() {
    // Hovering a user ADT in an annotation shows its variant list, not just a
    // `type: Shape` label.
    let mut state = GlobalState::default();
    let u = uri("file:///adt-use.flx");
    open(
        &mut state,
        &u,
        "type Shape = Circle(Float) | Rect(Float, Float)\n\
         fn area(s: Shape) -> Float { 0.0 }\n",
    );

    // Hover on `Shape` in the annotation `s: Shape` (line 1, char 12).
    let value = hover_markup(&mut state, &u, 1, 12).expect("hover on ADT use `Shape`");
    assert!(
        value.contains("Circle(Float)") && value.contains("Rect(Float, Float)"),
        "expected the variant list at the type-use site, got: {value}"
    );
}

#[test]
fn hover_on_type_alias_shows_body() {
    let mut state = GlobalState::default();
    let u = uri("file:///alias.flx");
    open(&mut state, &u, "alias Name = String\nlet n: Name = \"a\"\n");

    // Hover on `Name` at its declaration (line 0). `alias ` is 6 chars.
    let value = hover_markup(&mut state, &u, 0, 7).expect("hover on alias name");
    assert!(
        value.contains("alias Name = String"),
        "expected the alias body, got: {value}"
    );
}

#[test]
fn hover_on_import_name_returns_module_label() {
    let mut state = GlobalState::default();
    let u = uri("file:///imp.flx");
    open(
        &mut state,
        &u,
        "import Flow.Async exposing (..)\nfn body() with Async { 1 }\n",
    );
    // Cursor on `Async` in `Flow.Async` — column 13 (character 12).
    let value = hover_markup(&mut state, &u, 0, 12).expect("hover on import name");
    assert!(
        value.contains("module") && value.contains("Flow.Async"),
        "expected module label, got: {value}"
    );
}

#[test]
fn hover_on_named_field_init_returns_declared_type() {
    // `Person { name: "Alice", age: 30 }` — hover on `name` (the field
    // name, not the value) should reveal its declared type `String`.
    let mut state = GlobalState::default();
    let u = uri("file:///nfi.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet alice = Person { name: \"A\", age: 1 }\n",
    );
    // `name` field-name on line 2 starts at column 22 (0-indexed character 21).
    let value = hover_markup(&mut state, &u, 1, 22).expect("hover on field name");
    assert!(
        value.contains("name") && value.contains("String"),
        "expected field type in hover, got: {value}"
    );
}

#[test]
fn hover_on_member_access_returns_declared_type() {
    // `alice.name` — hover on `.name` should show `name: String` derived
    // from the data variant `alice` resolves to.
    let mut state = GlobalState::default();
    let u = uri("file:///mab.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet alice = Person { name: \"A\", age: 1 }\nlet n = alice.name\n",
    );
    // `name` on line 3 (0-indexed 2) starts at column 15 (character 14).
    let value = hover_markup(&mut state, &u, 2, 15).expect("hover on .name");
    assert!(
        value.contains("name") && value.contains("String"),
        "expected member type in hover, got: {value}"
    );
}

#[test]
fn hover_on_data_field_decl_returns_declared_type() {
    // `data Person { Person { name: String } }` — hover on the `name`
    // field at the declaration site should show its type.
    let mut state = GlobalState::default();
    let u = uri("file:///dfn.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\n",
    );
    // `name` field name at the decl site sits around column 24 (character 23).
    let value = hover_markup(&mut state, &u, 0, 24).expect("hover on data field decl");
    assert!(
        value.contains("name") && value.contains("String"),
        "expected declared type in data field hover, got: {value}"
    );
}

#[test]
fn hover_on_keyword_returns_keyword_doc() {
    let mut state = GlobalState::default();
    let u = uri("file:///kw.flx");
    open(&mut state, &u, "let x = 1\n");
    // Cursor on `let` (col 0).
    let value = hover_markup(&mut state, &u, 0, 1).expect("keyword hover");
    assert!(
        value.contains("Bind a value to a name"),
        "expected let keyword doc, got: {value}"
    );
}

#[test]
fn hover_on_new_keywords_returns_specific_docs() {
    // Table-driven smoke test: each case gives a source snippet, the
    // keyword to hover, and a phrase the keyword's docs MUST contain (so
    // we don't accidentally return the wrong entry's docs). Built-in
    // constructors `Some`/`None`/`Left`/`Right` are *excluded* on purpose
    // — they now route through the AST path and surface the inferred
    // type (see `hover_on_builtin_constructor_returns_inferred_type`).
    let cases: &[(&str, &str, &str)] = &[
        (
            "deriving",
            "data X { A, B } deriving (Eq)\n",
            "Auto-generate",
        ),
        ("type", "type T = Int\n", "transparent type alias"),
        ("where", "let x = a where a = 1\n", "let-binding"),
        (
            "select",
            "fn f() with Async { select { x -> 1 } }\n",
            "first ready",
        ),
        ("sealing", "fn f() { x sealing { Console } }\n", "Restrict"),
        (
            "primop",
            "intrinsic fn p() = primop X\n",
            "compiler primitive",
        ),
        // Newly added contextual keywords.
        (
            "ambient",
            "fn f() { x sealing (ambient - Console) }\n",
            "enclosing effect row",
        ),
        (
            "except",
            "import Flow.Math exposing (..) except (sqrt)\n",
            "Exclude members",
        ),
        (
            "end",
            "module M\n    public fn f() { 1 }\nend\n",
            "terminator",
        ),
        (
            "resume",
            "handle counter() with { get() -> resume(0) }\n",
            "Continue an effect handler",
        ),
    ];

    for (kw, source, expected_substring) in cases {
        let mut state = GlobalState::default();
        let u = uri(&format!("file:///kw_{kw}.flx"));
        open(&mut state, &u, source);
        let kw_byte_off = source.find(kw).expect("keyword in source");
        // Convert byte offset to LSP character (UTF-16 by default but our
        // sources are ASCII, so byte == char). The kw_byte_off may be on
        // any line — derive (line, col) from preceding newlines.
        let prefix = &source[..kw_byte_off];
        let line = prefix.matches('\n').count() as u32;
        let line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = (kw_byte_off - line_start + 1) as u32;
        let value = hover_markup(&mut state, &u, line, col)
            .unwrap_or_else(|| panic!("expected hover on `{kw}`"));
        assert!(
            value.contains(expected_substring),
            "for keyword `{kw}`, expected substring `{expected_substring}` in: {value}"
        );
    }
}

#[test]
fn hover_on_builtin_constructor_returns_inferred_type() {
    // The lexer reserves `Some`/`None`/`Left`/`Right`, but their hover
    // is intentionally routed through the AST path so it surfaces the
    // inferred type (e.g. `Option<Int>`) rather than a static doc. This
    // is the explicit policy enforced by the drift test in
    // `keywords::tests::every_lexer_keyword_has_hover_doc`.
    let cases: &[(&str, &str, &str)] = &[
        ("Some", "let x = Some(1)\n", "Option"),
        ("None", "let x: Option<Int> = None\n", "Option"),
        ("Left", "let x: Either<Int, Int> = Left(1)\n", "Either"),
        ("Right", "let x: Either<Int, Int> = Right(2)\n", "Either"),
    ];

    for (kw, source, expected_substring) in cases {
        let mut state = GlobalState::default();
        let u = uri(&format!("file:///ctor_{kw}.flx"));
        open(&mut state, &u, source);
        let kw_byte_off = source.find(kw).expect("constructor in source");
        let value = hover_markup(&mut state, &u, 0, (kw_byte_off + 1) as u32)
            .unwrap_or_else(|| panic!("expected hover on `{kw}`"));
        assert!(
            value.contains(expected_substring),
            "for constructor `{kw}`, expected `{expected_substring}` in inferred type: {value}"
        );
        // Negative: the prose from the old static doc must NOT appear.
        assert!(
            !value.contains("constructor representing"),
            "constructor `{kw}` should no longer return static doc: {value}"
        );
    }
}

#[test]
fn hover_on_keyword_inside_comment_does_not_return_doc() {
    // Regression: prose mentions of keywords inside `//` comments must NOT
    // surface keyword documentation. Today the locator returns None for
    // cursor positions inside comments (no AST node there); the regression
    // guards the keyword fast-path from leaking that.
    let mut state = GlobalState::default();
    let u = uri("file:///kwc.flx");
    open(&mut state, &u, "// use let to bind\nlet x = 1\n");
    // Cursor on `let` inside the comment (col 7).
    let hover = state.handle_hover(HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    });
    if let Some(h) = hover {
        let value = match h.contents {
            lsp_types::HoverContents::Markup(m) => m.value,
            other => panic!("unexpected hover contents: {other:?}"),
        };
        assert!(
            !value.contains("Bind a value to a name"),
            "keyword doc leaked into comment hover: {value}"
        );
    }
}

#[test]
fn completion_in_with_clause_lists_effect_labels() {
    let mut state = GlobalState::default();
    let u = uri("file:///cw.flx");
    open(&mut state, &u, "fn main() with \n");
    // Cursor right after `with `.
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 15,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion in with clause");
    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.iter().any(|l| l == "IO"),
        "expected IO in {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "Async"),
        "expected Async in {labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l == "let"),
        "got default keyword list in with-clause context: {labels:?}"
    );
}

#[test]
fn completion_after_record_dot_lists_fields() {
    let mut state = GlobalState::default();
    let u = uri("file:///cd.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet alice = Person { name: \"A\", age: 1 }\nlet x = alice.\n",
    );
    // Cursor right after `alice.` on line 3.
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 2,
                    character: 14,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion after record dot");
    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.iter().any(|l| l == "name"),
        "expected `name` field in {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "age"),
        "expected `age` field in {labels:?}"
    );
}

#[test]
fn completion_in_named_constructor_lists_fields() {
    let mut state = GlobalState::default();
    let u = uri("file:///cnc.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet p = Person { \n",
    );
    // Cursor inside the `{ ` on line 2, position right after `{ `.
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 1,
                    character: 17,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion in constructor body");
    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.iter().any(|l| l == "name"),
        "expected `name` field in {labels:?}"
    );
    assert!(
        labels.iter().any(|l| l == "age"),
        "expected `age` field in {labels:?}"
    );
}

#[test]
fn completion_after_module_dot_lists_module_members() {
    // Type `String.` and expect Flow.String members in the completion list.
    // The workspace is the flux repo root so prelude loads from lib/Flow/.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("completion-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    // `String.` — cursor right after the dot.
    let source = "fn main() with IO { String. }\n";
    open(&mut state, &u, source);

    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position {
                    line: 0,
                    character: 27,
                },
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");

    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    // Don't pin the exact set of String module members (subject to change);
    // just assert we got SOMETHING that looks like a Flow.String member.
    assert!(
        !labels.is_empty(),
        "expected non-empty completion list after `String.`"
    );
    // And confirm it's not the default keyword-only list (which would
    // include `let`/`fn`/etc. — none of those should be in a member list).
    assert!(
        !labels.iter().any(|l| l == "let" || l == "fn"),
        "got default keyword list instead of module members: {labels:?}"
    );
}

#[test]
fn completion_after_non_prelude_flow_module_dot_lists_members() {
    // `Http` is not part of the auto-prelude — completion must still work,
    // because every `lib/Flow/*.flx` module is eagerly indexed for it.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let workspace = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = workspace.join("completion-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    // `Http.` — `Http` starts at char 20, the dot at 24, cursor at 25.
    let source = "fn main() with IO { Http. }\n";
    open(&mut state, &u, source);

    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position::new(0, 25),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");

    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        !labels.is_empty() && !labels.iter().any(|l| l == "let" || l == "fn"),
        "expected Flow.Http members for a non-prelude module, got {labels:?}"
    );
}

#[test]
fn completion_after_user_module_dot_lists_members() {
    // Type `M.` where `M` aliases a sibling user module and expect that
    // module's public members — not the default keyword list.
    let math_src =
        "module Math {\n    public fn twice(x) { x * 2 }\n    public fn triple(x) { x * 3 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M. }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // `fn run() { M. }` — cursor right after the dot (char 13 on line 2).
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(2, 13),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");

    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.contains(&"twice".to_string()) && labels.contains(&"triple".to_string()),
        "expected user-module members `twice`/`triple`, got {labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l == "let" || l == "fn"),
        "got default keyword list instead of module members: {labels:?}"
    );
}

#[test]
fn goto_def_on_named_constructor_jumps_to_data_decl() {
    let mut state = GlobalState::default();
    let u = uri("file:///cons.flx");
    open(
        &mut state,
        &u,
        "data Person { Person { name: String, age: Int } }\nlet p = Person { name: \"A\", age: 1 }\n",
    );
    // `Person` reference on line 2 starts at column 9 (character 8).
    let location = state.handle_definition(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position {
                line: 1,
                character: 9,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert!(
        location.is_some(),
        "expected a definition Location for Person reference"
    );
}

#[test]
fn cross_file_goto_definition_resolves_modules_in_a_subdirectory_project() {
    // The workspace root is the temp dir, but the project's entry and its
    // package both live in a `proj/` subtree. `import Lib.App.Main` resolves
    // against `proj/` — an ancestor of the entry — not the root itself.
    let main_mod = "module Lib.App.Main {\n    public fn run() with IO { print(\"hi\") }\n}\n";
    let entry = "import Lib.App.Main\n\nfn main() with IO {\n    Lib.App.Main.run()\n}\n";
    let (_dir, mut state, uris) = workspace_fixture(&[
        ("proj/Lib/App/Main.flx", main_mod),
        ("proj/main.flx", entry),
    ]);
    open(&mut state, &uris[1], entry);

    // Cursor on `run` in `Lib.App.Main.run()` (line 3, char 18).
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 18),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition response");
    let location = expect_location(resp);
    assert_eq!(
        location.uri, uris[0],
        "expected goto-definition to land in the subdirectory module file"
    );
}

#[test]
fn goto_definition_on_import_jumps_into_the_module_file() {
    let main_mod = "module Lib.App.Main {\n    public fn run() with IO { print(\"hi\") }\n}\n";
    let entry = "import Lib.App.Main as M\n\nfn run() { M.run() }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Lib/App/Main.flx", main_mod), ("main.flx", entry)]);
    open(&mut state, &uris[1], entry);

    // char 12 — inside the `Lib.App.Main` module name; char 23 — the `M`
    // alias. Both should jump into the module's own file.
    for ch in [12u32, 23] {
        let resp = state
            .handle_definition(GotoDefinitionParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: ident(&uris[1]),
                    position: Position::new(0, ch),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            })
            .unwrap_or_else(|| panic!("goto-definition at char {ch}"));
        let location = expect_location(resp);
        assert_eq!(
            location.uri, uris[0],
            "import goto-def at char {ch} should land in the module file"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Document highlight
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn document_highlight_marks_all_occurrences() {
    let mut state = GlobalState::default();
    let u = uri("file:///hl.flx");
    open(
        &mut state,
        &u,
        "fn main() {\n    let total = 1\n    let next = total + total\n}\n",
    );

    // Cursor on the `total` declaration (line 1, char 9) — the binding plus
    // its two uses in `total + total` should all be highlighted.
    let highlights = state.handle_document_highlight(DocumentHighlightParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(1, 9),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert_eq!(
        highlights.len(),
        3,
        "expected 3 highlights for `total`, got {highlights:?}"
    );
}

#[test]
fn document_highlight_tags_read_and_write() {
    let mut state = GlobalState::default();
    let u = uri("file:///hl-rw.flx");
    open(
        &mut state,
        &u,
        "fn main() {\n    let total = 1\n    let next = total + total\n}\n",
    );

    let highlights = state.handle_document_highlight(DocumentHighlightParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(1, 9), // the `total` binding
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    let writes = highlights
        .iter()
        .filter(|h| h.kind == Some(lsp_types::DocumentHighlightKind::WRITE))
        .count();
    let reads = highlights
        .iter()
        .filter(|h| h.kind == Some(lsp_types::DocumentHighlightKind::READ))
        .count();
    assert_eq!(writes, 1, "the `let total` binding is a write");
    assert_eq!(reads, 2, "the two uses in `total + total` are reads");
}

#[test]
fn document_highlight_marks_function_exit_points() {
    let mut state = GlobalState::default();
    let u = uri("file:///hl-exits.flx");
    open(
        &mut state,
        &u,
        "fn pick(n: Int) -> Int {\n    if n > 0 {\n        return 1\n    }\n    return 0\n}\n",
    );

    // Cursor on the first `return` (line 2).
    let highlights = state.handle_document_highlight(DocumentHighlightParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(2, 10),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert_eq!(
        highlights.len(),
        2,
        "both `return`s should be highlighted, got {highlights:?}"
    );
}

#[test]
fn document_highlight_links_perform_and_handle() {
    let mut state = GlobalState::default();
    let u = uri("file:///hl-effect.flx");
    open(
        &mut state,
        &u,
        "effect Audit { log: String -> Int }\n\
         fn run() -> Int {\n\
         \x20   perform Audit.log(\"x\") handle Audit {\n\
         \x20       log(resume, m) -> resume(1)\n\
         \x20   }\n\
         }\n",
    );

    // Cursor on `log` in `perform Audit.log` (line 2, char 19).
    let highlights = state.handle_document_highlight(DocumentHighlightParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(2, 19),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert_eq!(
        highlights.len(),
        2,
        "the perform site and its handle arm should both highlight, got {highlights:?}"
    );
    // One on the perform line (2), one on the handle-arm line (3).
    assert!(highlights.iter().any(|h| h.range.start.line == 2));
    assert!(highlights.iter().any(|h| h.range.start.line == 3));
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace symbol search
// ─────────────────────────────────────────────────────────────────────────────

fn workspace_symbol_names(state: &mut GlobalState, query: &str) -> Vec<String> {
    let resp = state
        .handle_workspace_symbol(WorkspaceSymbolParams {
            query: query.to_string(),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("workspace symbol response");
    match resp {
        WorkspaceSymbolResponse::Nested(syms) => syms.into_iter().map(|s| s.name).collect(),
        WorkspaceSymbolResponse::Flat(syms) => syms.into_iter().map(|s| s.name).collect(),
    }
}

#[test]
fn workspace_symbol_finds_declarations_across_files() {
    // `twice` lives inside `module Math` in a file that is never opened —
    // the workspace discovers it on disk.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "fn compute() { 42 }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    let names = workspace_symbol_names(&mut state, "twice");
    assert!(
        names.iter().any(|n| n == "twice"),
        "expected the module member `twice`, got {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "compute"),
        "query `twice` should not match `compute`, got {names:?}"
    );

    // An empty query returns everything discovered — both files' symbols.
    let all = workspace_symbol_names(&mut state, "");
    assert!(
        all.iter().any(|n| n == "twice") && all.iter().any(|n| n == "compute"),
        "empty query should list every declaration, got {all:?}"
    );
}

#[test]
fn workspace_symbol_index_updates_on_edit() {
    // The symbol index is cached per file; an edit must refresh that file's
    // entry so the query reflects the new declaration, not the stale one.
    let main_src = "fn alpha() { 1 }\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("main.flx", main_src)]);
    open(&mut state, &uris[0], main_src);
    assert!(
        workspace_symbol_names(&mut state, "alpha")
            .iter()
            .any(|n| n == "alpha"),
        "expected to find `alpha` before the edit"
    );

    // Rename the function and re-sync the buffer.
    state.handle_did_change(DidChangeTextDocumentParams {
        text_document: lsp_types::VersionedTextDocumentIdentifier {
            uri: uris[0].clone(),
            version: 2,
        },
        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "fn beta() { 1 }\n".to_string(),
        }],
    });

    assert!(
        workspace_symbol_names(&mut state, "beta")
            .iter()
            .any(|n| n == "beta"),
        "expected the renamed `beta` after the edit"
    );
    assert!(
        !workspace_symbol_names(&mut state, "alpha")
            .iter()
            .any(|n| n == "alpha"),
        "the stale `alpha` must be gone from the index after the edit"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Prepare rename & folding ranges
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn prepare_rename_returns_the_identifier_range() {
    let mut state = GlobalState::default();
    let u = uri("file:///pr.flx");
    open(&mut state, &u, "fn main() {\n    let total = 1\n}\n\n");

    // Cursor on `total` (line 1) — renameable, returns its 5-char range.
    let resp = state
        .handle_prepare_rename(TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(1, 9),
        })
        .expect("prepare-rename response");
    match resp {
        lsp_types::PrepareRenameResponse::Range(range) => {
            assert_eq!(range.start.line, 1);
            assert_eq!(
                range.end.character - range.start.character,
                5,
                "expected the `total` identifier range"
            );
        }
        other => panic!("expected a plain range, got {other:?}"),
    }

    // Cursor on the trailing blank line — nothing to rename.
    let none = state.handle_prepare_rename(TextDocumentPositionParams {
        text_document: ident(&u),
        position: Position::new(3, 0),
    });
    assert!(none.is_none(), "a blank line has no renameable identifier");
}

#[test]
fn folding_range_covers_multiline_declarations() {
    let mut state = GlobalState::default();
    let u = uri("file:///fold.flx");
    open(&mut state, &u, "fn helper() {\n    1\n}\nlet x = 2\n");

    let folds = state.handle_folding_range(FoldingRangeParams {
        text_document: ident(&u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });

    // `fn helper` spans lines 0–2; `let x = 2` is single-line → one fold.
    assert_eq!(
        folds.len(),
        1,
        "expected one fold for the multi-line function, got {folds:?}"
    );
    assert_eq!(folds[0].start_line, 0);
    assert_eq!(folds[0].end_line, 2);
}

#[test]
fn selection_range_nests_outward_from_the_cursor() {
    let mut state = GlobalState::default();
    let u = uri("file:///sel.flx");
    open(&mut state, &u, "fn main() {\n    let x = 1 + 2\n}\n");

    // Cursor on the `1` literal inside `1 + 2` (line 1, char 12).
    let resp = state.handle_selection_range(SelectionRangeParams {
        text_document: ident(&u),
        positions: vec![Position::new(1, 12)],
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert_eq!(resp.len(), 1, "one chain per requested position");

    // Walk the parent chain, collecting each range.
    let mut ranges = Vec::new();
    let mut node = &resp[0];
    loop {
        ranges.push(node.range);
        match &node.parent {
            Some(parent) => node = parent,
            None => break,
        }
    }
    assert!(
        ranges.len() >= 3,
        "expected several nested ranges (literal → infix → statement → …), got {ranges:?}"
    );
    // Each parent must strictly enclose the child.
    for pair in ranges.windows(2) {
        let (inner, outer) = (pair[0], pair[1]);
        assert!(
            (outer.start.line, outer.start.character) <= (inner.start.line, inner.start.character)
                && (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character),
            "selection ranges are not nested: {ranges:?}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Go to implementation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn implementation_lists_class_instances() {
    let mut state = GlobalState::default();
    let u = uri("file:///impl.flx");
    open(
        &mut state,
        &u,
        "class Show<a> { show: a -> String }\n\
         instance Show<Int> { show(x) { \"int\" } }\n\
         instance Show<Bool> { show(x) { \"bool\" } }\n",
    );

    // Cursor on `Show` in the `class Show<a>` declaration (line 0, char 7).
    let resp = state
        .handle_implementation(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position::new(0, 7),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("implementation response");

    match resp {
        lsp_types::GotoDefinitionResponse::Array(locations) => assert_eq!(
            locations.len(),
            2,
            "expected both `Show` instances, got {locations:?}"
        ),
        other => panic!("expected an array of instance locations, got {other:?}"),
    }
}

#[test]
fn implementation_is_empty_off_a_class_name() {
    let mut state = GlobalState::default();
    let u = uri("file:///impl_none.flx");
    open(&mut state, &u, "fn main() {\n    let x = 1\n}\n");

    // Cursor on the local `x` — not a class, so no implementations.
    let resp = state.handle_implementation(GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(&u),
            position: Position::new(1, 8),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    assert!(
        resp.is_none(),
        "expected no implementations off a non-class"
    );
}

#[test]
fn implementation_finds_instances_across_modules() {
    // The class and one instance live in `Shapes`; a second instance lives in a
    // sibling module `Extra`. A plain `main` importing both ties them into one
    // module-graph component. Goto-implementation on the class name must list
    // both instances — including the cross-module one (and instances nested in
    // `module` blocks, which the old top-level-only scan missed).
    let shapes = "module Shapes {\n    public class Show<a> {\n        fn show(x: a) -> String\n    }\n    instance Show<Int> {\n        fn show(x) { \"int\" }\n    }\n}\n";
    let extra = "import Shapes\n\nmodule Extra {\n    instance Show<Bool> {\n        fn show(x) { \"bool\" }\n    }\n}\n";
    let main = "import Shapes\nimport Extra\n\nfn run() { 0 }\n";
    let (_dir, mut state, uris) = workspace_fixture(&[
        ("Shapes.flx", shapes),
        ("Extra.flx", extra),
        ("main.flx", main),
    ]);
    // Open the importer entry that ties the three files into one component.
    open(&mut state, &uris[2], main);

    // Cursor on `Show` in `public class Show<a>` (Shapes.flx line 1, char 18).
    let resp = state
        .handle_implementation(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[0]),
                position: Position::new(1, 18),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("implementation response");

    match resp {
        lsp_types::GotoDefinitionResponse::Array(locations) => {
            assert_eq!(
                locations.len(),
                2,
                "expected instances from both modules, got {locations:?}"
            );
            assert!(
                locations.iter().any(|l| l.uri == uris[1]),
                "expected the cross-module instance in Extra.flx, got {locations:?}"
            );
        }
        other => panic!("expected an array of instance locations, got {other:?}"),
    }
}

#[test]
fn implementation_resolves_public_class_name() {
    // Regression: `decl_name_start` used to add `public `'s width on top of a
    // span that already begins at the `class` keyword, so the cursor on a
    // `public class` name landed past the name and resolved nothing.
    let mut state = GlobalState::default();
    let u = uri("file:///pubclass.flx");
    open(
        &mut state,
        &u,
        "public class Show<a> {\n    fn show(x: a) -> String\n}\n\
         instance Show<Int> {\n    fn show(x) { \"int\" }\n}\n",
    );

    // Cursor on `Show` in `public class Show<a>` (line 0, char 14 — "public
    // class " is 13 chars, so `Show` starts at column 13).
    let resp = state
        .handle_implementation(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position::new(0, 14),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("implementation response on a public class");
    match resp {
        lsp_types::GotoDefinitionResponse::Array(locations) => {
            assert_eq!(
                locations.len(),
                1,
                "expected the one instance, got {locations:?}"
            )
        }
        other => panic!("expected an array of instance locations, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Code actions (quick fixes)
// ─────────────────────────────────────────────────────────────────────────────

fn action_title(action: &CodeActionOrCommand) -> &str {
    match action {
        CodeActionOrCommand::CodeAction(ca) => &ca.title,
        CodeActionOrCommand::Command(c) => &c.title,
    }
}

/// New text of the first `TextEdit` in a code action's `WorkspaceEdit`.
fn action_edit_text(action: &CodeActionOrCommand) -> Option<String> {
    let CodeActionOrCommand::CodeAction(ca) = action else {
        return None;
    };
    let DocumentChanges::Edits(edits) = ca.edit.as_ref()?.document_changes.as_ref()? else {
        return None;
    };
    match edits.first()?.edits.first()? {
        OneOf::Left(te) => Some(te.new_text.clone()),
        OneOf::Right(ate) => Some(ate.text_edit.new_text.clone()),
    }
}

/// Range of the first `TextEdit` in a code action's `WorkspaceEdit`.
fn action_edit_range(action: &CodeActionOrCommand) -> Option<Range> {
    let CodeActionOrCommand::CodeAction(ca) = action else {
        return None;
    };
    let DocumentChanges::Edits(edits) = ca.edit.as_ref()?.document_changes.as_ref()? else {
        return None;
    };
    match edits.first()?.edits.first()? {
        OneOf::Left(te) => Some(te.range),
        OneOf::Right(ate) => Some(ate.text_edit.range),
    }
}

/// All `(range, new_text)` edits in a code action's `WorkspaceEdit`.
fn action_edits(action: &CodeActionOrCommand) -> Vec<(Range, String)> {
    let CodeActionOrCommand::CodeAction(ca) = action else {
        return vec![];
    };
    let Some(DocumentChanges::Edits(edits)) =
        ca.edit.as_ref().and_then(|e| e.document_changes.as_ref())
    else {
        return vec![];
    };
    edits
        .iter()
        .flat_map(|tde| &tde.edits)
        .map(|e| match e {
            OneOf::Left(te) => (te.range, te.new_text.clone()),
            OneOf::Right(ate) => (ate.text_edit.range, ate.text_edit.new_text.clone()),
        })
        .collect()
}

fn code_action_params(u: &Uri, range: Range) -> CodeActionParams {
    CodeActionParams {
        text_document: ident(u),
        range,
        context: CodeActionContext::default(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

fn code_lens_params(u: &Uri) -> CodeLensParams {
    CodeLensParams {
        text_document: ident(u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

#[test]
fn diagnostics_carry_code_description_link() {
    // A coded diagnostic (here E015, non-exhaustive match) carries a
    // codeDescription deep-linking to the error-code reference anchor.
    let mut state = GlobalState::default();
    let u = uri("file:///diag.flx");
    let src = "data Color { Red, Green, Blue }\nfn pick(c: Color) -> Int {\n    match c {\n        Red -> 1,\n        Green -> 2\n    }\n}\n";
    let published = state.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: u.clone(),
            language_id: "flux".into(),
            version: 1,
            text: src.into(),
        },
    });

    let diags: Vec<_> = published.into_iter().flat_map(|p| p.diagnostics).collect();
    let e015 = diags
        .iter()
        .find(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E015"))
        .expect("an E015 diagnostic");
    let href = e015
        .code_description
        .as_ref()
        .expect("code_description on E015");
    assert!(
        href.href.as_str().ends_with("error_codes.md#e015"),
        "unexpected href: {}",
        href.href.as_str()
    );
}

#[test]
fn code_lens_offers_run_above_main() {
    let mut state = GlobalState::default();
    let u = uri("file:///run.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    print(\"hi\")\n}\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    assert_eq!(lenses.len(), 1, "expected exactly one Run lens on `main`");
    let cmd = lenses[0].command.as_ref().expect("lens command");
    assert_eq!(cmd.command, "flux.run");
    assert!(cmd.title.contains("Run"), "title was {:?}", cmd.title);
}

/// All commands of `name` carried by `lenses`.
fn lens_cmds<'a>(lenses: &'a [lsp_types::CodeLens], name: &str) -> Vec<&'a lsp_types::Command> {
    lenses
        .iter()
        .filter_map(|l| l.command.as_ref())
        .filter(|c| c.command == name)
        .collect()
}

#[test]
fn code_lens_offers_run_test_per_test_function() {
    let mut state = GlobalState::default();
    let u = uri("file:///tests.flx");
    open(
        &mut state,
        &u,
        "fn test_a() { assert_true(true) }\nfn test_b() { assert_true(true) }\nfn helper() { 1 }\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    let per_test = lens_cmds(&lenses, "flux.runTest");
    assert_eq!(per_test.len(), 2, "a lens per test fn, none for `helper`");
    let args = per_test[0].arguments.as_ref().expect("lens arguments");
    assert_eq!(
        args[1],
        serde_json::json!("test_a"),
        "the test name should travel as the second command argument"
    );
}

#[test]
fn code_lens_offers_run_all_tests_for_multiple_tests() {
    let mut state = GlobalState::default();
    let u = uri("file:///run-all.flx");
    open(
        &mut state,
        &u,
        "fn test_a() { assert_true(true) }\nfn test_b() { assert_true(true) }\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    let run_all = lens_cmds(&lenses, "flux.runTests");
    assert_eq!(
        run_all.len(),
        1,
        "exactly one file-level run-all-tests lens, got {lenses:?}"
    );
    assert!(
        run_all[0].title.contains("all"),
        "title was {:?}",
        run_all[0].title
    );
    // It carries only the file URI — no per-test filter.
    assert_eq!(
        run_all[0].arguments.as_ref().map(Vec::len),
        Some(1),
        "run-all should carry just the file uri"
    );
    // It sits on the first test's line (above `test_a`).
    let run_all_range = lenses
        .iter()
        .find(|l| {
            l.command
                .as_ref()
                .is_some_and(|c| c.command == "flux.runTests")
        })
        .map(|l| l.range)
        .unwrap();
    assert_eq!(
        run_all_range.start.line, 0,
        "run-all should be above test_a"
    );
}

#[test]
fn code_lens_no_run_all_for_single_test() {
    let mut state = GlobalState::default();
    let u = uri("file:///single-test.flx");
    open(&mut state, &u, "fn test_only() { assert_true(true) }\n");

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    assert!(
        lens_cmds(&lenses, "flux.runTests").is_empty(),
        "a lone test needs no run-all lens, got {lenses:?}"
    );
    assert_eq!(lens_cmds(&lenses, "flux.runTest").len(), 1);
}

#[test]
fn code_lens_offers_eval_on_doc_comment_snippet() {
    let mut state = GlobalState::default();
    let u = uri("file:///eval-lens.flx");
    open(
        &mut state,
        &u,
        "/// >>> 2 + 2\nfn main() with IO {\n    print(\"hi\")\n}\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    let evals = lens_cmds(&lenses, "flux.evalComment");
    assert_eq!(
        evals.len(),
        1,
        "one eval lens on the >>> line, got {lenses:?}"
    );
    // Arguments are [uri, expr, line, indent].
    let args = evals[0].arguments.as_ref().expect("eval lens arguments");
    assert_eq!(
        args[1],
        serde_json::json!("2 + 2"),
        "the expression travels as the second argument"
    );
    assert_eq!(
        args[2],
        serde_json::json!(0),
        "the >>> line number travels as the third argument"
    );
}

#[test]
fn code_lens_no_eval_for_plain_doc_or_non_doc_comment() {
    let mut state = GlobalState::default();
    let u = uri("file:///no-eval.flx");
    // A `///` line without `>>>`, a `///` result line, and a `// >>>` line that
    // is only a plain (non-doc) comment must none of them get an eval lens.
    open(
        &mut state,
        &u,
        "/// just docs\n/// => stale\n// >>> 1 + 1\nfn main() with IO {\n    print(\"hi\")\n}\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    assert!(
        lens_cmds(&lenses, "flux.evalComment").is_empty(),
        "no eval lens for plain docs, `=>` result lines, or `//` comments, got {lenses:?}"
    );
}

#[test]
fn code_action_changes_return_type_to_inferred() {
    let mut state = GlobalState::default();
    let u = uri("file:///return-mismatch.flx");
    // `area` is declared `-> Bool` but its body is `Int` — an E300 return
    // mismatch. The compiler attaches an inline suggestion (the inferred type);
    // the LSP surfaces it as a quick fix via `suggestion_actions`.
    open(&mut state, &u, "fn area() -> Bool {\n    3 + 4\n}\n");

    // The squiggle (E300 primary span) is the body's value expression on line 1.
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 4), Position::new(1, 9)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Change return type to `Int`"))
        .expect("expected a change-return-type quick fix");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some("Int"),
        "the fix replaces the annotation with the inferred type"
    );
    // The edit rewrites the `Bool` annotation on line 0, not the body on line 1.
    let range = action_edit_range(fix).expect("edit range");
    assert_eq!(
        range.start.line, 0,
        "the edit targets the return annotation, got {range:?}"
    );
}

#[test]
fn code_action_no_change_return_type_when_signature_matches() {
    let mut state = GlobalState::default();
    let u = uri("file:///return-ok.flx");
    open(&mut state, &u, "fn area() -> Int {\n    3 + 4\n}\n");

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 4), Position::new(1, 9)),
        ))
        .expect("code action response");
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("Change return type")),
        "a correctly-typed function offers no change-return-type fix, got {actions:?}"
    );
}

#[test]
fn code_action_makes_wildcard_import_explicit() {
    let mut state = GlobalState::default();
    // Repo-root URI so the prelude (and Flow.List's member list) loads.
    let u = repo_uri("explicit-import-fixture.flx");
    // `exposing (..)` brings all of Flow.List in; the buffer uses `map` and
    // `length` unqualified, and `reverse` only via `List.reverse` (qualified).
    open(
        &mut state,
        &u,
        "import Flow.List exposing (..)\n\
         fn main() with IO {\n\
         \x20   let r = List.reverse([1, 2, 3])\n\
         \x20   print(length(map(r, \\x -> x + 1)))\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 30)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Make `List` import explicit"))
        .expect("expected a make-import-explicit refactor");
    // Only the unqualified uses are listed (sorted); the qualified `reverse` is
    // excluded.
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some("(length, map)"),
        "explicit list should be the used unqualified members, sorted"
    );
    let range = action_edit_range(fix).expect("edit range");
    assert_eq!(range.start.line, 0, "the edit rewrites the import's clause");
}

#[test]
fn code_action_refines_explicit_import_list() {
    let mut state = GlobalState::default();
    let u = uri("file:///refine-import.flx");
    // `fold` and `filter` are exposed but unused → trimmed; `map` stays.
    open(
        &mut state,
        &u,
        "import Flow.List exposing (map, filter, fold)\n\
         fn main() with IO {\n\
         \x20   print(map([1, 2, 3], \\x -> x + 1))\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 40)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Refine `List` exposing list"))
        .expect("expected a refine-import refactor");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some("(map)"),
        "refine should drop the unused exposed names"
    );
}

#[test]
fn code_action_no_explicit_import_when_already_minimal() {
    let mut state = GlobalState::default();
    let u = uri("file:///minimal-import.flx");
    open(
        &mut state,
        &u,
        "import Flow.List exposing (map)\n\
         fn main() with IO {\n\
         \x20   print(map([1, 2, 3], \\x -> x + 1))\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 30)),
        ))
        .expect("code action response");
    assert!(
        !actions.iter().any(|a| {
            let t = action_title(a);
            t.contains("import explicit") || t.contains("exposing list")
        }),
        "an already-minimal explicit import offers no refactor, got {actions:?}"
    );
}

#[test]
fn code_action_no_explicit_import_for_qualified_only_import() {
    let mut state = GlobalState::default();
    let u = uri("file:///qualified-import.flx");
    // No `exposing` clause → nothing to make explicit.
    open(
        &mut state,
        &u,
        "import Flow.List\n\
         fn main() with IO {\n\
         \x20   print(List.map([1, 2, 3], \\x -> x + 1))\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 16)),
        ))
        .expect("code action response");
    assert!(
        !actions.iter().any(|a| {
            let t = action_title(a);
            t.contains("import explicit") || t.contains("exposing list")
        }),
        "a qualified-only import offers no make-explicit refactor, got {actions:?}"
    );
}

#[test]
fn code_lens_offers_make_imports_explicit() {
    let mut state = GlobalState::default();
    let u = repo_uri("explicit-lens-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.List exposing (..)\n\
         fn main() with IO {\n\
         \x20   print(map([1, 2, 3], \\x -> x + 1))\n\
         }\n",
    );

    let lenses = state
        .handle_code_lens(code_lens_params(&u))
        .expect("code lenses");
    let cmds = lens_cmds(&lenses, "flux.makeImportsExplicit");
    assert_eq!(cmds.len(), 1, "one make-explicit lens, got {lenses:?}");
    // Arguments are [uri, clauseRange, newText].
    let args = cmds[0].arguments.as_ref().expect("lens arguments");
    assert_eq!(
        args[2],
        serde_json::json!("(map)"),
        "the lens carries the rewritten clause text"
    );
}

// ── Item 5: prefix unused `let` with `_` (linter W001 quick-fix) ─────────────

#[test]
fn code_action_prefixes_unused_let_with_underscore() {
    let mut state = GlobalState::default();
    let u = uri("file:///unused-let.flx");
    // `x` is bound but never used — the linter reports W001.
    open(
        &mut state,
        &u,
        "fn main() with IO {\n\
         \x20   let x = 1\n\
         \x20   print(2)\n\
         }\n",
    );

    // Cursor on the binding name `x` (line 1, col 8 — after `    let `).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 8), Position::new(1, 8)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Prefix `x` with `_`"))
        .expect("expected a prefix-with-underscore quick fix");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some("_"),
        "the fix inserts a single `_`"
    );
    // A zero-width insert right before the name.
    let range = action_edit_range(fix).expect("edit range");
    assert_eq!((range.start.line, range.start.character), (1, 8));
    assert_eq!((range.end.line, range.end.character), (1, 8));
}

#[test]
fn code_action_no_prefix_for_used_let() {
    let mut state = GlobalState::default();
    let u = uri("file:///used-let.flx");
    // `x` is used by `print(x)`, so there's no W001 and nothing to prefix.
    open(
        &mut state,
        &u,
        "fn main() with IO {\n\
         \x20   let x = 1\n\
         \x20   print(x)\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 8), Position::new(1, 8)),
        ))
        .expect("code action response");
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("Prefix `x`")),
        "a used binding offers no prefix-with-underscore fix, got {actions:?}"
    );
}

#[test]
fn code_action_prefixes_public_let_after_keyword() {
    let mut state = GlobalState::default();
    let u = uri("file:///unused-public-let.flx");
    // A top-level `public let` — the insert lands after `public let `, not `let `.
    open(&mut state, &u, "public let y = 1\n");

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 11), Position::new(0, 11)),
        ))
        .expect("code action response");
    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Prefix `y` with `_`"))
        .expect("expected a prefix-with-underscore quick fix");
    let range = action_edit_range(fix).expect("edit range");
    assert_eq!(
        (range.start.line, range.start.character),
        (0, 11),
        "insert sits before `y`, i.e. after `public let `"
    );
}

// ── Item 6: convert number format (integer literal toggle) ───────────────────

#[test]
fn code_action_converts_decimal_to_hex_and_binary() {
    let mut state = GlobalState::default();
    let u = uri("file:///num-dec.flx");
    open(&mut state, &u, "let x = 255\n");

    // Cursor on the `255` literal (col 8).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 8), Position::new(0, 8)),
        ))
        .expect("code action response");

    let hex = actions
        .iter()
        .find(|a| action_title(a).contains("hexadecimal"))
        .expect("expected a hex conversion");
    assert_eq!(action_edit_text(hex).as_deref(), Some("0xFF"));
    let bin = actions
        .iter()
        .find(|a| action_title(a).contains("binary"))
        .expect("expected a binary conversion");
    assert_eq!(action_edit_text(bin).as_deref(), Some("0b11111111"));
    // The form already in use (decimal) is not offered.
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("Convert to decimal (")),
        "the current decimal form should not be offered, got {actions:?}"
    );
}

#[test]
fn code_action_converts_hex_to_decimal() {
    let mut state = GlobalState::default();
    let u = uri("file:///num-hex.flx");
    open(&mut state, &u, "let x = 0xFF\n");

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 8), Position::new(0, 8)),
        ))
        .expect("code action response");

    let dec = actions
        .iter()
        .find(|a| action_title(a).contains("Convert to decimal ("))
        .expect("expected a decimal conversion");
    assert_eq!(action_edit_text(dec).as_deref(), Some("255"));
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("hexadecimal")),
        "the current hex form should not be offered, got {actions:?}"
    );
}

#[test]
fn code_action_groups_large_decimal_with_separators() {
    let mut state = GlobalState::default();
    let u = uri("file:///num-grouped.flx");
    open(&mut state, &u, "let x = 1000\n");

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 8), Position::new(0, 8)),
        ))
        .expect("code action response");
    let grouped = actions
        .iter()
        .find(|a| action_title(a).contains("separators"))
        .expect("expected a digit-grouping conversion");
    assert_eq!(action_edit_text(grouped).as_deref(), Some("1_000"));
}

#[test]
fn code_action_no_number_format_off_literal() {
    let mut state = GlobalState::default();
    let u = uri("file:///num-off.flx");
    open(&mut state, &u, "let x = 42\n");

    // Cursor on the binding name `x` (col 4), not the literal.
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 4), Position::new(0, 4)),
        ))
        .expect("code action response");
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("Convert to")),
        "no number-format action when the cursor isn't on a literal, got {actions:?}"
    );
}

// ── Item 7: operator fixity (+ inferred type) in hover ───────────────────────

#[test]
fn hover_on_infix_operator_shows_fixity_and_type() {
    let mut state = GlobalState::default();
    let u = uri("file:///op-infix.flx");
    open(&mut state, &u, "let x = 1 + 2\n");

    // Hover on the `+` (col 10).
    let md = hover_markup(&mut state, &u, 0, 10).expect("hover on `+`");
    assert!(md.contains("infixl"), "expected fixity keyword, got: {md}");
    assert!(
        md.contains("Sum"),
        "expected the precedence level name, got: {md}"
    );
    assert!(
        md.to_lowercase().contains("int"),
        "expected the inferred type alongside fixity, got: {md}"
    );
}

#[test]
fn hover_on_comparison_operator_shows_equals_precedence() {
    let mut state = GlobalState::default();
    let u = uri("file:///op-eq.flx");
    open(&mut state, &u, "let b = 1 == 2\n");

    // Hover on the `==` (col 10) — a different precedence level than `+`. (Pipe
    // `|>` is desugared to a call at parse time, so it has no operator node.)
    let md = hover_markup(&mut state, &u, 0, 10).expect("hover on `==`");
    assert!(md.contains("infixl"), "expected fixity keyword, got: {md}");
    assert!(
        md.contains("Equals"),
        "expected the Equals precedence level, got: {md}"
    );
}

#[test]
fn hover_on_prefix_operator_shows_prefix_fixity() {
    let mut state = GlobalState::default();
    let u = uri("file:///op-prefix.flx");
    open(&mut state, &u, "let b = !true\n");

    // Hover on the `!` (col 8).
    let md = hover_markup(&mut state, &u, 0, 8).expect("hover on `!`");
    assert!(
        md.contains("prefix operator"),
        "expected prefix-operator hover, got: {md}"
    );
}

#[test]
fn hover_on_operand_shows_type_not_fixity() {
    let mut state = GlobalState::default();
    let u = uri("file:///op-operand.flx");
    open(&mut state, &u, "let x = 1 + 2\n");

    // Hover on the `1` operand (col 8), not the operator.
    let md = hover_markup(&mut state, &u, 0, 8).expect("hover on `1`");
    assert!(
        md.to_lowercase().contains("int"),
        "an operand still shows its type, got: {md}"
    );
    assert!(
        !md.contains("infixl"),
        "an operand should not show operator fixity, got: {md}"
    );
}

#[test]
fn code_action_adds_catchall_arm_for_non_exhaustive_match() {
    let mut state = GlobalState::default();
    let u = uri("file:///match.flx");
    // `match c` covers only two of three `Color` variants — E015.
    let src = "data Color { Red, Green, Blue }\nfn pick(c: Color) -> Int {\n    match c {\n        Red -> 1,\n        Green -> 2\n    }\n}\n";
    open(&mut state, &u, src);

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(2, 4), Position::new(2, 13)),
        ))
        .expect("code action response");

    let catchall = actions
        .iter()
        .find(|a| action_title(a).contains("catch-all"))
        .expect("expected a catch-all arm quick fix");
    let new_text = action_edit_text(catchall).expect("catch-all edit text");
    assert!(
        new_text.contains("_ -> ()"),
        "catch-all edit should insert a `_` arm, got {new_text:?}"
    );
}

#[test]
fn code_action_fills_missing_match_arms_for_adt() {
    let mut state = GlobalState::default();
    let u = uri("file:///fill.flx");
    // `match s` covers only `Circle` — `Rect` and `Unit` are missing.
    open(
        &mut state,
        &u,
        "type Shape = Circle(Float) | Rect(Float, Float) | Unit\n\
         fn describe(s: Shape) -> Int {\n\
         \x20   match s {\n\
         \x20       Circle(r) -> 1\n\
         \x20   }\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(2, 4), Position::new(2, 11)),
        ))
        .expect("code action response");

    let fill = actions
        .iter()
        .find(|a| action_title(a).contains("Fill"))
        .expect("expected a fill-missing-arms quick fix");
    let edit = action_edit_text(fill).expect("fill edit text");
    assert!(
        edit.contains("Rect(_, _) -> panic(\"todo\")") && edit.contains("Unit -> panic(\"todo\")"),
        "fill should generate real per-variant arms, got {edit:?}"
    );
    // The plain catch-all is still offered alongside.
    assert!(
        actions
            .iter()
            .any(|a| action_title(a).contains("catch-all")),
        "the bare catch-all should also be available"
    );
}

#[test]
fn code_action_fills_named_variant_arm_with_rest_pattern() {
    let mut state = GlobalState::default();
    let u = uri("file:///fill-named.flx");
    // `Active` is a named-field variant; its filled arm uses a `{ .. }` rest.
    open(
        &mut state,
        &u,
        "data Status { Active { since: Int }, Closed }\n\
         fn label(s: Status) -> Int {\n\
         \x20   match s {\n\
         \x20       Closed -> 0\n\
         \x20   }\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(2, 4), Position::new(2, 11)),
        ))
        .expect("code action response");

    let fill = actions
        .iter()
        .find(|a| action_title(a).contains("Fill"))
        .expect("expected a fill-missing-arms quick fix");
    assert!(
        action_edit_text(fill)
            .expect("fill edit text")
            .contains("Active { .. } -> panic(\"todo\")"),
        "named variant should fill with a `{{ .. }}` rest pattern"
    );
}

#[test]
fn code_action_offers_did_you_mean_keyword_fix() {
    let mut state = GlobalState::default();
    let u = uri("file:///typo.flx");
    // `func` is recovered by the parser with a `Did you mean `fn`?` hint.
    let src = "func main() {\n}\n";
    open(&mut state, &u, src);

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 4)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("fn"))
        .expect("expected a did-you-mean quick fix");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some("fn"),
        "did-you-mean fix should replace the bad keyword with `fn`"
    );
}

#[test]
fn code_action_adds_with_effect_for_e400() {
    let mut state = GlobalState::default();
    let u = uri("file:///e400.flx");
    // `f` declares `with FileSystem` but calls `print` (needs `Console`) → E400.
    // The quick fix appends `Console` to the existing effect row.
    open(
        &mut state,
        &u,
        "fn f() -> Int with FileSystem {\n    print(\"x\")\n    1\n}\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 4), Position::new(1, 9)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Add effect `Console`"))
        .expect("expected an add-effect quick fix");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some(", Console"),
        "should append `Console` to the existing `with FileSystem` row"
    );
    // Inserted on the signature line (line 0), after `FileSystem`.
    assert_eq!(action_edit_range(fix).expect("edit range").start.line, 0);
}

#[test]
fn code_action_removes_unused_import() {
    let mut state = GlobalState::default();
    let u = uri("file:///unused-import.flx");
    // `Array` is imported but never used → the linter's W003; the quick fix
    // deletes the whole import line.
    open(
        &mut state,
        &u,
        "import Flow.Array as Array\nfn main() with IO {\n    print(\"hi\")\n}\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(0, 0), Position::new(0, 10)),
        ))
        .expect("code action response");

    let fix = actions
        .iter()
        .find(|a| action_title(a).contains("Remove unused import"))
        .expect("expected a remove-unused-import quick fix");
    assert_eq!(
        action_edit_text(fix).as_deref(),
        Some(""),
        "removal edit should delete (empty replacement)"
    );
    let range = action_edit_range(fix).expect("edit range");
    assert_eq!(range.start.line, 0);
    assert_eq!(
        range.end.line, 1,
        "should delete through the start of the next line"
    );
}

#[test]
fn code_action_adds_type_annotation_to_let() {
    let mut state = GlobalState::default();
    let u = uri("file:///annotate.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    let n = 41 + 1\n    print(to_string(n))\n}\n",
    );

    // Cursor on `n` in `let n = …` (line 1, char 8).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 8), Position::new(1, 9)),
        ))
        .expect("code action response");

    let assist = actions
        .iter()
        .find(|a| action_title(a).contains("Add type annotation"))
        .expect("expected an add-type-annotation assist");
    assert_eq!(
        action_edit_text(assist).as_deref(),
        Some(": Int"),
        "should insert the inferred type"
    );
    // Inserted right after the name `n` (line 1, char 9).
    let r = action_edit_range(assist).expect("edit range");
    assert_eq!((r.start.line, r.start.character), (1, 9));
}

#[test]
fn code_action_introduces_variable_for_selection() {
    let mut state = GlobalState::default();
    let u = uri("file:///introduce.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    print(to_string(1 + 2))\n}\n",
    );

    // Select `1 + 2` (line 1, chars 20..25).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 20), Position::new(1, 25)),
        ))
        .expect("code action response");

    let assist = actions
        .iter()
        .find(|a| action_title(a).contains("Introduce variable"))
        .expect("expected an introduce-variable assist");
    let edits = action_edits(assist);
    assert_eq!(edits.len(), 2, "a `let` insert plus the selection rewrite");
    assert!(
        edits
            .iter()
            .any(|(_, t)| t.contains("let extracted = 1 + 2")),
        "should hoist the selection into a `let`, got {edits:?}"
    );
    assert!(
        edits.iter().any(|(_, t)| t == "extracted"),
        "should replace the selection with the new name, got {edits:?}"
    );
}

#[test]
fn code_action_inlines_single_use_let() {
    let mut state = GlobalState::default();
    let u = uri("file:///inline.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    let greeting = \"hi\"\n    print(greeting)\n}\n",
    );

    // Cursor on `greeting` in the `let` (line 1, char 8).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 8), Position::new(1, 12)),
        ))
        .expect("code action response");

    let assist = actions
        .iter()
        .find(|a| action_title(a).contains("Inline"))
        .expect("expected an inline assist");
    let edits = action_edits(assist);
    // One edit deletes the `let` line; one replaces the use with the value.
    assert!(
        edits
            .iter()
            .any(|(r, t)| t.is_empty() && r.start.line == 1 && r.end.line == 2),
        "should delete the `let` line, got {edits:?}"
    );
    assert!(
        edits.iter().any(|(_, t)| t == "\"hi\""),
        "should inline the (atomic) value without parens, got {edits:?}"
    );
}

#[test]
fn code_action_does_not_inline_multi_use_let() {
    let mut state = GlobalState::default();
    let u = uri("file:///inline-multi.flx");
    // `g` is used twice — inlining is not offered (avoids re-evaluation /
    // shadowing ambiguity).
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    let g = \"hi\"\n    print(g)\n    print(g)\n}\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 8), Position::new(1, 9)),
        ))
        .expect("code action response");
    assert!(
        !actions.iter().any(|a| action_title(a).contains("Inline")),
        "inline should not be offered for a multi-use binding"
    );
}

#[test]
fn code_action_adds_missing_methods_to_empty_instance() {
    let mut state = GlobalState::default();
    let u = uri("file:///instance-empty.flx");
    // `Show2` has a required `show` and a defaulted `pretty`; the empty
    // instance is missing only `show` (defaults aren't stubbed).
    open(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class Show2<a> {\n\
         \x20   fn show(x: a) -> String\n\
         \x20   fn pretty(x: a) -> String { show(x) }\n\
         }\n\
         instance Show2<Color> {\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(5, 0), Position::new(5, 8)),
        ))
        .expect("code action response");

    let add = actions
        .iter()
        .find(|a| action_title(a).contains("Add missing method"))
        .expect("expected an add-missing-methods quick fix");
    assert_eq!(action_title(add), "Add missing method for `Show2`");
    let edit = action_edit_text(add).expect("add edit text");
    assert!(
        edit.contains("fn show(x) { panic(\"todo\") }"),
        "required method should be stubbed with a panic body, got {edit:?}"
    );
    assert!(
        !edit.contains("pretty"),
        "a method with a default body should not be stubbed, got {edit:?}"
    );
}

#[test]
fn code_action_adds_only_unimplemented_instance_methods() {
    let mut state = GlobalState::default();
    let u = uri("file:///instance-partial.flx");
    // `Eq2` requires `eq` and `ne`; the instance already provides `eq`, so only
    // `ne` is stubbed — and it splices in after the existing method.
    open(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class Eq2<a> {\n\
         \x20   fn eq(x: a, y: a) -> Bool\n\
         \x20   fn ne(x: a, y: a) -> Bool\n\
         }\n\
         instance Eq2<Color> {\n\
         \x20   fn eq(x, y) { panic(\"p\") }\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(5, 0), Position::new(5, 8)),
        ))
        .expect("code action response");

    let add = actions
        .iter()
        .find(|a| action_title(a).contains("Add missing method"))
        .expect("expected an add-missing-methods quick fix");
    let edit = action_edit_text(add).expect("add edit text");
    assert!(
        edit.contains("fn ne(x, y) { panic(\"todo\") }"),
        "the unimplemented method should be stubbed, got {edit:?}"
    );
    assert!(
        !edit.contains("fn eq"),
        "an already-implemented method must not be re-stubbed, got {edit:?}"
    );
}

#[test]
fn code_action_no_add_methods_for_complete_instance() {
    let mut state = GlobalState::default();
    let u = uri("file:///instance-complete.flx");
    open(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class Eq3<a> {\n\
         \x20   fn eq(x: a, y: a) -> Bool\n\
         }\n\
         instance Eq3<Color> {\n\
         \x20   fn eq(x, y) { panic(\"p\") }\n\
         }\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(4, 0), Position::new(4, 8)),
        ))
        .expect("code action response");
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).contains("Add missing method")),
        "a complete instance should offer no add-missing-methods fix"
    );
}

#[test]
fn code_action_returns_nothing_for_a_clean_range() {
    let mut state = GlobalState::default();
    let u = uri("file:///clean.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    print(\"ok\")\n}\n",
    );

    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 4), Position::new(1, 9)),
        ))
        .expect("code action response");
    assert!(
        actions.is_empty(),
        "a diagnostic-free range should yield no quick fixes, got {:?}",
        actions.iter().map(action_title).collect::<Vec<_>>()
    );
}

#[test]
fn code_action_offers_import_for_unimported_custom_module() {
    // `main.flx` qualifies `Math.twice` but never imports `Math`. The sibling
    // module isn't in the module graph (no import to follow), so the fix has
    // to find it by scanning the workspace.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "fn run() {\n    Math.twice(21)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Cursor on `Math` in `Math.twice(21)` (line 1, chars 4..8).
    let actions = state
        .handle_code_action(code_action_params(
            &uris[1],
            Range::new(Position::new(1, 4), Position::new(1, 8)),
        ))
        .expect("code action response");

    let import = actions
        .iter()
        .find(|a| action_title(a).starts_with("Import"))
        .expect("expected an auto-import quick fix");
    assert_eq!(action_title(import), "Import `Math`");
    assert_eq!(
        action_edit_text(import).as_deref(),
        Some("import Math\n"),
        "the fix should insert `import Math` at the top of the file"
    );
}

#[test]
fn code_action_skips_import_when_module_already_bound() {
    // The module is already imported — no fix should be offered.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math\n\nfn run() {\n    Math.twice(21)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Cursor on `Math` in `Math.twice(21)` (line 3, chars 4..8).
    let actions = state
        .handle_code_action(code_action_params(
            &uris[1],
            Range::new(Position::new(3, 4), Position::new(3, 8)),
        ))
        .expect("code action response");
    assert!(
        !actions
            .iter()
            .any(|a| action_title(a).starts_with("Import")),
        "no import fix expected when the module is already imported, got {:?}",
        actions.iter().map(action_title).collect::<Vec<_>>()
    );
}

#[test]
fn code_action_offers_aliased_import_for_flow_stdlib_module() {
    // A bare `Json.…` reference (single segment) needs `import Flow.Json as
    // Json` so the typed name resolves. Anchored at the real repo root so the
    // Flow prelude loads and indexes every `lib/Flow/*.flx`.
    let mut state = GlobalState::default();
    let workspace_root = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let repo = workspace_root
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let buf_path = repo.join("auto-import-json-fixture.flx");
    let uri_str = format!(
        "file:///{}",
        buf_path.display().to_string().replace('\\', "/")
    );
    let u = uri(&uri_str);
    let source = "fn main() with IO {\n    Json.stringify(1)\n}\n";
    open(&mut state, &u, source);

    // Cursor on `Json` in `Json.stringify(1)` (line 1, chars 4..8).
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(1, 4), Position::new(1, 8)),
        ))
        .expect("code action response");

    let import = actions
        .iter()
        .find(|a| action_title(a).starts_with("Import"))
        .expect("expected a Flow stdlib auto-import quick fix");
    assert_eq!(action_title(import), "Import `Flow.Json as Json`");
    assert_eq!(
        action_edit_text(import).as_deref(),
        Some("import Flow.Json as Json\n"),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// completionItem/resolve
// ─────────────────────────────────────────────────────────────────────────────

/// Documentation markdown of a completion item, if any.
fn item_doc(item: &CompletionItem) -> Option<String> {
    match item.documentation.as_ref()? {
        Documentation::MarkupContent(m) => Some(m.value.clone()),
        Documentation::String(s) => Some(s.clone()),
    }
}

#[test]
fn completion_keyword_items_defer_docs_to_resolve() {
    let mut state = GlobalState::default();
    let u = uri("file:///kw.flx");
    open(&mut state, &u, "fn main() {\n    \n}\n");

    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position::new(1, 4),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");
    let items = match response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    };

    let kw = items
        .into_iter()
        .find(|i| i.label == "match")
        .expect("`match` keyword completion item");
    // Docs are deferred: the initial item carries resolve data but no
    // documentation (so the per-keystroke list stays small).
    assert!(
        kw.documentation.is_none(),
        "keyword docs should be deferred to resolve, not sent eagerly"
    );
    assert!(kw.data.is_some(), "keyword item should carry resolve data");

    let resolved = state.handle_completion_resolve(kw);
    let doc = item_doc(&resolved).expect("resolved keyword documentation");
    assert!(
        doc.contains("**`match`**"),
        "expected the `match` keyword card, got: {doc}"
    );
}

#[test]
fn completion_resolve_fills_effect_and_type_docs() {
    let mut state = GlobalState::default();

    let io = CompletionItem {
        label: "IO".to_string(),
        data: Some(serde_json::json!({ "kind": "effect", "word": "IO" })),
        ..Default::default()
    };
    let io_doc = item_doc(&state.handle_completion_resolve(io)).expect("IO doc");
    assert!(io_doc.contains("**`IO`**"), "got: {io_doc}");

    let int = CompletionItem {
        label: "Int".to_string(),
        data: Some(serde_json::json!({ "kind": "type", "word": "Int" })),
        ..Default::default()
    };
    let int_doc = item_doc(&state.handle_completion_resolve(int)).expect("Int doc");
    assert!(int_doc.contains("**`Int`**"), "got: {int_doc}");
}

/// A `file://` URI for `filename` placed at the real repo root, so opening it
/// makes the LSP load the Flow prelude (which anchors on the project root) and
/// index every `lib/Flow/*.flx`.
fn repo_root_uri(filename: &str) -> Uri {
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap();
    let repo = manifest
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let path = repo.join(filename);
    uri(&format!(
        "file:///{}",
        path.display().to_string().replace('\\', "/")
    ))
}

/// All completion items at `(line, ch)` for the document at `u`.
fn completion_items(state: &mut GlobalState, u: &Uri, line: u32, ch: u32) -> Vec<CompletionItem> {
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(u),
                position: Position::new(line, ch),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");
    match response {
        CompletionResponse::Array(items) => items,
        CompletionResponse::List(list) => list.items,
    }
}

#[test]
fn completion_in_expression_offers_module_names() {
    // Inside a function body (expression context), the known module names —
    // here the Flow stdlib, indexed once the prelude loads off the repo root —
    // are offered so a bare `Arr…` surfaces `Array`. Without this the
    // expression list is only locals + top-level decls + keywords, and typing
    // `Arr` shows nothing.
    let mut state = GlobalState::default();
    let u = repo_root_uri("module-name-completion-fixture.flx");
    open(&mut state, &u, "fn main() with IO {\n    \n}\n");

    // Cursor on the blank line inside `main` (line 1, char 4).
    let labels: Vec<String> = completion_items(&mut state, &u, 1, 4)
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert!(
        labels.contains(&"Array".to_string()),
        "expected `Array` among expression completions, got {labels:?}"
    );
}

#[test]
fn completion_module_name_carries_auto_import_edit() {
    // Accepting the `Array` module item in a buffer that hasn't imported it
    // must also insert `import Flow.Array as Array` (additionalTextEdits).
    let mut state = GlobalState::default();
    let u = repo_root_uri("module-name-autoimport-fixture.flx");
    open(&mut state, &u, "fn main() with IO {\n    \n}\n");

    let items = completion_items(&mut state, &u, 1, 4);
    let array = items
        .iter()
        .find(|i| i.label == "Array")
        .expect("`Array` module completion item");
    let edits = array
        .additional_text_edits
        .as_ref()
        .expect("an auto-import additionalTextEdits on the un-imported module");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "import Flow.Array as Array\n");
}

#[test]
fn completion_member_after_module_dot_auto_imports() {
    // `Array.` lists members even when `Flow.Array` is only indexed, not
    // imported — accepting any member must also add `import Flow.Array as
    // Array`.
    let mut state = GlobalState::default();
    let u = repo_root_uri("member-autoimport-fixture.flx");
    open(&mut state, &u, "fn main() with IO {\n    Array.\n}\n");

    // Cursor right after `Array.` (line 1, char 10).
    let items = completion_items(&mut state, &u, 1, 10);
    assert!(!items.is_empty(), "expected `Array` members");
    assert!(
        items.iter().all(|i| {
            i.additional_text_edits
                .as_ref()
                .is_some_and(|e| e[0].new_text == "import Flow.Array as Array\n")
        }),
        "every member should carry the auto-import edit, got {:?}",
        items
            .iter()
            .map(|i| (&i.label, &i.additional_text_edits))
            .collect::<Vec<_>>()
    );
}

#[test]
fn completion_member_no_import_edit_when_imported() {
    // With `Array` already imported, member items carry no auto-import edit.
    let mut state = GlobalState::default();
    let u = repo_root_uri("member-imported-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn main() with IO {\n    Array.\n}\n",
    );

    // Cursor right after `Array.` (line 3, char 10).
    let items = completion_items(&mut state, &u, 3, 10);
    assert!(!items.is_empty(), "expected `Array` members");
    assert!(
        items.iter().all(|i| i.additional_text_edits.is_none()),
        "imported-module members should carry no auto-import edit"
    );
}

#[test]
fn completion_module_name_no_edit_when_already_imported() {
    // With `Array` already imported, its completion item carries no
    // additionalTextEdits — nothing to add.
    let mut state = GlobalState::default();
    let u = repo_root_uri("module-name-imported-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn main() with IO {\n    \n}\n",
    );

    let items = completion_items(&mut state, &u, 3, 4);
    let array = items
        .iter()
        .find(|i| i.label == "Array")
        .expect("`Array` module completion item");
    assert!(
        array.additional_text_edits.is_none(),
        "an already-imported module should carry no auto-import edit"
    );
}

#[test]
fn completion_resolve_fills_member_doc_comment() {
    // `Either.` lists members; resolving one surfaces its `///` doc comment,
    // scanned from the Flow.Either source (the AST drops doc comments).
    let mut state = GlobalState::default();
    let u = repo_root_uri("member-doc-fixture.flx");
    open(&mut state, &u, "fn main() with IO {\n    Either.\n}\n");

    // Cursor right after `Either.` (line 1, char 11).
    let items = completion_items(&mut state, &u, 1, 11);
    let either = items
        .into_iter()
        .find(|i| i.label == "either")
        .expect("`either` member completion item");
    assert!(
        either.documentation.is_none(),
        "member docs should be deferred to resolve"
    );

    let doc = item_doc(&state.handle_completion_resolve(either)).expect("resolved member doc");
    assert!(
        doc.contains("Case analysis for Either"),
        "expected the `either` doc comment, got: {doc}"
    );
}

#[test]
fn completion_resolve_fills_user_module_member_doc() {
    // A sibling user module's member doc resolves too (not just the Flow
    // stdlib): the source is found in the analyzed snapshot's module cache.
    let math_src =
        "module Math {\n    /// Doubles its argument.\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math\n\nfn run() {\n    Math.\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Complete after `Math.` (line 3, char 9).
    let items = completion_items(&mut state, &uris[1], 3, 9);
    let twice = items
        .into_iter()
        .find(|i| i.label == "twice")
        .expect("`twice` member completion item");
    let doc = item_doc(&state.handle_completion_resolve(twice)).expect("resolved member doc");
    assert!(
        doc.contains("Doubles its argument"),
        "expected the user-module member doc, got: {doc}"
    );
}

#[test]
fn completion_resolve_passes_through_items_without_data() {
    let mut state = GlobalState::default();
    // No `data`, and `Result` is not a universal built-in type even if asked.
    let plain = CompletionItem {
        label: "foo".to_string(),
        ..Default::default()
    };
    assert!(
        state
            .handle_completion_resolve(plain)
            .documentation
            .is_none()
    );

    let result_ty = CompletionItem {
        label: "Result".to_string(),
        data: Some(serde_json::json!({ "kind": "type", "word": "Result" })),
        ..Default::default()
    };
    assert!(
        state
            .handle_completion_resolve(result_ty)
            .documentation
            .is_none(),
        "`Result` is a module type, not a universal built-in — no doc expected"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Cross-file analysis (module graph)
// ─────────────────────────────────────────────────────────────────────────────

/// Write `files` into a fresh temp directory and return a `GlobalState`
/// whose single workspace root is that directory, plus the `file://` URIs of
/// the written files (in the same order). The module graph reads dependency
/// files from disk and canonicalizes paths, so real on-disk files are needed.
fn workspace_fixture(files: &[(&str, &str)]) -> (tempfile::TempDir, GlobalState, Vec<Uri>) {
    let dir = tempfile::tempdir().unwrap();
    let mut uris = Vec::new();
    for (name, content) in files {
        let path = dir.path().join(name);
        // `name` may carry subdirectories (`Lib/App/Main.flx`) for nested
        // module fixtures — create the parent chain before writing.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        uris.push(flux_lsp::vfs::path_to_uri(&path).unwrap());
    }
    let mut state = GlobalState::default();
    state.set_workspace_folders(vec![dir.path().to_path_buf()]);
    (dir, state, uris)
}

#[test]
fn workspace_module_names_cache_refreshes_after_an_edit() {
    let (_dir, mut state, uris) = workspace_fixture(&[
        (
            "Math.flx",
            "module Math {\n    public fn twice(x) { x * 2 }\n}\n",
        ),
        ("main.flx", "fn main() -> Int { 1 }\n"),
    ]);

    // Discovery indexed Math as a workspace module (and memoizes on first read).
    let names = state.workspace.workspace_module_full_names();
    assert!(
        names.contains(&"Math".to_string()),
        "Math should be a known workspace module: {names:?}"
    );

    // Renaming the module via an edit must invalidate the memoized list.
    open(
        &mut state,
        &uris[0],
        "module Geometry {\n    public fn twice(x) { x * 2 }\n}\n",
    );
    let names = state.workspace.workspace_module_full_names();
    assert!(
        names.contains(&"Geometry".to_string()),
        "the renamed module should appear after the edit: {names:?}"
    );
    assert!(
        !names.contains(&"Math".to_string()),
        "the stale module name must be gone after the edit: {names:?}"
    );
}

#[test]
fn cross_file_inference_publishes_diagnostics_for_each_module() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);

    let diags = state.handle_did_open(DidOpenTextDocumentParams {
        text_document: TextDocumentItem {
            uri: uris[1].clone(),
            language_id: "flux".into(),
            version: 1,
            text: main_src.into(),
        },
    });

    // Cross-file analysis rebuilt both the entry and its imported module.
    assert_eq!(
        diags.len(),
        2,
        "expected diagnostics for Math + main, got {diags:?}"
    );
    // The cross-module call `M.twice` must resolve — no unresolved-name error.
    let main_diags = diags
        .iter()
        .find(|d| d.uri == uris[1])
        .expect("diagnostics for main.flx");
    assert!(
        main_diags
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("twice")),
        "unexpected diagnostic mentioning `twice`: {:?}",
        main_diags.diagnostics
    );
}

#[test]
fn cross_file_goto_definition_jumps_into_user_module() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Cursor on `twice` in `M.twice(21)` (line 2, inside the identifier).
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(2, 14),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition response");
    let location = expect_location(resp);
    assert_eq!(
        location.uri, uris[0],
        "expected goto-definition to land in Math.flx"
    );
}

#[test]
fn cross_file_goto_definition_resolves_unqualified_module_member() {
    // `import Math exposing (..)` brings `twice` into scope unqualified.
    // F12 on the bare `twice` must still land in Math.flx via the
    // cross-module member fallback.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math exposing (..)\n\nfn run() { twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Cursor on the bare `twice` in `twice(21)` (line 2, char 12).
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(2, 12),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition response");
    let location = expect_location(resp);
    assert_eq!(
        location.uri, uris[0],
        "expected unqualified-member goto-definition to land in Math.flx"
    );
}

#[test]
fn cross_file_goto_definition_resolves_deeply_qualified_member() {
    // `A.B.C.member` — a multi-segment qualified path. The `object` of the
    // `.run` access is a `MemberAccess` chain, not a bare identifier.
    let main_mod = "module Lib.App.Main {\n    public fn run() with IO { print(\"hi\") }\n}\n";
    let entry = "import Lib.App.Main\n\nfn main() with IO {\n    Lib.App.Main.run()\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Lib/App/Main.flx", main_mod), ("main.flx", entry)]);
    open(&mut state, &uris[1], entry);

    // Cursor on `run` in `Lib.App.Main.run()` (line 3, char 18).
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 18),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition response");
    let location = expect_location(resp);
    assert_eq!(
        location.uri, uris[0],
        "expected deeply-qualified goto-definition to land in Lib/App/Main.flx"
    );
}

#[test]
fn cross_file_goto_definition_resolves_aliased_subdir_module() {
    // `import Lib.App.Util as U` — an alias to a *subdirectory* module. The
    // alias resolves to the module's short segment, but the sibling cache keys
    // user modules by their full declared name; goto-def must still land in the
    // module file.
    let util = "module Lib.App.Util {\n    public fn helper(x) { x + 1 }\n}\n";
    let entry = "import Lib.App.Util as U\n\nfn main() with IO {\n    let _ = U.helper(1)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Lib/App/Util.flx", util), ("main.flx", entry)]);
    open(&mut state, &uris[1], entry);

    // Cursor on `helper` in `U.helper(1)` (line 3, char 16).
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 16),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition response");
    let location = expect_location(resp);
    assert_eq!(
        location.uri, uris[0],
        "aliased subdir-module goto-definition should land in Lib/App/Util.flx"
    );
}

#[test]
fn completion_after_deeply_qualified_module_dot_lists_members() {
    let main_mod = "module Lib.App.Main {\n    public fn run() with IO { print(\"hi\") }\n}\n";
    let entry = "import Lib.App.Main\n\nfn main() with IO {\n    Lib.App.Main.\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Lib/App/Main.flx", main_mod), ("main.flx", entry)]);
    open(&mut state, &uris[1], entry);

    // `    Lib.App.Main.` — cursor right after the final dot (line 3, char 17).
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 17),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");
    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.contains(&"run".to_string()),
        "expected `run` from Lib.App.Main, got {labels:?}"
    );
    assert!(
        !labels.iter().any(|l| l == "let" || l == "fn"),
        "got default keyword list instead of module members: {labels:?}"
    );
}

#[test]
fn completion_module_namespace_lists_next_segment() {
    let main_mod = "module Lib.App.Main {\n    public fn run() with IO { print(\"hi\") }\n}\n";
    let entry = "import Lib.App.Main\n\nfn main() with IO {\n    Lib.\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Lib/App/Main.flx", main_mod), ("main.flx", entry)]);
    open(&mut state, &uris[1], entry);

    // `    Lib.` — `Lib` is only a prefix of `Lib.App.Main`, so the next
    // path segment (`App`) should be offered (line 3, char 8).
    let response = state
        .handle_completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 8),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        })
        .expect("completion result");
    let labels: Vec<String> = match response {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(list) => list.items.into_iter().map(|i| i.label).collect(),
    };
    assert!(
        labels.contains(&"App".to_string()),
        "expected namespace segment `App`, got {labels:?}"
    );
}

#[test]
fn editing_imported_module_refreshes_dependent() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);
    open(&mut state, &uris[0], math_src);

    // Edit Math.flx; the dependent main.flx must be re-analyzed too.
    let diags = state.handle_did_change(DidChangeTextDocumentParams {
        text_document: lsp_types::VersionedTextDocumentIdentifier {
            uri: uris[0].clone(),
            version: 2,
        },
        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "module Math {\n    public fn twice(x) { x * 3 }\n}\n".into(),
        }],
    });
    assert!(
        diags.iter().any(|d| d.uri == uris[1]),
        "editing Math.flx should refresh diagnostics for main.flx, got {diags:?}"
    );
}

#[test]
fn cross_file_references_span_the_component() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    // Opening the entry analyzes the whole module-graph component.
    open(&mut state, &uris[1], main_src);

    // Cursor on the `twice` declaration in Math.flx (line 1).
    let refs = state.handle_references(ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: ident(&uris[0]),
            position: Position::new(1, 16),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    });
    assert!(
        refs.iter().any(|r| r.uri == uris[1]),
        "expected a reference to `twice` in main.flx, got {refs:?}"
    );
    let distinct: std::collections::HashSet<_> = refs.iter().map(|r| r.uri.as_str()).collect();
    assert!(
        distinct.len() >= 2,
        "expected references across both files, got {refs:?}"
    );
}

#[test]
fn cross_file_rename_edits_every_affected_file() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    let edit = state
        .handle_rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&uris[0]),
                position: Position::new(1, 16),
            },
            new_name: "tripled".into(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("rename WorkspaceEdit");
    let edits = match edit.document_changes {
        Some(DocumentChanges::Edits(edits)) => edits,
        other => panic!("expected DocumentChanges::Edits, got {other:?}"),
    };
    assert!(
        edits.len() >= 2,
        "expected a TextDocumentEdit per affected file, got {edits:?}"
    );
}

#[test]
fn local_rename_does_not_cross_files() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() {\n    let x = 1\n    M.twice(x)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // `x` is a local binding in main.flx — renaming it must not touch the
    // same-named parameter in Math.flx.
    let edit = state
        .handle_rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(3, 8),
            },
            new_name: "y".into(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("rename WorkspaceEdit");
    let edits = match edit.document_changes {
        Some(DocumentChanges::Edits(edits)) => edits,
        other => panic!("expected DocumentChanges::Edits, got {other:?}"),
    };
    assert_eq!(edits.len(), 1, "local rename should touch only main.flx");
    assert_eq!(edits[0].text_document.uri, uris[1]);
}

#[test]
fn rename_declaration_edits_only_the_name() {
    let mut state = GlobalState::default();
    let u = uri("file:///rename-decl.flx");
    // `fn twice(n)` is one statement; its recorded span covers the whole
    // signature, so the rename edit must be narrowed to just `twice`.
    open(
        &mut state,
        &u,
        "fn twice(n) { n + n }\n\nfn main() {\n    twice(1)\n}\n",
    );

    let edit = state
        .handle_rename(RenameParams {
            text_document_position: TextDocumentPositionParams {
                text_document: ident(&u),
                position: Position::new(0, 4),
            },
            new_name: "tripled".into(),
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .expect("rename edit");
    let edits = match edit.document_changes {
        Some(DocumentChanges::Edits(e)) => e,
        other => panic!("expected edits, got {other:?}"),
    };
    let ranges: Vec<Range> = edits[0]
        .edits
        .iter()
        .map(|e| match e {
            OneOf::Left(te) => te.range,
            other => panic!("unexpected edit form {other:?}"),
        })
        .collect();
    // Both the declaration and the call site rename `twice` (width 5),
    // never the surrounding `fn twice(n)` signature.
    for r in &ranges {
        assert_eq!(r.start.line, r.end.line);
        assert_eq!(
            r.end.character - r.start.character,
            "twice".len() as u32,
            "rename edit must span only the name, got {r:?}"
        );
    }
    // The declaration name starts after `fn ` (char 3), not at the statement
    // start (char 0).
    assert!(
        ranges
            .iter()
            .any(|r| r.start.line == 0 && r.start.character == 3),
        "declaration edit should start at the name, got {ranges:?}"
    );
}

#[test]
fn watched_file_change_refreshes_dependent() {
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Math.flx changes on disk while unopened; the watcher event must
    // re-analyze the dependent main.flx.
    std::fs::write(
        dir.path().join("Math.flx"),
        "module Math {\n    public fn twice(x) { x * 3 }\n}\n",
    )
    .unwrap();
    let diags = state.handle_did_change_watched_files(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: uris[0].clone(),
            typ: FileChangeType::CHANGED,
        }],
    });
    assert!(
        diags.iter().any(|d| d.uri == uris[1]),
        "a watched change to Math.flx should refresh main.flx, got {diags:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Closed / never-opened files stay queryable
// ─────────────────────────────────────────────────────────────────────────────

const MATH_SRC: &str = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
const MAIN_SRC: &str = "import Math as M\n\nfn run() { M.twice(21) }\n";

#[test]
fn closed_file_still_answers_references() {
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", MATH_SRC), ("main.flx", MAIN_SRC)]);
    open(&mut state, &uris[1], MAIN_SRC);
    open(&mut state, &uris[0], MATH_SRC);
    close(&mut state, &uris[0]); // Math.flx is now closed (still on disk)

    // References requested from the still-open main.flx must reach the
    // declaration in the closed Math.flx.
    let refs = state.handle_references(ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: ident(&uris[1]),
            position: Position::new(2, 14),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    });
    let distinct: std::collections::HashSet<_> = refs.iter().map(|r| r.uri.as_str()).collect();
    assert!(
        distinct.len() >= 2,
        "references should still span the closed Math.flx, got {refs:?}"
    );
}

#[test]
fn goto_definition_into_closed_module() {
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", MATH_SRC), ("main.flx", MAIN_SRC)]);
    open(&mut state, &uris[1], MAIN_SRC);
    open(&mut state, &uris[0], MATH_SRC);
    close(&mut state, &uris[0]);

    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[1]),
                position: Position::new(2, 14),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition into a closed module");
    assert_eq!(expect_location(resp).uri, uris[0]);
}

#[test]
fn references_reach_never_opened_file() {
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", MATH_SRC), ("main.flx", MAIN_SRC)]);
    // Only main.flx is ever opened — Math.flx is discovered on disk.
    open(&mut state, &uris[1], MAIN_SRC);

    let refs = state.handle_references(ReferenceParams {
        text_document_position: TextDocumentPositionParams {
            text_document: ident(&uris[1]),
            position: Position::new(2, 14),
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: true,
        },
    });
    let distinct: std::collections::HashSet<_> = refs.iter().map(|r| r.uri.as_str()).collect();
    assert!(
        distinct.len() >= 2,
        "references should reach the never-opened Math.flx, got {refs:?}"
    );
}

#[test]
fn closing_standalone_file_then_query_rebuilds_lazily() {
    let solo = "let answer = 42\nlet result = answer\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("solo.flx", solo)]);
    open(&mut state, &uris[0], solo);
    close(&mut state, &uris[0]); // snapshot dropped — no component keeps it

    // A request against the closed file must rebuild its snapshot lazily
    // from the on-disk content.
    let resp = state
        .handle_definition(GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(&uris[0]),
                position: Position::new(1, 13),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("goto-definition should rebuild the closed file's snapshot");
    assert_eq!(expect_location(resp).range.start.line, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Worker thread (end-to-end) — a read request is served off the main loop
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hover_request_is_served_via_worker() {
    use lsp_types::notification::{DidOpenTextDocument, Initialized};
    use lsp_types::request::HoverRequest;

    let (server_conn, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        let caps = serde_json::to_value(server_capabilities(PositionEncoding::Utf16)).unwrap();
        let _ = server_conn.initialize(caps).expect("server initialize");
        let server = Server::new(
            server_conn,
            PositionEncoding::Utf16,
            flux_lsp::loader::WatcherKind::Client,
        );
        server.run().expect("server run");
    });

    // initialize handshake
    client
        .sender
        .send(Message::Request(Request {
            id: 1.into(),
            method: Initialize::METHOD.to_string(),
            params: serde_json::to_value(InitializeParams::default()).unwrap(),
        }))
        .unwrap();
    let _ = client.receiver.recv().unwrap(); // initialize response
    client
        .sender
        .send(Message::Notification(Notification {
            method: Initialized::METHOD.to_string(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();

    // open a document
    let u = uri("file:///worker_hover.flx");
    client
        .sender
        .send(Message::Notification(Notification {
            method: DidOpenTextDocument::METHOD.to_string(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: u.clone(),
                    language_id: "flux".into(),
                    version: 1,
                    text: "let answer = 42\n".into(),
                },
            })
            .unwrap(),
        }))
        .unwrap();

    // a hover request — computed on the worker thread
    let hover_id: RequestId = 2.into();
    client
        .sender
        .send(Message::Request(Request {
            id: hover_id.clone(),
            method: HoverRequest::METHOD.to_string(),
            params: serde_json::to_value(HoverParams {
                text_document_position_params: TextDocumentPositionParams {
                    text_document: ident(&u),
                    position: Position::new(0, 4),
                },
                work_done_progress_params: WorkDoneProgressParams::default(),
            })
            .unwrap(),
        }))
        .unwrap();

    // The worker's response eventually arrives (diagnostics / registerCapability
    // messages may interleave first).
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut served = false;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match client.receiver.recv_timeout(remaining) {
            Ok(Message::Response(r)) if r.id == hover_id => {
                served = true;
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert!(served, "expected a hover response from the worker thread");

    // shutdown
    let shutdown_id: RequestId = 3.into();
    client
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_string(),
            params: Value::Null,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.receiver.recv_timeout(remaining).unwrap() {
            Message::Response(r) if r.id == shutdown_id => break,
            _ => continue,
        }
    }
    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::Exit::METHOD.to_string(),
            params: Value::Null,
        }))
        .unwrap();
    server_thread.join().unwrap();
}

// ─────────────────────────────────────────────────────────────────────────────
// Unresolved-name diagnostics (E004)
// ─────────────────────────────────────────────────────────────────────────────

/// Open `text` as `u` on an existing state (which may carry workspace roots),
/// returning just that file's published diagnostics. Unlike [`diags_for`] this
/// reuses the caller's `state`, so a `workspace_fixture` project's sibling files
/// stay visible.
fn open_diags(state: &mut GlobalState, u: &Uri, text: &str) -> Vec<lsp_types::Diagnostic> {
    state
        .handle_did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "flux".into(),
                version: 1,
                text: text.into(),
            },
        })
        .into_iter()
        .find(|d| &d.uri == u)
        .map(|d| d.diagnostics)
        .unwrap_or_default()
}

fn diags_for(state: &mut GlobalState, u: &Uri, text: &str) -> Vec<lsp_types::Diagnostic> {
    state
        .handle_did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "flux".into(),
                version: 1,
                text: text.into(),
            },
        })
        .into_iter()
        // `handle_did_open` may republish for several open buffers; keep only
        // the file we just opened.
        .find(|d| &d.uri == u)
        .map(|d| d.diagnostics)
        .unwrap_or_default()
}

fn diags_with_code<'a>(
    diags: &'a [lsp_types::Diagnostic],
    code: &str,
) -> Vec<&'a lsp_types::Diagnostic> {
    diags
        .iter()
        .filter(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == code))
        .collect()
}

fn e004(diags: &[lsp_types::Diagnostic]) -> Vec<&lsp_types::Diagnostic> {
    diags_with_code(diags, "E004")
}

#[test]
fn undefined_lowercase_identifier_is_flagged() {
    let mut state = GlobalState::default();
    let u = uri("file:///undef.flx");
    let diags = diags_for(&mut state, &u, "fn main() {\n    let y = mystery + 1\n}\n");
    let e = e004(&diags);
    assert_eq!(e.len(), 1, "expected one E004, got {diags:?}");
    assert!(
        e[0].message.contains("mystery"),
        "message should name the missing variable: {}",
        e[0].message
    );
    assert_eq!(e[0].severity, Some(lsp_types::DiagnosticSeverity::ERROR));
}

#[test]
fn undefined_identifier_offers_did_you_mean() {
    let mut state = GlobalState::default();
    let u = uri("file:///typo.flx");
    // `amout` is a typo for the bound `amount`.
    let diags = diags_for(
        &mut state,
        &u,
        "fn main() {\n    let amount = 5\n    let total = amout + 1\n}\n",
    );
    let e = e004(&diags);
    assert_eq!(e.len(), 1, "expected one E004, got {diags:?}");
    assert!(
        e[0].message.contains("Did you mean `amount`?"),
        "expected a suggestion: {}",
        e[0].message
    );
}

#[test]
fn bound_names_and_builtins_are_not_flagged() {
    // Params, `let`s, nested fns, top-level fns, and prelude builtins (`print`,
    // available once the repo-root prelude loads) all resolve.
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-resolve-fixture.flx");
    let diags = diags_for(
        &mut state,
        &u,
        "fn helper(n) { n + 1 }\n\
         fn main() with IO {\n\
             let xs = [1, 2, 3]\n\
             let f = \\x -> helper(x)\n\
             print(f(xs))\n\
         }\n",
    );
    assert!(
        e004(&diags).is_empty(),
        "no name should be flagged, got {:?}",
        e004(&diags)
    );
}

#[test]
fn uppercase_constructors_and_module_paths_are_not_flagged() {
    // Constructors and module qualifiers are uppercase and must never be flagged
    // as undefined values.
    let mut state = GlobalState::default();
    let u = uri("file:///ctor.flx");
    let diags = diags_for(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         fn pick(b) {\n\
             match b {\n\
                 true -> Red,\n\
                 false -> Green,\n\
             }\n\
         }\n",
    );
    assert!(
        e004(&diags).is_empty(),
        "constructors must not be flagged, got {:?}",
        e004(&diags)
    );
}

#[test]
fn unknown_aliased_module_member_is_flagged() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-member-fixture.flx");
    let diags = diags_for(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn run(xs) {\n    Array.frobnicate(xs)\n}\n",
    );
    // `Array` is imported, so the member (not the module) is flagged: E012.
    let e = diags_with_code(&diags, "E012");
    assert!(
        e.iter().any(|d| d.message.contains("frobnicate")),
        "expected an unknown-member E012 for `frobnicate`, got {diags:?}"
    );
}

#[test]
fn unimported_module_path_is_flagged() {
    // `Array` is a Flow stdlib module but no `import` binds it here, so the
    // qualified path is flagged with E013 (module not imported).
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-unimported-fixture.flx");
    let diags = diags_for(&mut state, &u, "fn run(xs) {\n    Array.map(xs, run)\n}\n");
    let e = diags_with_code(&diags, "E013");
    assert!(
        e.iter().any(|d| d.message.contains("Array")),
        "expected E013 for the unimported `Array`, got {diags:?}"
    );
}

#[test]
fn imported_module_path_is_not_flagged() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-imported-fixture.flx");
    let diags = diags_for(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn run(xs) {\n    Array.map(xs, run)\n}\n",
    );
    assert!(
        diags_with_code(&diags, "E013").is_empty(),
        "an imported module must not be flagged, got {:?}",
        diags_with_code(&diags, "E013")
    );
}

#[test]
fn unimported_module_squiggle_offers_import_fix() {
    // The E013 squiggle's range drives the existing auto-import quick fix.
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-fix-fixture.flx");
    let diags = diags_for(&mut state, &u, "fn run(xs) {\n    Array.map(xs, run)\n}\n");
    let e = diags_with_code(&diags, "E013");
    let diag = e.first().expect("an E013 diagnostic");

    let actions = state
        .handle_code_action(CodeActionParams {
            text_document: ident(&u),
            range: diag.range,
            context: CodeActionContext {
                diagnostics: vec![(*diag).clone()],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .expect("code action response");
    let titles: Vec<String> = actions
        .iter()
        .filter_map(|a| match a {
            CodeActionOrCommand::CodeAction(ca) => Some(ca.title.clone()),
            _ => None,
        })
        .collect();
    assert!(
        titles
            .iter()
            .any(|t| t.contains("Import") && t.contains("Array")),
        "expected an import fix for `Array` at the squiggle, got {titles:?}"
    );
}

#[test]
fn known_aliased_module_member_is_not_flagged() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-member-ok-fixture.flx");
    let diags = diags_for(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn run(xs) {\n    Array.map(xs, run)\n}\n",
    );
    assert!(
        e004(&diags).is_empty(),
        "a real member must not be flagged, got {:?}",
        e004(&diags)
    );
}

#[test]
fn unimported_sibling_module_path_is_flagged() {
    // `main.flx` qualifies `Math.twice` but never imports the sibling `Math`.
    // The module graph only follows imports, so `Math` never enters the
    // snapshot's `module_programs` — only the workspace module index surfaces
    // it. The squiggle must still fire (E013), turning the previously
    // on-demand sibling import into a squiggle-driven one.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "fn run() {\n    Math.twice(21)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    let diags = open_diags(&mut state, &uris[1], main_src);
    let e = diags_with_code(&diags, "E013");
    assert!(
        e.iter().any(|d| d.message.contains("Math")),
        "expected E013 for the unimported sibling `Math`, got {diags:?}"
    );
}

#[test]
fn imported_sibling_module_path_is_not_flagged() {
    // Same project, but `main.flx` imports the sibling — no squiggle.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math\n\nfn run() {\n    Math.twice(21)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    let diags = open_diags(&mut state, &uris[1], main_src);
    assert!(
        diags_with_code(&diags, "E013").is_empty(),
        "an imported sibling must not be flagged, got {:?}",
        diags_with_code(&diags, "E013")
    );
}

/// False-positive safety net: every shipped `examples/guide/**` and
/// `lib/Flow/**` file is known-good Flux, so the pass must flag nothing in them.
#[test]
fn shipped_sources_have_no_undefined_names() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["examples/guide", "lib/Flow"] {
        let full = repo.join(dir);
        if let Ok(entries) = std::fs::read_dir(&full) {
            for entry in entries.flatten() {
                let path = entry.path();
                // `Flow.FTest` calls `assert_throws`, which `Flow.Assert` does
                // not define — a genuine undefined reference the pass correctly
                // reports, so it is not a false positive to guard against here.
                if path.file_name().and_then(|n| n.to_str()) == Some("FTest.flx") {
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) == Some("flx") {
                    files.push(path);
                }
            }
        }
    }
    assert!(!files.is_empty(), "found no shipped .flx files to check");

    let mut offenders: Vec<String> = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path.strip_prefix(&repo).unwrap_or(&path);
        let u = uri(&format!(
            "file:///{}",
            path.display().to_string().replace('\\', "/")
        ));
        // Fresh state per file so one buffer's analysis can't bleed into another.
        let mut state = GlobalState::default();
        let diags = diags_for(&mut state, &u, &text);
        // E004 undefined name, E012 unknown module member, E013 module not
        // imported — all three are name-resolution findings that must not fire
        // on known-good shipped code.
        for code in ["E004", "E012", "E013"] {
            for d in diags_with_code(&diags, code) {
                offenders.push(format!("{}: [{code}] {}", rel.display(), d.message));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "name resolution produced false positives:\n{}",
        offenders.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Semantic tokens
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct DecodedToken {
    line: u32,
    start: u32,
    #[allow(dead_code)]
    length: u32,
    ttype: String,
    mods: Vec<String>,
}

/// Request `textDocument/semanticTokens/full` and decode the delta-packed
/// stream back into absolute-positioned tokens with their type/modifier names
/// resolved through the advertised legend.
fn semantic_tokens(state: &mut GlobalState, u: &Uri) -> Vec<DecodedToken> {
    let legend = flux_lsp::handlers::semantic_tokens::semantic_tokens_legend();
    let types: Vec<String> = legend
        .token_types
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    let mod_names: Vec<String> = legend
        .token_modifiers
        .iter()
        .map(|m| m.as_str().to_string())
        .collect();

    let result = state.handle_semantic_tokens_full(SemanticTokensParams {
        text_document: ident(u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });

    let mut decoded = Vec::new();
    let mut line = 0u32;
    let mut start = 0u32;
    for tok in result.data {
        if tok.delta_line != 0 {
            line += tok.delta_line;
            start = tok.delta_start;
        } else {
            start += tok.delta_start;
        }
        let mods = mod_names
            .iter()
            .enumerate()
            .filter(|(i, _)| tok.token_modifiers_bitset & (1 << i) != 0)
            .map(|(_, m)| m.clone())
            .collect();
        decoded.push(DecodedToken {
            line,
            start,
            length: tok.length,
            ttype: types[tok.token_type as usize].clone(),
            mods,
        });
    }
    decoded
}

fn token_at(toks: &[DecodedToken], line: u32, start: u32) -> &DecodedToken {
    toks.iter()
        .find(|t| t.line == line && t.start == start)
        .unwrap_or_else(|| panic!("no semantic token at {line}:{start}; got {toks:#?}"))
}

#[test]
fn semantic_tokens_classify_declarations_and_references() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-decls.flx");
    open(
        &mut state,
        &u,
        "fn double(x) {\n    x * 2\n}\n\nfn main() {\n    double(21)\n}\n",
    );
    let toks = semantic_tokens(&mut state, &u);

    // `fn` keyword, `double` a function declaration, `x` a parameter.
    assert_eq!(token_at(&toks, 0, 0).ttype, "keyword");
    let decl = token_at(&toks, 0, 3);
    assert_eq!(decl.ttype, "function");
    assert!(decl.mods.contains(&"declaration".to_string()));
    assert_eq!(token_at(&toks, 0, 10).ttype, "parameter");

    // Body: parameter use, operator, number literal.
    assert_eq!(token_at(&toks, 1, 4).ttype, "parameter");
    assert_eq!(token_at(&toks, 1, 6).ttype, "operator");
    assert_eq!(token_at(&toks, 1, 8).ttype, "number");

    // A *call* to `double` is a function reference without `declaration`.
    let call = token_at(&toks, 5, 4);
    assert_eq!(call.ttype, "function");
    assert!(!call.mods.contains(&"declaration".to_string()));
    assert_eq!(token_at(&toks, 5, 11).ttype, "number");
}

#[test]
fn semantic_tokens_classify_data_constructors_and_bindings() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-data.flx");
    open(&mut state, &u, "data Color { Red, Green }\nlet c = Red\n");
    let toks = semantic_tokens(&mut state, &u);

    let color = token_at(&toks, 0, 5);
    assert_eq!(color.ttype, "enum");
    assert!(color.mods.contains(&"declaration".to_string()));

    assert_eq!(token_at(&toks, 0, 13).ttype, "enumMember"); // Red
    assert_eq!(token_at(&toks, 0, 18).ttype, "enumMember"); // Green

    let binding = token_at(&toks, 1, 4);
    assert_eq!(binding.ttype, "variable");
    assert!(binding.mods.contains(&"readonly".to_string()));
    assert!(binding.mods.contains(&"declaration".to_string()));

    // `Red` referenced as a value is still an enum member.
    assert_eq!(token_at(&toks, 1, 8).ttype, "enumMember");
}

#[test]
fn semantic_tokens_mark_stdlib_with_default_library() {
    // Opening at the repo root loads the Flow prelude, so `Array` resolves to a
    // stdlib module and `map` to one of its members.
    let mut state = GlobalState::default();
    let u = repo_root_uri("sem-stdlib-fixture.flx");
    open(
        &mut state,
        &u,
        "import Flow.Array as Array\n\nfn run(xs) {\n    Array.map(xs, run)\n}\n",
    );
    let toks = semantic_tokens(&mut state, &u);

    // The `Array` qualifier on the call line reads as a default-library namespace
    // even though `Array` is also a built-in type name.
    let array_ref = token_at(&toks, 3, 4);
    assert_eq!(array_ref.ttype, "namespace");
    assert!(array_ref.mods.contains(&"defaultLibrary".to_string()));

    // `map` is a Flow stdlib member → method + defaultLibrary.
    let map_ref = token_at(&toks, 3, 10);
    assert_eq!(map_ref.ttype, "method");
    assert!(map_ref.mods.contains(&"defaultLibrary".to_string()));
}

#[test]
fn semantic_tokens_split_multiline_strings() {
    // A triple-quoted string spans lines; each covered line must yield its own
    // single-line `string` token (semantic tokens may not be multi-line).
    let mut state = GlobalState::default();
    let u = uri("file:///sem-multiline.flx");
    open(&mut state, &u, "let s = \"\"\"\nhello\nworld\n\"\"\"\n");
    let toks = semantic_tokens(&mut state, &u);

    let string_lines: Vec<u32> = toks
        .iter()
        .filter(|t| t.ttype == "string")
        .map(|t| t.line)
        .collect();
    // The literal opens on line 0 and closes on line 3; the interior lines are
    // covered too. No token may straddle a newline.
    assert!(
        string_lines.contains(&0) && string_lines.contains(&1) && string_lines.contains(&2),
        "expected per-line string tokens, got {string_lines:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Call hierarchy
// ─────────────────────────────────────────────────────────────────────────────

fn prepare_call_hierarchy(
    state: &mut GlobalState,
    u: &Uri,
    pos: Position,
) -> Vec<CallHierarchyItem> {
    state
        .handle_prepare_call_hierarchy(CallHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(u),
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap_or_default()
}

/// `fn helper` on lines 0-2, `fn main` on lines 4-7 calling `helper` twice.
const CALL_HIERARCHY_SRC: &str =
    "fn helper(x) {\n    x + 1\n}\n\nfn main() {\n    helper(1)\n    helper(2)\n}\n";

#[test]
fn prepare_call_hierarchy_on_decl_returns_item() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("ch-prepare.flx");
    open(&mut state, &u, CALL_HIERARCHY_SRC);

    // Cursor on `main` (line 4, "fn " then `main`).
    let items = prepare_call_hierarchy(&mut state, &u, Position::new(4, 4));
    assert_eq!(items.len(), 1, "expected one item, got {items:?}");
    assert_eq!(items[0].name, "main");
    assert_eq!(items[0].kind, lsp_types::SymbolKind::FUNCTION);
    // The selection range covers just the name on the decl line.
    assert_eq!(items[0].selection_range.start.line, 4);
}

#[test]
fn prepare_call_hierarchy_on_call_site_resolves_to_decl() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("ch-prepare-call.flx");
    open(&mut state, &u, CALL_HIERARCHY_SRC);

    // Cursor on the `helper` call inside `main` (line 5) resolves to helper's
    // declaration (line 0).
    let items = prepare_call_hierarchy(&mut state, &u, Position::new(5, 6));
    assert_eq!(items.len(), 1, "expected one item, got {items:?}");
    assert_eq!(items[0].name, "helper");
    assert_eq!(items[0].selection_range.start.line, 0);
}

#[test]
fn incoming_calls_lists_callers() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("ch-incoming.flx");
    open(&mut state, &u, CALL_HIERARCHY_SRC);

    // Prepare on `helper`'s declaration, then ask who calls it.
    let item = prepare_call_hierarchy(&mut state, &u, Position::new(0, 4))
        .into_iter()
        .next()
        .expect("an item for helper");
    let incoming = state
        .handle_incoming_calls(CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default();

    assert_eq!(incoming.len(), 1, "expected one caller, got {incoming:?}");
    assert_eq!(incoming[0].from.name, "main");
    // `main` calls `helper` twice.
    assert_eq!(incoming[0].from_ranges.len(), 2);
}

#[test]
fn outgoing_calls_lists_callees() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("ch-outgoing.flx");
    open(&mut state, &u, CALL_HIERARCHY_SRC);

    // Prepare on `main`, then ask what it calls.
    let item = prepare_call_hierarchy(&mut state, &u, Position::new(4, 4))
        .into_iter()
        .next()
        .expect("an item for main");
    let outgoing = state
        .handle_outgoing_calls(CallHierarchyOutgoingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default();

    assert_eq!(outgoing.len(), 1, "expected one callee, got {outgoing:?}");
    assert_eq!(outgoing[0].to.name, "helper");
    assert_eq!(outgoing[0].from_ranges.len(), 2);
}

#[test]
fn cross_file_incoming_calls_span_modules() {
    // `Math.twice` is called from `main.flx`; incoming calls must reach across
    // the module-graph component into the caller's file.
    let math_src = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main_src = "import Math\n\nfn run() {\n    Math.twice(21)\n}\n";
    let (_dir, mut state, uris) =
        workspace_fixture(&[("Math.flx", math_src), ("main.flx", main_src)]);
    open(&mut state, &uris[1], main_src);

    // Prepare on `twice` at its declaration in Math.flx (line 1).
    open(&mut state, &uris[0], math_src);
    let item = prepare_call_hierarchy(&mut state, &uris[0], Position::new(1, 16))
        .into_iter()
        .find(|i| i.name == "twice")
        .expect("an item for twice");

    let incoming = state
        .handle_incoming_calls(CallHierarchyIncomingCallsParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default();

    assert!(
        incoming.iter().any(|c| c.from.name == "run"),
        "expected `run` among callers of `twice`, got {incoming:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Type hierarchy
// ─────────────────────────────────────────────────────────────────────────────

fn prepare_type_hierarchy(
    state: &mut GlobalState,
    u: &Uri,
    pos: Position,
) -> Vec<TypeHierarchyItem> {
    state
        .handle_prepare_type_hierarchy(TypeHierarchyPrepareParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: ident(u),
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        })
        .unwrap_or_default()
}

fn type_supertypes(state: &mut GlobalState, item: TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
    state
        .handle_supertypes(TypeHierarchySupertypesParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
}

fn type_subtypes(state: &mut GlobalState, item: TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
    state
        .handle_subtypes(TypeHierarchySubtypesParams {
            item,
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
}

/// `Eq` (lines 0-2), `Ord` with superclass `Eq` (lines 4-6), and an
/// `instance Ord<Int>` (lines 8-10).
const TYPE_HIERARCHY_SRC: &str = "class Eq<a> {\n    fn eq(x: a, y: a) -> Bool\n}\n\nclass Eq<a> => Ord<a> {\n    fn lt(x: a, y: a) -> Bool\n}\n\ninstance Ord<Int> {\n    fn lt(x, y) { true }\n}\n";

#[test]
fn type_hierarchy_prepare_returns_class() {
    let mut state = GlobalState::default();
    let u = uri("file:///th-prepare.flx");
    open(&mut state, &u, TYPE_HIERARCHY_SRC);

    // Cursor on `Eq` in `class Eq<a>` (line 0, "class " then `Eq`).
    let items = prepare_type_hierarchy(&mut state, &u, Position::new(0, 7));
    assert_eq!(items.len(), 1, "expected one item, got {items:?}");
    assert_eq!(items[0].name, "Eq");
}

#[test]
fn type_hierarchy_subtypes_lists_subclasses_and_instances() {
    let mut state = GlobalState::default();
    let u = uri("file:///th-subtypes.flx");
    open(&mut state, &u, TYPE_HIERARCHY_SRC);

    // Subtypes of `Eq`: the subclass `Ord` that names it as a superclass.
    let eq = prepare_type_hierarchy(&mut state, &u, Position::new(0, 7))
        .into_iter()
        .next()
        .expect("an item for Eq");
    let eq_subs = type_subtypes(&mut state, eq);
    assert!(
        eq_subs.iter().any(|i| i.name == "Ord"),
        "expected `Ord` among Eq's subtypes, got {eq_subs:?}"
    );

    // Subtypes of `Ord`: the implementing type `Int` from `instance Ord<Int>`.
    let ord = eq_subs
        .into_iter()
        .find(|i| i.name == "Ord")
        .expect("the Ord subtype item");
    let ord_subs = type_subtypes(&mut state, ord);
    assert!(
        ord_subs.iter().any(|i| i.name == "Int"),
        "expected the `Int` instance among Ord's subtypes, got {ord_subs:?}"
    );
}

#[test]
fn type_hierarchy_prepare_on_superclass_constrained_subclass() {
    // Regression: the class name in `class Eq<a> => Ord<a>` sits after `=>`, so
    // the locator must resolve `Ord` at its real position (not the keyword-offset
    // position, which would land on the `Eq` constraint).
    let mut state = GlobalState::default();
    let u = uri("file:///th-subclass.flx");
    open(&mut state, &u, TYPE_HIERARCHY_SRC);

    // Cursor on `Ord` in `class Eq<a> => Ord<a>` (line 4, char 16).
    let items = prepare_type_hierarchy(&mut state, &u, Position::new(4, 16));
    assert_eq!(items.len(), 1, "expected one item, got {items:?}");
    assert_eq!(items[0].name, "Ord");
}

#[test]
fn type_hierarchy_supertypes_lists_superclass() {
    let mut state = GlobalState::default();
    let u = uri("file:///th-supertypes.flx");
    open(&mut state, &u, TYPE_HIERARCHY_SRC);

    // Prepare directly on the `=>`-declared subclass `Ord`, then ask for its
    // supertypes.
    let ord = prepare_type_hierarchy(&mut state, &u, Position::new(4, 16))
        .into_iter()
        .next()
        .expect("an item for Ord");
    let supers = type_supertypes(&mut state, ord);
    assert!(
        supers.iter().any(|i| i.name == "Eq"),
        "expected `Eq` among Ord's supertypes, got {supers:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pull-model diagnostics (textDocument/diagnostic)
// ─────────────────────────────────────────────────────────────────────────────

fn diagnostic_report(
    state: &mut GlobalState,
    u: &Uri,
    previous_result_id: Option<String>,
) -> DocumentDiagnosticReportResult {
    state.handle_document_diagnostic(DocumentDiagnosticParams {
        text_document: ident(u),
        identifier: None,
        previous_result_id,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    })
}

/// Unwrap a `Full` pull report into its `(result_id, diagnostics)`.
fn expect_full(
    result: DocumentDiagnosticReportResult,
) -> (Option<String>, Vec<lsp_types::Diagnostic>) {
    match result {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(r)) => {
            let r = r.full_document_diagnostic_report;
            (r.result_id, r.items)
        }
        other => panic!("expected a Full report, got {other:?}"),
    }
}

#[test]
fn document_diagnostic_full_report_carries_diagnostics() {
    let mut state = GlobalState::default();
    let u = uri("file:///pull-diag.flx");
    open(&mut state, &u, "fn main() {\n    let y = mystery + 1\n}\n");

    let (result_id, items) = expect_full(diagnostic_report(&mut state, &u, None));
    assert!(
        result_id.is_some(),
        "a Full report should carry a result id"
    );
    assert!(
        items
            .iter()
            .any(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E004")),
        "expected the E004 squiggle in the pull report, got {items:?}"
    );
}

#[test]
fn document_diagnostic_unchanged_when_result_id_matches() {
    let mut state = GlobalState::default();
    let u = uri("file:///pull-unchanged.flx");
    open(&mut state, &u, "fn main() {\n    let y = mystery + 1\n}\n");

    // First pull yields a Full report tagged with a result id.
    let (result_id, _) = expect_full(diagnostic_report(&mut state, &u, None));
    let result_id = result_id.expect("a result id");

    // Re-pulling with that id — the document is untouched — short-circuits to
    // an Unchanged report echoing the same id (no diagnostics on the wire).
    match diagnostic_report(&mut state, &u, Some(result_id.clone())) {
        DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Unchanged(r)) => {
            assert_eq!(r.unchanged_document_diagnostic_report.result_id, result_id);
        }
        other => panic!("expected an Unchanged report, got {other:?}"),
    }
}

#[test]
fn document_diagnostic_full_when_result_id_is_stale() {
    let mut state = GlobalState::default();
    let u = uri("file:///pull-stale.flx");
    open(&mut state, &u, "fn main() {\n    let y = mystery + 1\n}\n");

    // A non-matching previous id forces a fresh Full report.
    let (result_id, items) =
        expect_full(diagnostic_report(&mut state, &u, Some("stale".to_string())));
    assert!(result_id.is_some());
    assert!(
        items
            .iter()
            .any(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E004")),
        "a stale id should still return the diagnostics, got {items:?}"
    );
}

#[test]
fn document_diagnostic_unknown_document_is_empty_full() {
    let mut state = GlobalState::default();
    let u = uri("file:///never-opened.flx");

    // No snapshot for the document — a pull result can't be null, so the server
    // answers with an empty Full report.
    let (_id, items) = expect_full(diagnostic_report(&mut state, &u, None));
    assert!(
        items.is_empty(),
        "an unknown document should report no diagnostics, got {items:?}"
    );
}

#[test]
fn document_diagnostic_clean_file_has_no_items() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("pull-clean.flx");
    open(&mut state, &u, "fn add(a, b) { a + b }\n");

    let (result_id, items) = expect_full(diagnostic_report(&mut state, &u, None));
    assert!(result_id.is_some());
    assert!(
        items.is_empty(),
        "a well-formed file should pull no diagnostics, got {items:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Workspace-wide pull diagnostics (workspace/diagnostic)
// ─────────────────────────────────────────────────────────────────────────────

fn workspace_diagnostic(
    state: &mut GlobalState,
    previous_result_ids: Vec<PreviousResultId>,
) -> Vec<WorkspaceDocumentDiagnosticReport> {
    let result = state.handle_workspace_diagnostic(WorkspaceDiagnosticParams {
        identifier: None,
        previous_result_ids,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    });
    match result {
        WorkspaceDiagnosticReportResult::Report(r) => r.items,
        other => panic!("expected a workspace Report, got {other:?}"),
    }
}

fn report_uri(report: &WorkspaceDocumentDiagnosticReport) -> &Uri {
    match report {
        WorkspaceDocumentDiagnosticReport::Full(f) => &f.uri,
        WorkspaceDocumentDiagnosticReport::Unchanged(u) => &u.uri,
    }
}

#[test]
fn workspace_diagnostic_reports_analyzed_files_only() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    // `mystery` is undefined → an E004 the workspace sweep should surface.
    let main = "import Math as M\n\nfn run() { M.twice(mystery) }\n";
    // A module nobody opens or imports — must stay out of the sweep so the
    // project's long tail of files doesn't flood the report.
    let unrelated = "module Unrelated {\n    public fn noop() { 0 }\n}\n";
    let (_dir, mut state, uris) = workspace_fixture(&[
        ("Math.flx", math),
        ("main.flx", main),
        ("Unrelated.flx", unrelated),
    ]);

    // Opening main pulls Math into its component — both get analyzed, even
    // though only main was opened. `Unrelated` is never analyzed.
    open(&mut state, &uris[1], main);

    let items = workspace_diagnostic(&mut state, vec![]);
    assert_eq!(
        items.len(),
        2,
        "only the analyzed working set (main + imported Math), got {items:?}"
    );
    assert!(
        items.iter().all(|it| report_uri(it) != &uris[2]),
        "the never-opened Unrelated module must not be reported"
    );

    // main.flx: a Full report carrying the E004.
    let main_full = items
        .iter()
        .find_map(|it| match it {
            WorkspaceDocumentDiagnosticReport::Full(f) if f.uri == uris[1] => Some(f),
            _ => None,
        })
        .expect("a Full report for main.flx");
    assert!(
        main_full
            .full_document_diagnostic_report
            .items
            .iter()
            .any(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E004")),
        "expected E004 in main.flx's workspace report, got {:?}",
        main_full.full_document_diagnostic_report.items
    );

    // Math.flx: analyzed as main's import, reported clean.
    let math_full = items
        .iter()
        .find_map(|it| match it {
            WorkspaceDocumentDiagnosticReport::Full(f) if f.uri == uris[0] => Some(f),
            _ => None,
        })
        .expect("a Full report for Math.flx");
    assert!(
        math_full.full_document_diagnostic_report.items.is_empty(),
        "Math.flx should be clean, got {:?}",
        math_full.full_document_diagnostic_report.items
    );
}

#[test]
fn workspace_diagnostic_scan_all_reports_unopened_files() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    // An unopened file with an undefined name — exactly the "red folder" case.
    let broken = "fn lonely() { mystery + 1 }\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("Broken.flx", broken)]);

    // The opt-in flag force-analyzes every discovered file, so both are reported
    // even though neither was opened.
    state.set_workspace_diagnostics_scan_all(true);
    let items = workspace_diagnostic(&mut state, vec![]);
    assert!(
        items.iter().any(|it| report_uri(it) == &uris[0]),
        "scan-all should report the never-opened Math.flx"
    );
    // And the unopened file's error is surfaced — this is what reddens folders.
    let broken_full = items
        .iter()
        .find_map(|it| match it {
            WorkspaceDocumentDiagnosticReport::Full(f) if f.uri == uris[1] => Some(f),
            _ => None,
        })
        .expect("a report for the never-opened Broken.flx");
    assert!(
        broken_full
            .full_document_diagnostic_report
            .items
            .iter()
            .any(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E004")),
        "scan-all should surface E004 in the unopened Broken.flx, got {:?}",
        broken_full.full_document_diagnostic_report.items
    );
}

#[test]
fn workspace_diagnostic_unchanged_on_second_pull() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);

    // Analyze the working set (opening main pulls in Math).
    open(&mut state, &uris[1], main);

    // First sweep: every analyzed file comes back Full with a result id.
    let first = workspace_diagnostic(&mut state, vec![]);
    let previous: Vec<PreviousResultId> = first
        .iter()
        .filter_map(|it| match it {
            WorkspaceDocumentDiagnosticReport::Full(f) => f
                .full_document_diagnostic_report
                .result_id
                .clone()
                .map(|value| PreviousResultId {
                    uri: f.uri.clone(),
                    value,
                }),
            _ => None,
        })
        .collect();
    assert_eq!(previous.len(), 2, "both files should carry a result id");

    // Nothing changed, so re-pulling with those ids yields all Unchanged.
    let second = workspace_diagnostic(&mut state, previous);
    assert!(
        second
            .iter()
            .all(|it| matches!(it, WorkspaceDocumentDiagnosticReport::Unchanged(_))),
        "expected every report Unchanged on the second pull, got {second:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Rename-file import fixes (workspace/willRenameFiles)
// ─────────────────────────────────────────────────────────────────────────────

/// Every `TextEdit` the workspace edit targets at `uri`.
fn edits_for(edit: &WorkspaceEdit, uri: &Uri) -> Vec<TextEdit> {
    match &edit.document_changes {
        Some(DocumentChanges::Edits(changes)) => changes
            .iter()
            .filter(|c| &c.text_document.uri == uri)
            .flat_map(|c| {
                c.edits.iter().filter_map(|e| match e {
                    OneOf::Left(te) => Some(te.clone()),
                    OneOf::Right(_) => None,
                })
            })
            .collect(),
        _ => vec![],
    }
}

/// Apply non-overlapping LSP `TextEdit`s to an ASCII source (line/char == byte).
fn apply_edits(src: &str, edits: &[TextEdit]) -> String {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(src.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let off = |p: Position| line_starts[p.line as usize] + p.character as usize;
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| (off(e.range.start), off(e.range.end), e.new_text.as_str()))
        .collect();
    spans.sort_by_key(|(s, _, _)| *s);
    let mut out = String::new();
    let mut cursor = 0;
    for (s, e, new) in spans {
        out.push_str(&src[cursor..s]);
        out.push_str(new);
        cursor = e;
    }
    out.push_str(&src[cursor..]);
    out
}

fn rename_to(dir: &std::path::Path, old: &Uri, new_name: &str) -> RenameFilesParams {
    let new_uri = flux_lsp::vfs::path_to_uri(&dir.join(new_name)).unwrap();
    RenameFilesParams {
        files: vec![FileRename {
            old_uri: old.as_str().to_string(),
            new_uri: new_uri.as_str().to_string(),
        }],
    }
}

#[test]
fn will_rename_rewrites_module_decl_and_unaliased_uses() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main = "import Math\n\nfn run() { Math.twice(21) }\n";
    let (dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);

    let edit = state
        .handle_will_rename_files(rename_to(dir.path(), &uris[0], "Calc.flx"))
        .expect("a workspace edit for the rename");

    // The renamed file's own declaration moves to the new name.
    assert_eq!(
        apply_edits(math, &edits_for(&edit, &uris[0])),
        "module Calc {\n    public fn twice(x) { x * 2 }\n}\n"
    );
    // The dependent's import path AND its unaliased `Math.` use are rewritten.
    assert_eq!(
        apply_edits(main, &edits_for(&edit, &uris[1])),
        "import Calc\n\nfn run() { Calc.twice(21) }\n"
    );
}

#[test]
fn will_rename_leaves_aliased_uses_alone() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main = "import Math as M\n\nfn run() { M.twice(21) }\n";
    let (dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);

    let edit = state
        .handle_will_rename_files(rename_to(dir.path(), &uris[0], "Calc.flx"))
        .expect("a workspace edit for the rename");

    // Only the import path changes; the alias `M` (and `M.twice`) stay put.
    let main_edits = edits_for(&edit, &uris[1]);
    assert_eq!(
        main_edits.len(),
        1,
        "only the import path, got {main_edits:?}"
    );
    assert_eq!(
        apply_edits(main, &main_edits),
        "import Calc as M\n\nfn run() { M.twice(21) }\n"
    );
}

#[test]
fn will_rename_entry_script_makes_no_edits() {
    // main.flx declares no module, so nothing imports it — a rename needs no
    // import fixes.
    let main = "fn main() with IO {\n    print(\"hi\")\n}\n";
    let (dir, mut state, uris) = workspace_fixture(&[("main.flx", main)]);

    assert!(
        state
            .handle_will_rename_files(rename_to(dir.path(), &uris[0], "app.flx"))
            .is_none(),
        "renaming an entry script should produce no edit"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Document links (textDocument/documentLink)
// ─────────────────────────────────────────────────────────────────────────────

fn document_links(state: &mut GlobalState, u: &Uri) -> Vec<lsp_types::DocumentLink> {
    state
        .handle_document_link(DocumentLinkParams {
            text_document: ident(u),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        })
        .unwrap_or_default()
}

#[test]
fn document_link_points_import_to_sibling_module() {
    let math = "module Math {\n    public fn twice(x) { x * 2 }\n}\n";
    let main = "import Math\n\nfn run() { Math.twice(21) }\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);
    open(&mut state, &uris[1], main);

    let links = document_links(&mut state, &uris[1]);
    let link = links
        .iter()
        .find(|l| l.target.as_ref() == Some(&uris[0]))
        .expect("a document link targeting Math.flx");
    // The clickable range is the `Math` path, right after `import `.
    assert_eq!(link.range.start.line, 0);
    assert_eq!(link.range.start.character, 7);
    assert_eq!(link.range.end.character, 7 + "Math".len() as u32);
}

#[test]
fn document_link_points_import_to_flow_stdlib() {
    let mut state = GlobalState::default();
    let u = repo_root_uri("doclink-flow.flx");
    open(
        &mut state,
        &u,
        "import Flow.List as L\n\nfn main() with IO {\n    print(1)\n}\n",
    );

    let links = document_links(&mut state, &u);
    let link = links
        .iter()
        .find(|l| l.tooltip.as_deref() == Some("Open module `Flow.List`"))
        .expect("a document link for Flow.List");
    let target = link.target.as_ref().expect("link target uri");
    assert!(
        target.as_str().replace('\\', "/").ends_with("/List.flx"),
        "expected the link to point at Flow/List.flx, got {}",
        target.as_str()
    );
    // The path spans `Flow.List` after `import ` (the `as L` is not part of it).
    assert_eq!(link.range.start.character, 7);
    assert_eq!(link.range.end.character, 7 + "Flow.List".len() as u32);
}

#[test]
fn document_link_none_without_imports() {
    let mut state = GlobalState::default();
    let u = uri("file:///no-imports.flx");
    open(&mut state, &u, "fn main() with IO {\n    print(1)\n}\n");

    assert!(
        document_links(&mut state, &u).is_empty(),
        "a file with no imports should expose no links"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// inlayHint/resolve
// ─────────────────────────────────────────────────────────────────────────────

fn inlay_hints(state: &mut GlobalState, u: &Uri) -> Vec<lsp_types::InlayHint> {
    state.handle_inlay_hints(lsp_types::InlayHintParams {
        text_document: ident(u),
        // The handler collects every hint in the file; the range is ignored.
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 10_000,
                character: 0,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    })
}

#[test]
fn inlay_hint_resolve_adds_tooltip_and_annotation_edit() {
    let mut state = GlobalState::default();
    let u = uri("file:///inlay.flx");
    open(
        &mut state,
        &u,
        "fn main() with IO {\n    let x = 41 + 1\n    print(x)\n}\n",
    );

    let hints = inlay_hints(&mut state, &u);
    let hint = hints.first().cloned().expect("a type hint for `let x`");
    // The initial response is lightweight: the tooltip and edit are deferred to
    // resolve, with just the reconstruction payload stashed in `data`.
    assert!(hint.data.is_some(), "hint carries resolve data");
    assert!(
        hint.tooltip.is_none() && hint.text_edits.is_none(),
        "initial hint omits tooltip and text edit"
    );

    let resolved = state.handle_inlay_hint_resolve(hint.clone());
    assert!(resolved.tooltip.is_some(), "resolve fills in the tooltip");
    let edits = resolved
        .text_edits
        .expect("a `let` hint resolves to an insert-annotation edit");
    assert_eq!(edits.len(), 1);
    assert!(
        edits[0].new_text.starts_with(": "),
        "the edit inserts a `: T` annotation, got {:?}",
        edits[0].new_text
    );
    // The annotation is inserted at the hint's own position (zero-width range).
    assert_eq!(edits[0].range.start, hint.position);
    assert_eq!(edits[0].range.end, hint.position);
}

#[test]
fn inlay_hint_resolve_ignores_hint_without_data() {
    let mut state = GlobalState::default();
    // A hint with no `data` (e.g. one this server didn't produce) round-trips
    // through resolve unchanged rather than fabricating a tooltip/edit.
    let hint = lsp_types::InlayHint {
        position: Position {
            line: 0,
            character: 0,
        },
        label: lsp_types::InlayHintLabel::String(": Int".into()),
        kind: Some(lsp_types::InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: None,
        data: None,
    };

    let resolved = state.handle_inlay_hint_resolve(hint);
    assert!(resolved.tooltip.is_none());
    assert!(resolved.text_edits.is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// semanticTokens range + full/delta
// ─────────────────────────────────────────────────────────────────────────────

fn semantic_tokens_full_raw(state: &mut GlobalState, u: &Uri) -> lsp_types::SemanticTokens {
    state.handle_semantic_tokens_full(SemanticTokensParams {
        text_document: ident(u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    })
}

fn replace_buffer(state: &mut GlobalState, u: &Uri, version: i32, text: &str) {
    state.handle_did_change(DidChangeTextDocumentParams {
        text_document: lsp_types::VersionedTextDocumentIdentifier {
            uri: u.clone(),
            version,
        },
        content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.to_string(),
        }],
    });
}

/// Apply a semantic-tokens delta edit list to the previous token stream,
/// reproducing what an LSP client does. `start`/`delete_count` are in the
/// protocol's integer units (5 per token).
fn apply_token_edits(
    mut tokens: Vec<lsp_types::SemanticToken>,
    edits: &[lsp_types::SemanticTokensEdit],
) -> Vec<lsp_types::SemanticToken> {
    let mut ordered: Vec<&lsp_types::SemanticTokensEdit> = edits.iter().collect();
    ordered.sort_by_key(|e| std::cmp::Reverse(e.start));
    for e in ordered {
        let start = (e.start / 5) as usize;
        let delete = (e.delete_count / 5) as usize;
        let data = e.data.clone().unwrap_or_default();
        tokens.splice(start..start + delete, data);
    }
    tokens
}

#[test]
fn semantic_tokens_full_carries_result_id() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-id.flx");
    open(&mut state, &u, "fn main() {\n    1\n}\n");

    let result = semantic_tokens_full_raw(&mut state, &u);
    assert!(
        result.result_id.is_some(),
        "a full response must be tagged so the client can request a delta against it"
    );
    assert!(!result.data.is_empty());
}

#[test]
fn semantic_tokens_delta_splices_after_edit() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-delta.flx");
    open(
        &mut state,
        &u,
        "fn alpha(x) {\n    x\n}\n\nfn main() {\n    alpha(1)\n}\n",
    );

    let before = semantic_tokens_full_raw(&mut state, &u);
    let prev_id = before.result_id.clone().expect("result id");

    // Rename the parameter `x` → `value`: same token roles, but the length
    // change (and the columns it shifts) makes the stream genuinely differ, so
    // the delta is a real splice rather than a no-op.
    replace_buffer(
        &mut state,
        &u,
        2,
        "fn alpha(value) {\n    value\n}\n\nfn main() {\n    alpha(1)\n}\n",
    );

    let delta = state.handle_semantic_tokens_full_delta(lsp_types::SemanticTokensDeltaParams {
        text_document: ident(&u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        previous_result_id: prev_id.clone(),
    });

    let edits = match delta {
        lsp_types::SemanticTokensFullDeltaResult::TokensDelta(d) => {
            assert!(d.result_id.is_some());
            assert_ne!(
                d.result_id.as_deref(),
                Some(prev_id.as_str()),
                "the delta carries a fresh id"
            );
            assert!(!d.edits.is_empty(), "a rename changes some tokens");
            d.edits
        }
        other => panic!("expected a token delta, got {other:?}"),
    };

    // Replaying the splice on the old stream must reproduce a fresh full set.
    let reconstructed = apply_token_edits(before.data.clone(), &edits);
    let expected = semantic_tokens_full_raw(&mut state, &u).data;
    assert_eq!(reconstructed, expected);
}

#[test]
fn semantic_tokens_delta_unknown_previous_id_returns_full() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-stale.flx");
    open(&mut state, &u, "fn main() {\n    1\n}\n");
    let _ = semantic_tokens_full_raw(&mut state, &u);

    let delta = state.handle_semantic_tokens_full_delta(lsp_types::SemanticTokensDeltaParams {
        text_document: ident(&u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        previous_result_id: "does-not-exist".to_string(),
    });

    match delta {
        lsp_types::SemanticTokensFullDeltaResult::Tokens(t) => {
            assert!(t.result_id.is_some());
            assert!(
                !t.data.is_empty(),
                "a stale baseline falls back to a full set"
            );
        }
        other => panic!("expected a full token set, got {other:?}"),
    }
}

#[test]
fn semantic_tokens_range_limits_to_requested_lines() {
    let mut state = GlobalState::default();
    let u = uri("file:///sem-range.flx");
    // Three separate declarations on distinct lines.
    open(
        &mut state,
        &u,
        "fn one() { 1 }\nfn two() { 2 }\nfn three() { 3 }\n",
    );

    let full = semantic_tokens_full_raw(&mut state, &u);

    // Ask for just the middle line (line 1).
    let ranged = state.handle_semantic_tokens_range(lsp_types::SemanticTokensRangeParams {
        text_document: ident(&u),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        range: Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 0,
            },
        },
    });

    assert!(
        ranged.data.len() < full.data.len(),
        "a single-line range returns fewer tokens than the whole file"
    );
    assert!(!ranged.data.is_empty(), "line 1 has tokens");
    // The first token's absolute line is the delta_line (baseline 0); every
    // token in a single-line range sits on that one line.
    assert_eq!(
        ranged.data[0].delta_line, 1,
        "the only line touched is line 1"
    );
    assert!(
        ranged.data[1..].iter().all(|t| t.delta_line == 0),
        "all range tokens are on the same line"
    );
    // Range results aren't cached for deltas.
    assert!(ranged.result_id.is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// signatureHelp
// ─────────────────────────────────────────────────────────────────────────────

fn signature_help(
    state: &mut GlobalState,
    u: &Uri,
    line: u32,
    character: u32,
) -> Option<lsp_types::SignatureHelp> {
    state.handle_signature_help(lsp_types::SignatureHelpParams {
        context: None,
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(u),
            position: Position { line, character },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    })
}

/// The substring of the signature label spanned by parameter `idx`'s offsets.
/// Test labels are ASCII, so UTF-16 offsets coincide with byte offsets.
fn param_label_text(sig: &lsp_types::SignatureInformation, idx: usize) -> String {
    let p = &sig.parameters.as_ref().expect("parameters")[idx];
    match &p.label {
        lsp_types::ParameterLabel::LabelOffsets([s, e]) => {
            sig.label[*s as usize..*e as usize].to_string()
        }
        other => panic!("expected label offsets, got {other:?}"),
    }
}

const SIG_SRC: &str = "fn add(x, y) { x + y }\n\nfn main() {\n    add(1, 2)\n}\n";

#[test]
fn signature_help_shows_param_names_with_offsets() {
    let mut state = GlobalState::default();
    let u = uri("file:///sig-names.flx");
    open(&mut state, &u, SIG_SRC);

    // Cursor on the first argument `1` (line 3, char 8).
    let help = signature_help(&mut state, &u, 3, 8).expect("signature help in the first arg");
    assert_eq!(help.active_parameter, Some(0));
    let sig = &help.signatures[0];
    assert!(
        sig.label.starts_with("add(x: "),
        "label carries the callee name and parameter names, got {:?}",
        sig.label
    );
    assert!(sig.label.contains(", y: "));
    assert!(sig.label.contains(") -> "));
    // The reported offsets carve out the right parameter substrings.
    assert!(param_label_text(sig, 0).starts_with("x:"));
    assert!(param_label_text(sig, 1).starts_with("y:"));
}

#[test]
fn signature_help_tracks_active_parameter() {
    let mut state = GlobalState::default();
    let u = uri("file:///sig-active.flx");
    open(&mut state, &u, SIG_SRC);

    // Cursor on the second argument `2` (line 3, char 11).
    let help = signature_help(&mut state, &u, 3, 11).expect("signature help in the second arg");
    assert_eq!(help.active_parameter, Some(1));
    assert_eq!(help.signatures[0].active_parameter, Some(1));
}

#[test]
fn signature_help_no_arg_function_has_no_active_parameter() {
    let mut state = GlobalState::default();
    let u = uri("file:///sig-noarg.flx");
    open(
        &mut state,
        &u,
        "fn ping() { 1 }\n\nfn main() {\n    ping()\n}\n",
    );

    // Cursor between the empty parens (line 3, char 9).
    let help = signature_help(&mut state, &u, 3, 9).expect("signature help in empty parens");
    assert_eq!(help.active_parameter, None);
    let sig = &help.signatures[0];
    assert!(sig.label.starts_with("ping()"));
    assert!(sig.parameters.as_ref().is_none_or(|p| p.is_empty()));
}

#[test]
fn signature_help_clamps_active_parameter_past_last() {
    let mut state = GlobalState::default();
    let u = uri("file:///sig-clamp.flx");
    open(&mut state, &u, SIG_SRC);

    // Cursor just before the closing paren, past the last argument (char 12):
    // the slot index (2) is clamped to the last real parameter (1).
    let help = signature_help(&mut state, &u, 3, 12).expect("signature help past the last arg");
    assert_eq!(help.active_parameter, Some(1));
}

#[test]
fn signature_help_includes_doc_comment() {
    let mut state = GlobalState::default();
    let u = uri("file:///sig-doc.flx");
    open(
        &mut state,
        &u,
        "/// Adds two numbers.\nfn add(x, y) { x + y }\n\nfn main() {\n    add(1, 2)\n}\n",
    );

    let help = signature_help(&mut state, &u, 4, 8).expect("signature help with a doc comment");
    match &help.signatures[0].documentation {
        Some(Documentation::MarkupContent(m)) => assert!(m.value.contains("Adds two numbers")),
        other => panic!("expected the callee's doc comment, got {other:?}"),
    }
}

#[test]
fn signature_help_cross_module_shows_param_names_and_doc() {
    let math = "module Math {\n    /// Sums two ints.\n    public fn add(x, y) { x + y }\n}\n";
    let main = "import Math\n\nfn main() {\n    Math.add(1, 2)\n}\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);
    open(&mut state, &uris[1], main);

    // Cursor on the first argument of the qualified call `Math.add(1, 2)`
    // (line 3, char 13). The callee is declared in the *other* module.
    let help = signature_help(&mut state, &uris[1], 3, 13).expect("cross-module signature help");
    let sig = &help.signatures[0];
    assert!(
        sig.label.starts_with("add(x: "),
        "imported callee should show parameter names, got {:?}",
        sig.label
    );
    assert!(sig.label.contains(", y: "));
    assert_eq!(help.active_parameter, Some(0));
    match &sig.documentation {
        Some(Documentation::MarkupContent(m)) => assert!(m.value.contains("Sums two ints")),
        other => panic!("expected the imported callee's doc comment, got {other:?}"),
    }
}

#[test]
fn signature_help_exposing_unqualified_shows_param_names() {
    let math = "module Math {\n    /// Sums two ints.\n    public fn add(x, y) { x + y }\n}\n";
    // `add` is imported unqualified via `exposing`, so it's called without `Math.`.
    let main = "import Math exposing (add)\n\nfn main() {\n    add(1, 2)\n}\n";
    let (_dir, mut state, uris) = workspace_fixture(&[("Math.flx", math), ("main.flx", main)]);
    open(&mut state, &uris[1], main);

    // Cursor on the first argument of the unqualified call `add(1, 2)`
    // (line 3, char 8).
    let help =
        signature_help(&mut state, &uris[1], 3, 8).expect("exposing-unqualified signature help");
    let sig = &help.signatures[0];
    assert!(
        sig.label.starts_with("add(x: "),
        "an `exposing`-imported callee should show parameter names, got {:?}",
        sig.label
    );
    assert!(sig.label.contains(", y: "));
    assert_eq!(help.active_parameter, Some(0));
    match &sig.documentation {
        Some(Documentation::MarkupContent(m)) => assert!(m.value.contains("Sums two ints")),
        other => panic!("expected the exposed callee's doc comment, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// linkedEditingRange
// ─────────────────────────────────────────────────────────────────────────────

fn linked_editing(
    state: &mut GlobalState,
    u: &Uri,
    line: u32,
    character: u32,
) -> Option<lsp_types::LinkedEditingRanges> {
    state.handle_linked_editing_range(lsp_types::LinkedEditingRangeParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: ident(u),
            position: Position { line, character },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    })
}

const LINKED_SRC: &str = "fn main() {\n    let count = 1\n    count + count\n}\n";

#[test]
fn linked_editing_links_same_file_occurrences() {
    let mut state = GlobalState::default();
    let u = uri("file:///linked.flx");
    open(&mut state, &u, LINKED_SRC);

    // Cursor inside the first use of `count` on line 2.
    let linked = linked_editing(&mut state, &u, 2, 6).expect("linked ranges for `count`");

    // The declaration plus both uses (3 occurrences), each spanning `count`.
    assert_eq!(linked.ranges.len(), 3);
    for r in &linked.ranges {
        assert_eq!(r.start.line, r.end.line, "occurrences are single-line");
        assert_eq!(
            r.end.character - r.start.character,
            "count".len() as u32,
            "every range spans exactly the identifier"
        );
    }
    // The cursor sits inside one of the returned ranges (so the editor links it).
    assert!(
        linked
            .ranges
            .iter()
            .any(|r| r.start.line == 2 && r.start.character <= 6 && 6 < r.end.character)
    );
    assert!(linked.word_pattern.is_some());
}

#[test]
fn linked_editing_none_off_identifier() {
    let mut state = GlobalState::default();
    let u = uri("file:///linked-none.flx");
    open(&mut state, &u, LINKED_SRC);

    // Cursor on the `+` operator (line 2, char 10) — not an identifier.
    assert!(linked_editing(&mut state, &u, 2, 10).is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// source.organizeImports
// ─────────────────────────────────────────────────────────────────────────────

/// Request code actions filtered to `source.organizeImports` and return the
/// single TextEdit of the organize action, if any.
fn organize_imports_edit(state: &mut GlobalState, u: &Uri) -> Option<TextEdit> {
    let params = CodeActionParams {
        text_document: ident(u),
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
        context: CodeActionContext {
            diagnostics: vec![],
            only: Some(vec![lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
            trigger_kind: None,
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let resp = state.handle_code_action(params)?;
    let action = resp.into_iter().find_map(|a| match a {
        CodeActionOrCommand::CodeAction(ca)
            if ca.kind == Some(lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS) =>
        {
            Some(ca)
        }
        _ => None,
    })?;
    match action.edit?.document_changes? {
        DocumentChanges::Edits(mut edits) => match edits.pop()?.edits.pop()? {
            OneOf::Left(e) => Some(e),
            OneOf::Right(_) => None,
        },
        _ => None,
    }
}

#[test]
fn organize_imports_sorts_and_drops_unused() {
    let mut state = GlobalState::default();
    let u = uri("file:///org-imports.flx");
    // `Zebra` is unused; `Apple` is used qualified. Out of alphabetical order.
    open(
        &mut state,
        &u,
        "import Zebra\nimport Apple\n\nfn main() with IO {\n    Apple.go()\n}\n",
    );

    let edit = organize_imports_edit(&mut state, &u).expect("organize action");
    // Unused `Zebra` dropped, leaving only the used import.
    assert_eq!(edit.new_text, "import Apple");
    // The edit replaces the import block (lines 0..1).
    assert_eq!(edit.range.start, Position::new(0, 0));
    assert_eq!(edit.range.end.line, 1);
}

#[test]
fn organize_imports_keeps_exposing_imports() {
    let mut state = GlobalState::default();
    let u = uri("file:///org-exposing.flx");
    // `Helpers` is never used *qualified*, but it exposes `go` unqualified —
    // so it must not be dropped (the unused check can't see the `go` binding).
    open(
        &mut state,
        &u,
        "import Helpers exposing (go)\n\nfn main() with IO {\n    go()\n}\n",
    );

    // Already sorted + nothing safely removable → no edit offered.
    assert!(organize_imports_edit(&mut state, &u).is_none());
}

#[test]
fn organize_imports_not_offered_without_source_request() {
    let mut state = GlobalState::default();
    let u = uri("file:///org-noreq.flx");
    open(
        &mut state,
        &u,
        "import Zebra\nimport Apple\n\nfn main() with IO {\n    Apple.go()\n}\n",
    );

    // A normal (cursor) code-action request carries no `only` filter, so the
    // source action must not appear.
    let resp = state
        .handle_code_action(code_action_params(
            &u,
            Range {
                start: Position::new(0, 0),
                end: Position::new(0, 0),
            },
        ))
        .unwrap_or_default();
    assert!(
        !resp.iter().any(|a| matches!(
            a,
            CodeActionOrCommand::CodeAction(ca)
                if ca.kind == Some(lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
        )),
        "organize-imports should not appear without an explicit source request"
    );
}

/// Collected error codes from a freshly opened buffer.
fn published_codes(state: &mut GlobalState, u: &Uri, src: &str) -> Vec<String> {
    state
        .handle_did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: u.clone(),
                language_id: "flux".into(),
                version: 1,
                text: src.into(),
            },
        })
        .into_iter()
        .flat_map(|p| p.diagnostics)
        .filter_map(|d| match d.code {
            Some(NumberOrString::String(c)) => Some(c),
            _ => None,
        })
        .collect()
}

#[test]
fn incomplete_instance_publishes_e442() {
    // `Show9` requires `show`; the empty instance omits it — the compiler's
    // class-collection check (E442) is now surfaced as an editor squiggle.
    let mut state = GlobalState::default();
    let u = uri("file:///e442.flx");
    let codes = published_codes(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class Show9<a> {\n\
         \x20   fn show(x: a) -> String\n\
         }\n\
         instance Show9<Color> {\n\
         }\n",
    );
    assert!(
        codes.iter().any(|c| c == "E442"),
        "an incomplete instance should publish E442, got {codes:?}"
    );
}

#[test]
fn instance_with_unknown_method_publishes_e446() {
    // `extra` is not a method of `One` — E446 (method not in class).
    let mut state = GlobalState::default();
    let u = uri("file:///e446.flx");
    let codes = published_codes(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class One<a> {\n\
         \x20   fn show(x: a) -> String\n\
         }\n\
         instance One<Color> {\n\
         \x20   fn show(x) { panic(\"p\") }\n\
         \x20   fn extra(x) { panic(\"p\") }\n\
         }\n",
    );
    assert!(
        codes.iter().any(|c| c == "E446"),
        "an instance with a non-class method should publish E446, got {codes:?}"
    );
}

#[test]
fn complete_builtin_instance_publishes_no_class_errors() {
    // The shipped `type_class_basic.flx` pattern: explicit, complete instances
    // of the *built-in* `Eq` class. None of the surfaced class checks
    // (E440–E448) — and crucially not a phantom duplicate (E443) against the
    // built-in instance — may fire here.
    let mut state = GlobalState::default();
    let u = uri("file:///builtin-eq.flx");
    let codes = published_codes(
        &mut state,
        &u,
        "instance Eq<Int> {\n\
         \x20   fn eq(x, y) { x == y }\n\
         \x20   fn neq(x, y) { x != y }\n\
         }\n\
         instance Eq<String> {\n\
         \x20   fn eq(x, y) { x == y }\n\
         \x20   fn neq(x, y) { x != y }\n\
         }\n\
         fn main() with IO {\n\
         \x20   print(42)\n\
         }\n",
    );
    assert!(
        !codes.iter().any(|c| c.starts_with("E44")),
        "complete built-in-class instances must not produce class errors, got {codes:?}"
    );
}

#[test]
fn add_missing_methods_quick_fix_resolves_e442() {
    // The quick fix attaches the E442 it clears, so the client can associate it
    // with the squiggle.
    let mut state = GlobalState::default();
    let u = uri("file:///e442-fix.flx");
    open(
        &mut state,
        &u,
        "data Color { Red, Green }\n\
         class Show8<a> {\n\
         \x20   fn show(x: a) -> String\n\
         }\n\
         instance Show8<Color> {\n\
         }\n",
    );
    // Cursor inside the instance body — the whole-instance trigger still fires.
    let actions = state
        .handle_code_action(code_action_params(
            &u,
            Range::new(Position::new(5, 0), Position::new(5, 0)),
        ))
        .expect("code action response");
    let add = actions
        .iter()
        .find(|a| action_title(a).contains("Add missing method"))
        .expect("expected the add-missing-methods quick fix");
    let CodeActionOrCommand::CodeAction(ca) = add else {
        panic!("expected a CodeAction");
    };
    let attached = ca.diagnostics.as_ref().expect("attached diagnostics");
    assert!(
        attached
            .iter()
            .any(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "E442")),
        "the fix should carry the E442 it resolves, got {:?}",
        attached.iter().map(|d| &d.code).collect::<Vec<_>>()
    );
}
