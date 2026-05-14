//! VM TCP echo integration test (proposal 0174 Phase 1b-vi-e).
//!
//! Runs a complete loopback echo: listener fiber accepts one connection,
//! echoes the payload back; client fiber connects, writes, reads reply.
//! Both run concurrently via `Async.both` on the fiber scheduler.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::net::TcpListener;
use std::time::Duration;

// Bind to port 0 to let the OS pick a free port, then release the listener
// immediately.  There is a small TOCTOU window before the Flux subprocess
// binds, but this is acceptable for integration tests and avoids hardcoded
// port collisions when multiple CI jobs run in parallel.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind port 0")
        .local_addr()
        .expect("local_addr")
        .port()
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn tcp_loopback_echo_round_trips() {
    let port = free_port();
    let source = format!(
        r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {{
    let l = listen("127.0.0.1", {port})
    let conn = accept(l)
    let msg = read(conn, 1024)
    let _w = write_all(conn, msg)
    close(conn)
}}

fn client() -> String with Async {{
    let conn = connect("127.0.0.1", {port})
    let _w = write_all(conn, "hello")
    let reply = read(conn, 1024)
    close(conn)
    reply
}}

fn body() -> String with Async {{
    let pair = both(server, client)
    pair.1
}}

fn main() with IO {{
    let reply = run_async(body)
    print(reply)
}}
"#
    );
    let (stdout, stderr, success, _elapsed) = run_source(&source, "echo");
    assert!(
        success,
        "tcp echo must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim() == "\"hello\"",
        "expected '\"hello\"', got:\n{stdout}"
    );
}

#[test]
fn tcp_listen_accept_close() {
    let port = free_port();
    let source = format!(
        r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {{
    let l = listen("127.0.0.1", {port})
    let conn = accept(l)
    close(conn)
}}

fn client() -> Unit with Async {{
    let conn = connect("127.0.0.1", {port})
    close(conn)
}}

fn body() -> Unit with Async {{
    let _ = both(server, client)
}}

fn main() with IO {{
    let _ = run_async(body)
    print("ok")
}}
"#
    );
    let (stdout, stderr, success, _elapsed) = run_source(&source, "accept_close");
    assert!(
        success,
        "tcp listen/accept/close must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim() == "\"ok\"",
        "expected '\"ok\"', got:\n{stdout}"
    );
}
