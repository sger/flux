//! VM HTTP/1.1 server tests (proposal 0174 Phase 3a).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    listener.local_addr().expect("local addr").port()
}

fn write_fixture(source: String) -> PathBuf {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("flux-vm-http-{}-{id}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn server_source(port: u16, max_connections: i64, max_header: i64, max_body: i64) -> String {
    format!(
        r#"
import Flow.Async exposing (..)
import Flow.Http exposing (..)

fn handler(req) with Async {{
    ok(req.path + ":" + req.body)
}}

fn main() with IO {{
    let config = server_config({max_connections}, {max_header}, {max_body}, 30000)
    let _ = run_async(fn() {{ serve_config("127.0.0.1", {port}, config, handler) }})
}}
"#
    )
}

fn spawn_server(path: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux server")
}

fn connect_retry(port: u16, child: &mut Child) -> TcpStream {
    let start = Instant::now();
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(err) if start.elapsed() < Duration::from_secs(10) => {
                let _ = err;
                if let Ok(Some(status)) = child.try_wait() {
                    panic!("server exited before accepting connections: {status}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                let _ = child.kill();
                panic!("server did not accept connections: {err}");
            }
        }
    }
}

fn send_request(port: u16, child: &mut Child, request: &[u8]) -> String {
    let mut stream = connect_retry(port, child);
    stream.write_all(request).expect("write request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    response
}

fn wait_success(mut child: Child) -> Output {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().expect("wait server"),
            Ok(None) if start.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let output = child.wait_with_output().expect("wait killed server");
                panic!(
                    "server did not exit:\nstdout:\n{}\nstderr:\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(err) => panic!("wait server failed: {err}"),
        }
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "server process failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn serve_config_returns_after_one_configured_connection() {
    let port = free_port();
    let path = write_fixture(server_source(port, 1, 65_536, 8_388_608));
    let mut child = spawn_server(&path);

    let response = send_request(
        port,
        &mut child,
        b"GET /hello HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("Content-Length: 7\r\n"), "{response}");
    assert!(response.ends_with("\r\n\r\n/hello:"), "{response}");
}

#[test]
fn serve_config_handles_two_sequential_connections() {
    let port = free_port();
    let path = write_fixture(server_source(port, 2, 65_536, 8_388_608));
    let mut child = spawn_server(&path);

    let first = send_request(
        port,
        &mut child,
        b"GET /one HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    let second = send_request(
        port,
        &mut child,
        b"GET /two HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert!(first.ends_with("\r\n\r\n/one:"), "{first}");
    assert!(second.ends_with("\r\n\r\n/two:"), "{second}");
}

#[test]
fn serve_config_handles_two_keep_alive_requests_on_one_connection() {
    let port = free_port();
    let path = write_fixture(server_source(port, 1, 65_536, 8_388_608));
    let mut child = spawn_server(&path);
    let mut stream = connect_retry(port, &mut child);

    stream
        .write_all(
            b"GET /one HTTP/1.1\r\nHost: local\r\n\r\nGET /two HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
        )
        .expect("write requests");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read response");
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert_eq!(response.matches("HTTP/1.1 200 OK\r\n").count(), 2, "{response}");
    assert!(response.contains("\r\n\r\n/one:"), "{response}");
    assert!(response.ends_with("\r\n\r\n/two:"), "{response}");
}

#[test]
fn malformed_request_returns_400_without_invoking_handler() {
    let port = free_port();
    let path = write_fixture(server_source(port, 1, 65_536, 8_388_608));
    let mut child = spawn_server(&path);

    let response = send_request(
        port,
        &mut child,
        b"get /bad HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{response}");
    assert!(!response.contains("/bad:"), "{response}");
}

#[test]
fn oversized_body_returns_413_without_invoking_handler() {
    let port = free_port();
    let path = write_fixture(server_source(port, 1, 65_536, 4));
    let mut child = spawn_server(&path);

    let response = send_request(
        port,
        &mut child,
        b"POST /big HTTP/1.1\r\nHost: local\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    );
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert!(
        response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"),
        "{response}"
    );
    assert!(!response.contains("/big:"), "{response}");
}

#[test]
fn oversized_header_returns_413_without_invoking_handler() {
    let port = free_port();
    let path = write_fixture(server_source(port, 1, 24, 8_388_608));
    let mut child = spawn_server(&path);

    let response = send_request(
        port,
        &mut child,
        b"GET /wide HTTP/1.1\r\nHost: local\r\nConnection: close\r\n\r\n",
    );
    let output = wait_success(child);
    let _ = std::fs::remove_file(&path);

    assert_success(&output);
    assert!(
        response.starts_with("HTTP/1.1 413 Payload Too Large\r\n"),
        "{response}"
    );
    assert!(!response.contains("/wide:"), "{response}");
}
