//! Native LLVM HTTP/1.1 client helper parity tests (proposal 0174 Phase 3).

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);
static NEXT_PORT: AtomicUsize = AtomicUsize::new(23880);
static NATIVE_HTTP_CLIENT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn next_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::Relaxed) as u16
}

fn write_fixture(source: String) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flux-native-http-client-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn run_source(source: String) -> (String, String, bool) {
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

fn native_http_client_test_lock() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_HTTP_CLIENT_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("native HTTP client test lock poisoned")
}

fn client_server_source(port: u16, client: &str, handler: &str) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

{handler}

fn server() -> Unit with Async, AsyncFail {{
    let h = serve("127.0.0.1", {port}, handler)
    let _sleep = sleep(350)
    shutdown(h)
}}

{client}

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

fn raw_server_source(port: u16, server_body: &str, client_body: &str) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
import Flow.Tcp as Tcp

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
        "native client fixture failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

#[test]
fn native_http_client_get_loopback() {
    let _guard = native_http_client_test_lock();
    let port = next_port();
    let source = client_server_source(
        port,
        &format!(
            r#"
fn client() -> String with Async, AsyncFail {{
    let _wait = sleep(50)
    let resp = get("http://127.0.0.1:{port}/hello")
    resp.body
}}
"#
        ),
        r#"
fn handler(req) with Async {
    ok(req.path + ":ok")
}
"#,
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("/hello:ok"), "{stdout}");
}

#[test]
fn native_http_client_post_loopback() {
    let _guard = native_http_client_test_lock();
    let port = next_port();
    let source = client_server_source(
        port,
        &format!(
            r#"
fn client() -> String with Async, AsyncFail {{
    let _wait = sleep(50)
    let resp = post("http://127.0.0.1:{port}/echo", "payload")
    resp.body
}}
"#
        ),
        r#"
fn handler(req) with Async {
    ok(req.path + ":" + req.body)
}
"#,
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("/echo:payload"), "{stdout}");
}

#[test]
fn native_http_client_decodes_chunked_response() {
    let _guard = native_http_client_test_lock();
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
fn native_http_client_malformed_response_fails() {
    let _guard = native_http_client_test_lock();
    let port = next_port();
    let source = raw_server_source(
        port,
        r#"
    let _raw = Tcp.read(conn, 4096)
    let _write = Tcp.write_all(conn, "NOPE\r\n\r\n")
"#,
        &format!(
            r#"
    fn call_get() -> Response with Async, AsyncFail {{
        get("http://127.0.0.1:{port}/bad")
    }}
    let result = try_(call_get)
    if result_is_ok(result) {{ "unexpected-ok" }} else {{ "protocol-failed" }}
"#
        ),
    );
    let stdout = run_ok(source);
    assert!(stdout.contains("protocol-failed"), "{stdout}");
}

#[test]
fn native_http_client_rejects_https_scheme() {
    let _guard = native_http_client_test_lock();
    let source = r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn body() -> String with Async, AsyncFail {
    fn call_get() -> Response with Async, AsyncFail {
        get("https://example.test/")
    }
    let result = try_(call_get)
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
