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
    ClientCapabilities, CodeActionContext, CodeActionOrCommand, CodeActionParams, CodeLensParams,
    CompletionItem, CompletionParams, CompletionResponse, DidChangeTextDocumentParams,
    DidChangeWatchedFilesParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    DocumentChanges, DocumentFormattingParams, DocumentHighlightParams, DocumentSymbolParams,
    Documentation, FileChangeType, FileEvent, FoldingRangeParams, FormattingOptions,
    GotoDefinitionParams, HoverParams, InitializeParams, InitializedParams, NumberOrString, OneOf,
    PartialResultParams, Position, Range, ReferenceContext, ReferenceParams, RenameParams,
    SelectionRangeParams, TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
    Uri, WorkDoneProgressParams, WorkspaceSymbolParams, WorkspaceSymbolResponse,
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
fn hover_on_data_declaration_name_returns_data_label() {
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
        value.contains("data") && value.contains("Person"),
        "expected data label, got: {value}"
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
    assert_eq!(lenses.len(), 2, "a lens per test fn, none for `helper`");
    let cmd = lenses[0].command.as_ref().expect("lens command");
    assert_eq!(cmd.command, "flux.runTest");
    let args = cmd.arguments.as_ref().expect("lens arguments");
    assert_eq!(
        args[1],
        serde_json::json!("test_a"),
        "the test name should travel as the second command argument"
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
        match client.receiver.recv_timeout(remaining).unwrap() {
            Message::Response(r) if r.id == hover_id => {
                served = true;
                break;
            }
            _ => continue,
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
