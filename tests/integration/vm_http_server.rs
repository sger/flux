//! VM HTTP/1.1 server tests (proposal 0174 Phase 3a).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NEXT_PORT: AtomicUsize = AtomicUsize::new(19880);
static VM_HTTP_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn next_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed) as u16
}

fn write_fixture(source: String) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-vm-http-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: String) -> (String, String, bool) {
    let mutex = VM_HTTP_TEST_LOCK.get_or_init(|| Mutex::new(()));
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

fn fixture_source(
    port: u16,
    max_connections: i64,
    max_header: i64,
    max_body: i64,
    request: &str,
) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
import Flow.Map as Map
import Flow.Stream as Stream
import Flow.Tcp as Tcp

fn handler(req) with Async {{
    ok(req.path + ":" + req.body)
}}

fn server() -> Unit with Async, AsyncFail {{
    let config = server_config({max_connections}, {max_header}, {max_body}, 30000)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(200)
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

fn body() -> String with Async {{
    let pair = both(server, client)
    pair.1
}}

fn main() with IO {{
    print(run_async(body))
}}
"#
    )
}

fn run_http_fixture(max_connections: i64, max_header: i64, max_body: i64, request: &str) -> String {
    let port = next_port();
    let (stdout, stderr, success) = run_source(fixture_source(
        port,
        max_connections,
        max_header,
        max_body,
        request,
    ));
    assert!(
        success,
        "server fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
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
    print(run_async(body))
}}
"#
    )
}

#[test]
fn serve_config_returns_and_serves_one_connection() {
    let response = run_http_fixture(
        1,
        65_536,
        8_388_608,
        "GET /hello HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    assert!(response.contains("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("Content-Length: 7"), "{response}");
    assert!(response.contains("/hello:"), "{response}");
}

#[test]
fn serve_config_returns_before_accepting() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    ok("late")
}}

fn body() -> String with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    shutdown_now(h)
    "returned"
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("returned"), "{stdout}");
}

#[test]
fn serve_config_rejects_negative_config_fields() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    ok("unexpected")
}}

fn body() -> String with Async, AsyncFail {{
    let config = server_config(0 - 1, 65536, 8388608, 30000)
    let _h = serve_config("127.0.0.1", {port}, config, handler)
    "unexpected"
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        !success,
        "negative config fixture unexpectedly succeeded:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("ServerConfig.max_connections must be non-negative"),
        "expected max_connections diagnostic, got:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn shutdown_stops_accepting_new_connections() {
    let mutex = VM_HTTP_TEST_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    ok("unexpected")
}}

fn body() -> String with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    let _sleep = sleep(50)
    shutdown(h)
    "stopped"
}}
"#
    ));
    let path = write_fixture(source);
    let child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn flux");
    std::thread::sleep(std::time::Duration::from_millis(250));
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let late_connect =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(200));
    let output = child.wait_with_output().expect("wait for flux");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let success = output.status.success();
    assert!(
        success,
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("stopped"), "{stdout}");
    assert!(
        late_connect.is_err(),
        "late connection unexpectedly succeeded"
    );
}

#[test]
fn max_connections_backpressures_live_connections() {
    let mutex = VM_HTTP_TEST_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = mutex.lock().unwrap_or_else(|e| e.into_inner());
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    if req.path == "/hold" {{
        let _slow = sleep(600)
        ok("hold")
    }} else {{
        ok("second")
    }}
}}

fn body() -> String with Async, AsyncFail {{
    let config = server_config(1, 65536, 8388608, 30000)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(1500)
    shutdown(h)
    "server-done"
}}
"#
    ));
    let path = write_fixture(source);
    let child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn flux");

    // The VM compiles all modules before the server starts accepting. Retry
    // connecting for up to 3s to accommodate slow/debug builds.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut first = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100))
            {
                Ok(s) => break s,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("connect first client: {e}"),
            }
        }
    };
    first
        .write_all(b"GET /hold HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
        .expect("write first request");

    // Wait long enough that the first request is definitely inside its 300ms
    // sleep, but short enough that it hasn't completed yet.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let mut second =
        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500))
            .expect("connect second client");
    second
        .write_all(b"GET /second HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
        .expect("write second request");
    // 120ms window to detect a response — first hold has ~150ms remaining so
    // a response here means backpressure didn't fire.
    second
        .set_read_timeout(Some(std::time::Duration::from_millis(120)))
        .expect("set second short timeout");
    let mut early = [0u8; 128];
    let early_read = second.read(&mut early);
    assert!(
        early_read.is_err(),
        "second request was served while first connection was still active: {early_read:?}"
    );

    first
        .set_read_timeout(Some(std::time::Duration::from_millis(2000)))
        .expect("set first timeout");
    let mut first_response = String::new();
    first
        .read_to_string(&mut first_response)
        .expect("read first response");
    assert!(first_response.contains("hold"), "{first_response}");

    second
        .set_read_timeout(Some(std::time::Duration::from_millis(1000)))
        .expect("set second timeout");
    let mut second_response = String::new();
    second
        .read_to_string(&mut second_response)
        .expect("read second response");
    assert!(second_response.contains("second"), "{second_response}");

    let output = child.wait_with_output().expect("wait for flux");
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    assert!(
        output.status.success(),
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("server-done"), "{stdout}");
}

#[test]
fn shutdown_drains_active_connection() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _slow = sleep(150)
    ok("drained")
}}

fn server() -> String with Async, AsyncFail {{
    let h = serve_config("127.0.0.1", {port}, default_config(), handler)
    let _sleep = sleep(80)
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("drained"), "{stdout}");
}

#[test]
fn handler_timeout_returns_504() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _slow = sleep(200)
    ok("too-late")
}}

fn server() -> Unit with Async, AsyncFail {{
    let config = server_config(1, 65536, 8388608, 50)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(300)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /slow HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 504 Gateway Timeout"), "{stdout}");
    assert!(stdout.contains("Gateway Timeout"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}

#[test]
fn fast_handler_succeeds_with_request_timeout_configured() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _fast = sleep(20)
    ok("fast")
}}

fn server() -> Unit with Async, AsyncFail {{
    let config = server_config(1, 65536, 8388608, 200)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(200)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /fast HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("fast"), "{stdout}");
}

#[test]
fn timed_out_handler_closes_pipelined_connection() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    if req.path == "/slow" {{
        let _slow = sleep(200)
        ok("too-late")
    }} else {{
        ok("second")
    }}
}}

fn server() -> Unit with Async, AsyncFail {{
    let config = server_config(1, 65536, 8388608, 50)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(300)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /slow HTTP/1.1\r\nHost: local\r\n\r\nGET /second HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.matches("HTTP/1.1 504 Gateway Timeout").count(),
        1,
        "{stdout}"
    );
    assert_eq!(stdout.matches("HTTP/1.1 200 OK").count(), 0, "{stdout}");
    assert!(!stdout.contains("second"), "{stdout}");
}

#[test]
fn handler_timeout_does_not_break_graceful_shutdown_drain() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let _slow = sleep(100)
    ok("drained")
}}

fn server() -> String with Async, AsyncFail {{
    let config = server_config(1, 65536, 8388608, 300)
    let h = serve_config("127.0.0.1", {port}, config, handler)
    let _sleep = sleep(80)
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("drained"), "{stdout}");
    assert!(!stdout.contains("Gateway Timeout"), "{stdout}");
}

#[test]
fn shutdown_now_cancels_active_connection() {
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
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("closed"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}

#[test]
fn shutdown_is_idempotent() {
    let graceful_port = next_port();
    let forced_port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    ok("ok")
}}

fn body() -> String with Async, AsyncFail {{
    let graceful = serve_config("127.0.0.1", {graceful_port}, default_config(), handler)
    let _graceful_sleep = sleep(20)
    shutdown(graceful)
    shutdown(graceful)
    let forced = serve_config("127.0.0.1", {forced_port}, default_config(), handler)
    let _forced_sleep = sleep(20)
    shutdown_now(forced)
    shutdown_now(forced)
    "ok"
}}
"#
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("ok"), "{stdout}");
}

#[test]
fn keep_alive_serves_two_requests_on_one_connection() {
    let response = run_http_fixture(
        1,
        65_536,
        8_388_608,
        "GET /one HTTP/1.1\r\nHost: local\r\n\r\nGET /two HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    assert_eq!(response.matches("HTTP/1.1 200 OK").count(), 2, "{response}");
    assert!(response.contains("/one:"), "{response}");
    assert!(response.contains("/two:"), "{response}");
}

#[test]
fn malformed_request_returns_400_without_invoking_handler() {
    let response = run_http_fixture(
        1,
        65_536,
        8_388_608,
        "get /bad HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    assert!(response.contains("HTTP/1.1 400 Bad Request"), "{response}");
    assert!(!response.contains("/bad:"), "{response}");
}

#[test]
fn oversized_body_returns_413_without_invoking_handler() {
    let response = run_http_fixture(
        1,
        65_536,
        4,
        "POST /big HTTP/1.1\r\nHost: local\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    );
    assert!(
        response.contains("HTTP/1.1 413 Payload Too Large"),
        "{response}"
    );
    assert!(!response.contains("/big:"), "{response}");
}

#[test]
fn oversized_header_returns_413_without_invoking_handler() {
    let response = run_http_fixture(
        1,
        24,
        8_388_608,
        "GET /wide HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    assert!(
        response.contains("HTTP/1.1 413 Payload Too Large"),
        "{response}"
    );
    assert!(!response.contains("/wide:"), "{response}");
}

#[test]
fn streaming_response_writes_chunked_frames() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    stream_response(200, {{}}, Stream.from_array([|"hello", " world"|]))
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(250)
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
        "streaming fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("Transfer-Encoding: chunked"), "{stdout}");
    assert!(stdout.contains("Connection: close"), "{stdout}");
    assert!(stdout.contains("5\nhello\n"), "{stdout}");
    assert!(stdout.contains("6\n world\n"), "{stdout}");
    assert!(stdout.contains("0\n\n"), "{stdout}");
    assert!(!stdout.contains("Content-Length"), "{stdout}");
}

#[test]
fn empty_streaming_response_writes_only_terminator() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    stream_response(204, {{}}, Stream.empty())
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(250)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /empty HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
        "empty streaming fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 204 No Content"), "{stdout}");
    assert!(stdout.contains("Transfer-Encoding: chunked"), "{stdout}");
    assert!(stdout.contains("0\n\n"), "{stdout}");
    assert!(!stdout.contains("1\n"), "{stdout}");
}

#[test]
fn streaming_response_preserves_safe_headers_and_closes_pipeline() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    let h1 = Map.set({{}}, "X-Flux", req.path)
    let h2 = Map.set(h1, "Content-Length", "999")
    if req.path == "/second" {{
        stream_response(200, h2, Stream.once("second"))
    }} else {{
        stream_response(202, h2, Stream.once("first"))
    }}
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(300)
    shutdown(h)
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /first HTTP/1.1\r\nHost: local\r\n\r\nGET /second HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
        "pipeline streaming fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 202 Accepted"), "{stdout}");
    assert!(stdout.contains("X-Flux: /first"), "{stdout}");
    assert!(!stdout.contains("Content-Length: 999"), "{stdout}");
    assert_eq!(stdout.matches("HTTP/1.1").count(), 1, "{stdout}");
    assert!(!stdout.contains("second"), "{stdout}");
}

#[test]
fn sse_response_emits_event_stream_frames() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn handler(req) with Async {{
    sse_response(Stream.from_array([|sse_event("one"), sse_named_event("tick", "two")|]))
}}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(250)
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
        "sse fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
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
fn streaming_shutdown_drains_delayed_stream() {
    let port = next_port();
    let source = lifecycle_source(&format!(
        r#"
fn delayed_chunk() {{
    Stream.make(fn() {{
        sleep(150)
        Some(("late", Stream.empty()))
    }})
}}

fn handler(req) with Async {{
    stream_response(200, {{}}, delayed_chunk())
}}

fn server() -> String with Async, AsyncFail {{
    let h = serve_stream("127.0.0.1", {port}, handler)
    let _sleep = sleep(80)
    shutdown(h)
    "stopped"
}}

fn client() -> String with Async {{
    let _wait = sleep(50)
    let conn = Tcp.connect("127.0.0.1", {port})
    let _write = Tcp.write_all(conn, "GET /drain HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
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
    ));
    let (stdout, stderr, success) = run_source(source);
    assert!(
        success,
        "streaming drain fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("HTTP/1.1 200 OK"), "{stdout}");
    assert!(stdout.contains("late"), "{stdout}");
}

#[test]
fn streaming_shutdown_now_cancels_in_flight_stream() {
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
    fn read_once() with Async {{
        Tcp.read(conn, 4096)
    }}
    let first = result_or(timeout_result(500, read_once), "")
    let second = result_or(timeout_result(500, read_once), "")
    Tcp.close(conn)
    first + second
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
        "streaming forced shutdown fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Transfer-Encoding: chunked"), "{stdout}");
    assert!(!stdout.contains("too-late"), "{stdout}");
}
