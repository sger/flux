//! VM HTTP/1.1 client helper tests (proposal 0174 Phase 3).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NEXT_PORT: AtomicUsize = AtomicUsize::new(22880);
static VM_HTTP_CLIENT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn next_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed) as u16
}

fn write_fixture(source: String) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-vm-http-client-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: String) -> (String, String, bool) {
    let mutex = VM_HTTP_CLIENT_TEST_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    let path = write_fixture(source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

fn raw_server_source(port: u16, server_body: &str, client_body: &str) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
import Flow.Http as Http
import Flow.Map as Map
import Flow.Tcp as Tcp
import Flow.String as Str

fn server() -> Unit with Async {{
    let listener = Tcp.listen("127.0.0.1", {port})
    let conn = Tcp.accept(listener)
    {server_body}
    Tcp.close(conn)
}}

fn client() -> String with Async, AsyncFail {{
    let _wait = sleep(50)
    {client_body}
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}

fn main() with IO {{
    print(run_async(body))
}}
"#
    )
}

fn run_ok(source: String) -> String {
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

fn spawn_one_response_server(port: u16, response: &'static [u8]) -> std::thread::JoinHandle<()> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind response server");
        ready_tx.send(()).expect("signal ready");
        let (mut stream, _) = listener.accept().expect("accept client");
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        stream.write_all(response).expect("write response");
    });
    ready_rx.recv().expect("response server ready");
    handle
}

#[test]
fn get_returns_response_fields_from_local_server() {
    let port = next_port();
    let source = raw_server_source(
        port,
        r#"
    let _raw = Tcp.read(conn, 4096)
    let _write = Tcp.write_all(conn, "HTTP/1.1 201 Created\r\nX-Test: vm-get\r\nConnection: close\r\nContent-Length: 9\r\n\r\n/hello:ok")
"#,
        &format!(
            r#"
    let resp = get("http://127.0.0.1:{port}/hello")
    let status = if resp.status == 201 {{ "created" }} else {{ "bad-status" }}
    let header = match Map.get(resp.headers, "X-Test") {{
        Some(value) -> value,
        _ -> "missing-header"
    }}
    status + ":" + header + ":" + resp.body
"#
        ),
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("created:vm-get:/hello:ok"), "{stdout}");
}

#[test]
fn post_sends_body_and_returns_response() {
    let port = next_port();
    let source = raw_server_source(
        port,
        r#"
    let raw = Tcp.read(conn, 4096)
    let body = if Str.str_contains(raw, "payload") { "/echo:payload" } else { "missing" }
    let wire = "HTTP/1.1 202 Accepted\r\nX-Mode: vm-post\r\nConnection: close\r\nContent-Length: 13\r\n\r\n" + body
    let _write = Tcp.write_all(conn, wire)
"#,
        &format!(
            r#"
    let resp = post("http://127.0.0.1:{port}/echo", "payload")
    let status = if resp.status == 202 {{ "accepted" }} else {{ "bad-status" }}
    let header = match Map.get(resp.headers, "X-Mode") {{
        Some(value) -> value,
        _ -> "missing-header"
    }}
    status + ":" + header + ":" + resp.body
"#
        ),
    );
    let stdout = run_ok(source);
    assert!(
        stdout.contains("accepted:vm-post:/echo:payload"),
        "{stdout}"
    );
}

#[test]
fn request_writes_custom_headers() {
    let port = next_port();
    let source = raw_server_source(
        port,
        r#"
    let raw = Tcp.read(conn, 4096)
    let body = if Str.str_contains(raw, "X-Test: yes") { "custom" } else { "missing" }
    let wire = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 6\r\n\r\n" + body
    let _write = Tcp.write_all(conn, wire)
"#,
        &format!(
            r#"
    let resp = Http.request(Http.method_get(), "http://127.0.0.1:{port}/headers", {{"X-Test": "yes"}}, "")
    resp.body
"#
        ),
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("custom"), "{stdout}");
}

#[test]
fn malformed_response_returns_protocol_failure() {
    let port = next_port();
    let handle = spawn_one_response_server(port, b"NOPE\r\n\r\n");
    let source = format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn body() -> String with Async, AsyncFail {{
    fn call_get() -> Response with Async, AsyncFail {{
        get("http://127.0.0.1:{port}/bad")
    }}
    let result = try(call_get)
    if result_is_ok(result) {{ "unexpected-ok" }} else {{ "protocol-failed" }}
}}

fn main() with IO {{
    print(run_async(body))
}}
"#
    );
    let stdout = run_ok(source);
    handle.join().expect("response server join");
    assert!(stdout.contains("protocol-failed"), "{stdout}");
}

#[test]
fn chunked_response_body_is_decoded() {
    let port = next_port();
    let source = raw_server_source(
        port,
        r#"
    let _raw = Tcp.read(conn, 4096)
    let _write = Tcp.write_all(conn, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n")
"#,
        &format!(
            r#"
    let resp = get("http://127.0.0.1:{port}/chunked")
    resp.body
"#
        ),
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("hello world"), "{stdout}");
}

#[test]
fn unsupported_https_url_is_rejected() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn body() -> String with Async, AsyncFail {
    fn call_get() -> Response with Async, AsyncFail {
        get("https://example.test/")
    }
    let result = try(call_get)
    if result_is_ok(result) { "unexpected-ok" } else { "rejected" }
}

fn main() with IO {
    print(run_async(body))
}
"#
    .to_string();
    let stdout = run_ok(source);
    assert!(stdout.contains("rejected"), "{stdout}");
}
