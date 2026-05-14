use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, Initialize,
    Shutdown,
};
use lsp_types::{
    ClientCapabilities, CompletionParams, DidOpenTextDocumentParams, DocumentFormattingParams,
    DocumentSymbolParams, FormattingOptions, GotoDefinitionParams, HoverParams, InitializeParams,
    InitializedParams, PartialResultParams, Position, PublishDiagnosticsParams,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
};
use serde_json::Value;

use flux_lsp::{Server, server_capabilities};

fn spawn_server() -> (Connection, thread::JoinHandle<()>) {
    let (server_conn, client_conn) = Connection::memory();
    let handle = thread::spawn(move || {
        let caps = serde_json::to_value(server_capabilities()).unwrap();
        let _params = server_conn.initialize(caps).expect("server initialize");
        let server = Server::new(server_conn);
        server.run().expect("server run");
    });
    (client_conn, handle)
}

fn drive_initialize(client: &Connection) {
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

    // Wait for the initialize response.
    loop {
        match client.receiver.recv().unwrap() {
            Message::Response(r) if r.id == init_id => break,
            _ => continue,
        }
    }

    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::Initialized::METHOD.to_string(),
            params: serde_json::to_value(InitializedParams {}).unwrap(),
        }))
        .unwrap();
}

fn shutdown(client: &Connection, server_thread: thread::JoinHandle<()>) {
    let shutdown_id: RequestId = 999.into();
    client
        .sender
        .send(Message::Request(Request {
            id: shutdown_id.clone(),
            method: Shutdown::METHOD.to_string(),
            params: Value::Null,
        }))
        .unwrap();
    loop {
        match client.receiver.recv().unwrap() {
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

fn recv_publish_diagnostics(client: &Connection) -> PublishDiagnosticsParams {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.receiver.recv_timeout(remaining) {
            Ok(Message::Notification(n))
                if n.method == lsp_types::notification::PublishDiagnostics::METHOD =>
            {
                return serde_json::from_value(n.params).unwrap();
            }
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for publishDiagnostics"),
        }
    }
}

#[test]
fn initialize_handshake_advertises_capabilities() {
    let (client, server_thread) = spawn_server();

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

    shutdown(&client, server_thread);
}

#[test]
fn did_open_publishes_parse_diagnostics_for_broken_source() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///broken.flx").unwrap();
    // `}` at the top level is a parser error (E_unexpected_token style).
    let text = "}\n".to_string();
    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::DidOpenTextDocument::METHOD.to_string(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "flux".to_string(),
                    version: 1,
                    text,
                },
            })
            .unwrap(),
        }))
        .unwrap();

    let params = recv_publish_diagnostics(&client);
    assert_eq!(params.uri, uri);
    assert!(
        !params.diagnostics.is_empty(),
        "expected at least one diagnostic for broken source"
    );
    let first = &params.diagnostics[0];
    assert_eq!(first.source.as_deref(), Some("flux"));
    assert_eq!(
        first.severity,
        Some(lsp_types::DiagnosticSeverity::ERROR),
        "first diagnostic should be an error: {first:?}"
    );

    shutdown(&client, server_thread);
}

fn open_document(client: &Connection, uri: &Uri, text: &str) {
    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::DidOpenTextDocument::METHOD.to_string(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "flux".to_string(),
                    version: 1,
                    text: text.to_string(),
                },
            })
            .unwrap(),
        }))
        .unwrap();
    // Drain the initial publishDiagnostics so later request-response loops
    // don't pick it up as a stray.
    let _ = recv_publish_diagnostics(client);
}

fn request_response(client: &Connection, id: i32, method: &str, params: Value) -> Value {
    let request_id: RequestId = id.into();
    client
        .sender
        .send(Message::Request(Request {
            id: request_id.clone(),
            method: method.to_string(),
            params,
        }))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match client.receiver.recv_timeout(remaining) {
            Ok(Message::Response(r)) if r.id == request_id => {
                assert!(r.error.is_none(), "request error: {:?}", r.error);
                return r.result.expect("missing result");
            }
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for response to {method}"),
        }
    }
}

#[test]
fn document_symbol_lists_top_level_items() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///syms.flx").unwrap();
    let text = "fn greet(name) { name }\nlet answer = 42\n";
    open_document(&client, &uri, text);

    let params = DocumentSymbolParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let result = request_response(
        &client,
        100,
        DocumentSymbolRequest::METHOD,
        serde_json::to_value(params).unwrap(),
    );

    let symbols = result.as_array().expect("array response");
    let names: Vec<&str> = symbols
        .iter()
        .filter_map(|s| s.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"greet"), "expected `greet`, got: {names:?}");
    assert!(
        names.contains(&"answer"),
        "expected `answer`, got: {names:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn hover_on_integer_literal_returns_type() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///hov.flx").unwrap();
    let text = "let x = 42\n";
    open_document(&client, &uri, text);

    // Cursor on the `4` of `42` — column index of the `4` is 8 (0-based).
    let params = HoverParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 0,
                character: 8,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let result = request_response(
        &client,
        200,
        HoverRequest::METHOD,
        serde_json::to_value(params).unwrap(),
    );

    assert!(!result.is_null(), "expected a Hover result, got null");
    let contents = &result["contents"];
    let value = contents["value"]
        .as_str()
        .expect("hover contents should have a string value");
    assert!(
        value.to_lowercase().contains("int"),
        "expected integer type in hover, got: {value}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn goto_definition_resolves_local_let() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///def.flx").unwrap();
    // Line 0: `let answer = 42`
    // Line 1: `let result = answer`
    let text = "let answer = 42\nlet result = answer\n";
    open_document(&client, &uri, text);

    // Cursor on the `answer` use on line 1 (after `let result = `).
    let params = GotoDefinitionParams {
        text_document_position_params: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 1,
                character: 15,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let result = request_response(
        &client,
        300,
        GotoDefinition::METHOD,
        serde_json::to_value(params).unwrap(),
    );

    assert!(!result.is_null(), "expected definition location, got null");
    let uri_back = result["uri"].as_str().expect("uri");
    assert!(
        uri_back.contains("def.flx"),
        "definition uri should match source uri: {uri_back}"
    );
    let start_line = result["range"]["start"]["line"]
        .as_u64()
        .expect("start line");
    assert_eq!(start_line, 0, "definition should point to line 0");

    shutdown(&client, server_thread);
}

#[test]
fn completion_returns_top_level_symbols_and_keywords() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///comp.flx").unwrap();
    let text = "fn greet(name) { name }\nlet answer = 42\n";
    open_document(&client, &uri, text);

    let params = CompletionParams {
        text_document_position: TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            position: Position {
                line: 2,
                character: 0,
            },
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: None,
    };
    let result = request_response(
        &client,
        400,
        Completion::METHOD,
        serde_json::to_value(params).unwrap(),
    );

    let items = result.as_array().expect("array completion response");
    let labels: Vec<&str> = items
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(
        labels.contains(&"greet"),
        "completion should include `greet`, got: {labels:?}"
    );
    assert!(
        labels.contains(&"answer"),
        "completion should include `answer`, got: {labels:?}"
    );
    assert!(
        labels.contains(&"let"),
        "completion should include the `let` keyword, got: {labels:?}"
    );

    shutdown(&client, server_thread);
}

#[test]
fn formatting_request_returns_text_edits_for_messy_source() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///fmt.flx").unwrap();
    // Messy spacing/blank lines so the formatter actually changes something.
    let text = "let x  =  1\n\n\n\nlet y = 2\n";
    open_document(&client, &uri, text);

    let params = DocumentFormattingParams {
        text_document: TextDocumentIdentifier { uri: uri.clone() },
        options: FormattingOptions {
            tab_size: 4,
            insert_spaces: true,
            ..Default::default()
        },
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let result = request_response(
        &client,
        500,
        Formatting::METHOD,
        serde_json::to_value(params).unwrap(),
    );

    let edits = result.as_array().expect("array TextEdits");
    assert!(
        !edits.is_empty(),
        "expected at least one text edit for messy source"
    );
    let new_text = edits[0]["newText"].as_str().expect("newText");
    // The formatter normalizes whitespace; quickly check the output differs.
    assert_ne!(new_text, text, "formatter should produce different text");

    shutdown(&client, server_thread);
}

#[test]
fn did_open_clean_source_publishes_no_diagnostics() {
    let (client, server_thread) = spawn_server();
    drive_initialize(&client);

    let uri = Uri::from_str("file:///clean.flx").unwrap();
    let text = "let x = 1\n".to_string();
    client
        .sender
        .send(Message::Notification(Notification {
            method: lsp_types::notification::DidOpenTextDocument::METHOD.to_string(),
            params: serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "flux".to_string(),
                    version: 1,
                    text,
                },
            })
            .unwrap(),
        }))
        .unwrap();

    let params = recv_publish_diagnostics(&client);
    assert_eq!(params.uri, uri);
    assert!(
        params.diagnostics.is_empty(),
        "expected zero diagnostics on clean source, got {:?}",
        params.diagnostics
    );

    shutdown(&client, server_thread);
}
