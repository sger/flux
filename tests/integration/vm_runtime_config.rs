//! VM `Async.run_async_with` + `RuntimeConfig` end-to-end (proposal 0174 Phase 2 slice 2-vii).
//!
//! Drives the full `flux` CLI over fixtures that exercise the new
//! `RuntimeConfig` surface and the `FLUX_WORKERS` env-var fallback. The
//! tests verify the wiring runs end-to-end without crashing and produces
//! the expected source-level value. Direct introspection of the worker
//! thread count (e.g., asserting `current_num_workers() == 1`) is not
//! reachable from a CLI-driven integration test; that assertion lives in
//! the in-process unit test module that drives `vm_fibers` directly.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source_with_env(source: &str, env: &[(&str, &str)]) -> (String, String, bool, Duration) {
    flux_runner::run_flux_with_env(source, "runtime_config", env)
}

#[test]
fn run_async_with_explicit_single_worker_returns_value() {
    // RuntimeConfig with worker_count = Some(1). Verifies the ADT
    // pattern-match in `run_async_with` and the primop dispatch land
    // an Int return value.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    42
}

fn main() with IO {
    let v = run_async_with(with_worker_count(1), body)
    print(v)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source_with_env(source, &[]);
    assert!(
        success,
        "run_async_with(Some(1)) program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected 42 on stdout, got: {stdout:?}"
    );
    assert!(elapsed < Duration::from_secs(5));
}

#[test]
fn run_async_with_workers_convenience_returns_value() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    123
}

fn main() with IO {
    let v = run_async_with_workers(1, body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, &[]);
    assert!(
        success,
        "run_async_with_workers program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("123"),
        "expected 123 on stdout, got: {stdout:?}"
    );
}

#[test]
fn run_async_with_defaults_returns_value() {
    // worker_count = None means "use the runtime default". The path
    // through `run_async_with` is the same; only the resolved worker
    // count differs.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    7
}

fn main() with IO {
    let v = run_async_with(default_runtime_config(), body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, &[]);
    assert!(
        success,
        "run_async_with(default) program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("7"),
        "expected 7 on stdout, got: {stdout:?}"
    );
}

#[test]
fn flux_workers_env_var_does_not_break_default_run_async() {
    // `FLUX_WORKERS=1` is the env-var fallback for `Async.run_async`
    // (the default-config path). Setting it must not break programs
    // that use the existing zero-config `run_async`. Direct verification
    // that the scheduler honored the env value lives in the unit test
    // module that pokes `vm_fibers` directly; the integration test
    // only asserts end-to-end correctness.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    99
}

fn main() with IO {
    let v = run_async(body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, &[("FLUX_WORKERS", "1")]);
    assert!(
        success,
        "run_async with FLUX_WORKERS=1 must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("99"),
        "expected 99 on stdout, got: {stdout:?}"
    );
}

#[test]
fn run_async_with_explicit_dns_pool_resolves_hostname() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {
    let l = listen("127.0.0.1", 21974)
    let conn = accept(l)
    let msg = read(conn, 1024)
    let _ = write_all(conn, msg)
    close(conn)
}

fn client() -> String with Async {
    let conn = connect("localhost", 21974)
    let _ = write_all(conn, "dns-config")
    let reply = read(conn, 1024)
    close(conn)
    reply
}

fn body() -> String with Async {
    let pair = both(server, client)
    pair.1
}

fn main() with IO {
    print(run_async_with(with_dns_pool_size(1), body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, &[]);
    assert!(
        success,
        "run_async_with dns_pool_size=1 must resolve localhost:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("dns-config"),
        "expected echoed dns-config, got: {stdout:?}"
    );
}

#[test]
fn flux_dns_threads_env_var_resolves_hostname() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Tcp exposing (..)

fn server() -> Unit with Async {
    let l = listen("127.0.0.1", 21975)
    let conn = accept(l)
    let msg = read(conn, 1024)
    let _ = write_all(conn, msg)
    close(conn)
}

fn client() -> String with Async {
    let conn = connect("localhost", 21975)
    let _ = write_all(conn, "dns-env")
    let reply = read(conn, 1024)
    close(conn)
    reply
}

fn body() -> String with Async {
    let pair = both(server, client)
    pair.1
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(source, &[("FLUX_DNS_THREADS", "1")]);
    assert!(
        success,
        "FLUX_DNS_THREADS=1 must configure hostname resolution:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("dns-env"),
        "expected echoed dns-env, got: {stdout:?}"
    );
}
