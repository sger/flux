//! VM HTTP/1.1 server smoke tests (proposal 0174 Phase 3a).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn http_serve_returns_handler_response_over_loopback() {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-http-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(
        &path,
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn handler(req) with Async {
    ok(req.path)
}

fn main() with IO {
    let _ = run_async(fn() { serve("127.0.0.1", 19893, handler) })
}
"#,
    )
    .expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux server");

    let start = Instant::now();
    let mut stream = loop {
        match TcpStream::connect("127.0.0.1:19893") {
            Ok(stream) => break stream,
            Err(err) if start.elapsed() < Duration::from_secs(10) => {
                let _ = err;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                let _ = child.kill();
                panic!("server did not accept connections: {err}");
            }
        }
    };

    stream
        .write_all(b"GET /hello HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n")
        .expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");

    let output = child.wait_with_output().expect("wait server");
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "server process failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Length: 6\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\n/hello"), "{response}");
}
