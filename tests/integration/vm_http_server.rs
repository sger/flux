//! VM HTTP/1.1 server tests (proposal 0174 Phase 3a).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NEXT_PORT: AtomicUsize = AtomicUsize::new(19880);

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
import Flow.Tcp as Tcp

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
fn shutdown_stops_accepting_new_connections() {
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
