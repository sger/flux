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
    ClientCapabilities, CompletionParams, CompletionResponse, DidOpenTextDocumentParams,
    DocumentFormattingParams, DocumentSymbolParams, FormattingOptions, GotoDefinitionParams,
    HoverParams, InitializeParams, InitializedParams, PartialResultParams, Position,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
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

// ─────────────────────────────────────────────────────────────────────────────
// End-to-end (JSON-RPC) — keeps the dispatcher honest
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn initialize_handshake_advertises_capabilities() {
    let (server_conn, client) = Connection::memory();
    let server_thread = thread::spawn(move || {
        let caps = serde_json::to_value(server_capabilities(PositionEncoding::Utf16)).unwrap();
        let _params = server_conn.initialize(caps).expect("server initialize");
        let server = Server::new(server_conn, PositionEncoding::Utf16);
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
    assert_eq!(caps["definitionProvider"], Value::Bool(true));
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
fn hover_on_effect_name_returns_effect_label() {
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
        value.contains("effect: IO"),
        "expected effect label, got: {value}"
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
    match resp {
        lsp_types::GotoDefinitionResponse::Scalar(loc) => {
            assert_eq!(loc.uri, u);
            assert_eq!(loc.range.start.line, 0, "definition should land on line 0");
        }
        other => panic!("expected scalar location, got {other:?}"),
    }
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
        format!(
            "file:///{}%3A{}",
            &raw[..1],
            &raw[2..]
        )
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
    open(&mut state, &u, "data Person { Person { name: String, age: Int } }\n");
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
    open(&mut state, &u, "import Flow.Async exposing (..)\nfn body() with Async { 1 }\n");
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
    // Table-driven smoke test for the 10 keywords added in M5a. Each case
    // gives a source snippet, the keyword to hover, and a phrase the
    // keyword's docs MUST contain (so we don't accidentally return the
    // wrong entry's docs).
    let cases: &[(&str, &str, &str)] = &[
        ("deriving", "data X { A, B } deriving (Eq)\n", "Auto-generate"),
        ("type", "type T = Int\n", "transparent type alias"),
        ("where", "let x = a where a = 1\n", "let-binding"),
        (
            "select",
            "fn f() with Async { select { x -> 1 } }\n",
            "first ready",
        ),
        ("sealing", "fn f() { x sealing { Console } }\n", "Restrict"),
        ("primop", "intrinsic fn p() = primop X\n", "compiler primitive"),
        ("Some", "let x = Some(1)\n", "Option"),
        ("None", "let x = None\n", "absent value"),
        ("Left", "let x = Left(1)\n", "Either"),
        ("Right", "let x = Right(2)\n", "Either"),
    ];

    for (kw, source, expected_substring) in cases {
        let mut state = GlobalState::default();
        let u = uri(&format!("file:///kw_{kw}.flx"));
        open(&mut state, &u, source);
        let kw_byte_off = source.find(kw).expect("keyword in source");
        // Convert byte offset to LSP character (UTF-16 by default but our
        // sources are ASCII, so byte == char).
        let value = hover_markup(&mut state, &u, 0, (kw_byte_off + 1) as u32)
            .unwrap_or_else(|| panic!("expected hover on `{kw}`"));
        assert!(
            value.contains(expected_substring),
            "for keyword `{kw}`, expected substring `{expected_substring}` in: {value}"
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
    assert!(labels.iter().any(|l| l == "IO"), "expected IO in {labels:?}");
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
