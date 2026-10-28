//! Native LLVM HTTP/1.1 client helper parity tests (proposal 0174 Phase 3).

#![cfg(feature = "llvm")]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use flux_runner::flux_source;
use flux_runner::scratch::free_port;

static NATIVE_HTTP_CLIENT_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn run_source(source: String) -> (String, String, bool) {
    let ran = flux_source(&source, "native_http_client_tests")
        .native()
        .run();
    (ran.stdout, ran.stderr, ran.success)
}

fn native_http_client_test_lock() -> std::sync::MutexGuard<'static, ()> {
    NATIVE_HTTP_CLIENT_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn raw_server_source(port: u16, server_body: &str, client_body: &str) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)
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
    print(run_async_with(with_worker_count(1), body))
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

fn spawn_malformed_response_server(listener: TcpListener) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("set malformed server nonblocking");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match listener.accept() {
                Ok((mut stream, _addr)) => {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut buf = [0_u8; 4096];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(b"NOPE\r\n\r\n");
                    return;
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    })
}

#[test]
fn native_http_client_get_exposes_response_fields() {
    let _guard = native_http_client_test_lock();
    let port = free_port();
    let source = raw_server_source(
        port,
        r#"
    let _raw = Tcp.read(conn, 4096)
    let _write = Tcp.write_all(conn, "HTTP/1.1 201 Created\r\nX-Test: native-get\r\nConnection: close\r\nContent-Length: 9\r\n\r\n/hello:ok")
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
    assert!(stdout.contains("created:native-get:/hello:ok"), "{stdout}");
}

#[test]
fn native_http_client_post_loopback() {
    let _guard = native_http_client_test_lock();
    let port = free_port();
    let source = raw_server_source(
        port,
        r#"
    let raw = Tcp.read(conn, 4096)
    let body = if Str.str_contains(raw, "payload") { "/echo:payload" } else { "missing" }
    let wire = "HTTP/1.1 202 Accepted\r\nX-Mode: native-post\r\nConnection: close\r\nContent-Length: 13\r\n\r\n" + body
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
        stdout.contains("accepted:native-post:/echo:payload"),
        "{stdout}"
    );
}

#[test]
fn native_http_client_decodes_chunked_response() {
    let _guard = native_http_client_test_lock();
    let port = free_port();
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
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind malformed response server");
    let port = listener
        .local_addr()
        .expect("read malformed response server port")
        .port();
    let server = spawn_malformed_response_server(listener);
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
        print(run_async_with(with_worker_count(1), body))
    }}
"#
    );
    let stdout = run_ok(source);
    server.join().expect("malformed response server thread");
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
