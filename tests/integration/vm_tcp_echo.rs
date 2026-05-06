//! VM TCP echo integration test (proposal 0174 Phase 1b-vi-e).
//!
//! Runs a complete loopback echo: listener fiber accepts one connection,
//! echoes the payload back; client fiber connects, writes, reads reply.
//! Both run concurrently via `Async.both` on the fiber scheduler.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-tcp-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn tcp_loopback_echo_round_trips() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {
    let l = listen("127.0.0.1", 19871)
    let conn = accept(l)
    let msg = read(conn, 1024)
    let _w = write_all(conn, msg)
    close(conn)
}

fn client() -> String with Async {
    let conn = connect("127.0.0.1", 19871)
    let _w = write_all(conn, "hello")
    let reply = read(conn, 1024)
    close(conn)
    reply
}

fn body() -> String with Async {
    let pair = both(server, client)
    pair.1
}

fn main() with IO {
    let reply = run_async(body)
    print(reply)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "echo");
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
    let source = r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {
    let l = listen("127.0.0.1", 19872)
    let conn = accept(l)
    close(conn)
}

fn client() -> Unit with Async {
    let conn = connect("127.0.0.1", 19872)
    close(conn)
}

fn body() -> Unit with Async {
    let _ = both(server, client)
}

fn main() with IO {
    let _ = run_async(body)
    print("ok")
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "accept_close");
    assert!(
        success,
        "tcp listen/accept/close must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.trim() == "\"ok\"",
        "expected '\"ok\"', got:\n{stdout}"
    );
}
