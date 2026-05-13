//! Native LLVM HTTP/1.1 server parity tests (proposal 0174 Phase 3).

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NEXT_PORT: AtomicUsize = AtomicUsize::new(21880);
static NATIVE_HTTP_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn next_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed) as u16
}

fn write_fixture(source: String) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-native-http-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: String) -> (String, String, bool) {
    let _guard = NATIVE_HTTP_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native HTTP test lock poisoned");
    let path = write_fixture(source);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run native flux");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

fn lifecycle_source(body: &str) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
import Flow.Map as Map
import Flow.Stream as Stream
import Flow.Tcp as Tcp

fn read_all_http(conn, acc: String) -> String with Async {{
    fn read_once() with Async {{
        Tcp.read(conn, 4096)
    }}
    let read_result = timeout_result(500, read_once)
    let chunk = result_or(read_result, "")
    if chunk == "" {{ acc }} else {{ read_all_http(conn, acc + chunk) }}
}}

{body}

fn main() with IO {{
    print(run_async_with(with_worker_count(1), body))
}}
"#
    )
}

fn basic_source(
    port: u16,
    max_header: i64,
    max_body: i64,
    timeout: i64,
    request: &str,
    handler: &str,
) -> String {
    lifecycle_source(&format!(
        r#"
{handler}

fn server() -> Unit with Async, AsyncFail {{
    let config = server_config(1, {max_header}, {max_body}, {timeout})
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(350)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, {request:?})
    let response = Tcp.read(conn, 4096)
    let tail = Tcp.read(conn, 4096)
    Tcp.close(conn)
    response + tail
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ))
}

#[test]
fn native_http_serves_one_request() {
    let port = next_port();
    let source = basic_source(
        port,
        65_536,
        8_388_608,
        30_000,
        "GET /hello HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        r#"
fn handler(req) with Async {
    ok(req.path + ":" + req.body)
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("/hello:"), "{stdout}");
}

#[test]
fn native_http_keep_alive_serves_two_pipelined_requests() {
    let port = next_port();
    let source = basic_source(
        port,
        65_536,
        8_388_608,
        30_000,
        "GET /one HTTP/1.1\r\nHost: local\r\n\r\nGET /two HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        r#"
fn handler(req) with Async {
    ok(req.path)
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.matches("HTTP/1.1 200 OK").count(), 2, "{stdout}");
    assert!(stdout.contains("/one"), "{stdout}");
    assert!(stdout.contains("/two"), "{stdout}");
}

#[test]
fn native_http_serves_repeated_browser_style_connections() {
    let port = next_port();
    let source = format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
import Flow.Tcp as Tcp

fn handler(req) with Async {{
    ok(req.path)
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    let _sleep = sleep(700)
    shutdown(h)
}}

fn request_root() -> String with Async {{
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET / HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = Tcp.read(conn, 4096)
    Tcp.close(conn)
    response
}}

fn request_favicon() -> String with Async {{
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /favicon.ico HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = Tcp.read(conn, 4096)
    Tcp.close(conn)
    response
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let _one: String = request_root()
    let _two: String = request_favicon()
    let _three: String = request_root()
    "ok"
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}

fn main() with IO {{
    print(run_async_with(with_worker_count(1), body))
}}
"#
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native repeated HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn native_http_malformed_request_returns_400() {
    let port = next_port();
    let source = basic_source(
        port,
        65_536,
        8_388_608,
        30_000,
        "get /bad HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        r#"
fn handler(req) with Async {
    ok("unexpected")
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 400 Bad Request"), "{stdout}");
    assert!(!stdout.contains("unexpected"), "{stdout}");
}

#[test]
fn native_http_oversized_body_returns_413() {
    let port = next_port();
    let source = basic_source(
        port,
        65_536,
        4,
        30_000,
        "POST /big HTTP/1.1\r\nHost: local\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        r#"
fn handler(req) with Async {
    ok("unexpected")
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("HTTP/1.1 413 Payload Too Large"),
        "{stdout}"
    );
    assert!(!stdout.contains("unexpected"), "{stdout}");
}

#[test]
fn native_http_oversized_header_returns_413() {
    let port = next_port();
    let source = basic_source(
        port,
        24,
        8_388_608,
        30_000,
        "GET /wide HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        r#"
fn handler(req) with Async {
    ok("unexpected")
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("HTTP/1.1 413 Payload Too Large"),
        "{stdout}"
    );
    assert!(!stdout.contains("unexpected"), "{stdout}");
}

#[test]
fn native_http_handler_timeout_returns_504() {
    let port = next_port();
    let source = basic_source(
        port,
        65_536,
        8_388_608,
        50,
        "GET /slow HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        r#"
fn handler(req) with Async {
    let _slow = sleep(200)
    ok("too-late")
}
"#,
    );
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 504 Gateway Timeout"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}

#[test]
fn native_http_shutdown_drains_active_handler() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _slow = sleep(250)
    ok("drained")
}}

fn server() -> String with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    let _sleep = sleep(150)
    shutdown(h)
    "stopped"
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /drain HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = Tcp.read(conn, 4096)
    Tcp.close(conn)
    response
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("drained"), "{stdout}");
}

#[test]
fn native_http_shutdown_now_closes_active_connection() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _slow = sleep(1000)
    ok("too-late")
}}

fn server() -> String with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    let _sleep = sleep(100)
    shutdown_now(h)
    "forced"
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /force HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    fn read_once() with Async {{
        Tcp.read(conn, 4096)
    }}
    let read_result = timeout_result(500, read_once)
    Tcp.close(conn)
    let bytes = result_or(read_result, "closed")
    if bytes == "" {{ "closed" }} else {{ bytes }}
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native HTTP fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("closed"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}

#[test]
fn native_http_streaming_response_writes_chunked_frames() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    stream_response(200, {{}}, Stream.from_array([|"hello", " native"|]))
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(350)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /stream HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = read_all_http(conn, "")
    Tcp.close(conn)
    response
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native streaming fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("Transfer-Encoding: chunked"), "{stdout}");
    assert!(stdout.contains("5\nhello\n"), "{stdout}");
    assert!(stdout.contains("7\n native\n"), "{stdout}");
    assert!(stdout.contains("0\n\n"), "{stdout}");
}

#[test]
fn native_http_sse_response_emits_frames() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    sse_response(Stream.from_array([|sse_event("one"), sse_named_event("tick", "two")|]))
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(350)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /events HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = read_all_http(conn, "")
    Tcp.close(conn)
    response
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native SSE fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Content-Type: text/event-stream"),
        "{stdout}"
    );
    assert!(stdout.contains("Cache-Control: no-cache"), "{stdout}");
    assert!(stdout.contains("data: one\n\n"), "{stdout}");
    assert!(stdout.contains("event: tick\ndata: two\n\n"), "{stdout}");
}

#[test]
fn native_http_shutdown_now_cancels_streaming_response() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn very_late_chunk() {{
    Stream.make(fn() {{
        sleep(1000)
        Some(("too-late", Stream.empty()))
    }})
}}

fn handler(req) with Async {{
    stream_response(200, {{}}, very_late_chunk())
}}

fn server() -> String with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(100)
    shutdown_now(h)
    "forced"
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /force HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
    let response = read_all_http(conn, "")
    Tcp.close(conn)
    response
}}

fn body() -> String with Async, AsyncFail {{
    let pair = both(server, client)
    pair.1
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "native streaming forced shutdown fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Transfer-Encoding: chunked"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}
