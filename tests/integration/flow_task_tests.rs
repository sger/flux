//! Flow.Task surface integration tests (proposal 0174 Phase 1a-vi follow-up).
//!
//! Drives the full `flux` CLI rather than the bare [`Compiler`] because
//! [`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) only resolves through
//! the driver's stdlib root.
//!
//! The fixture in [`tests/flux/flow_task_surface.flx`] runs through
//! `flux --test` and proves the positive type-level surface for `Int`,
//! `List<Int>`, tuples, `await`, and `cancel<a>` (no `Sendable` bound).
//!
//! D1 closes the cross-module class-bound gap for `Task.spawn<a: Sendable>`:
//! function-typed payloads are rejected through the imported `Flow.Task`
//! surface, while concrete sendable payloads still type-check.
//!
//! VM keeps sequential task execution. Native `Task.await` is
//! fiber-suspending: it parks only the calling fiber while native task workers
//! publish completions back into the async scheduler.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_flux_source(source: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!(
        "flux-flow-task-d1-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for Flow.Task D1 fixture");
    let path = dir.join("flow_task_d1.flx");
    std::fs::write(&path, source).expect("write Flow.Task D1 fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on Flow.Task D1 fixture");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

#[cfg(feature = "llvm")]
fn run_flux_source_native(source: &str, tag: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!(
        "flux-flow-task-native-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        tag
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for native Flow.Task fixture");
    let path = dir.join("flow_task_native_source.flx");
    std::fs::write(&path, source).expect("write native Flow.Task fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run native flux on Flow.Task fixture");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

#[test]
fn flow_task_surface_compiles_and_passes() {
    let (stdout, success) = run_flux_test("flow_task_surface.flx");
    assert!(success, "Flow.Task surface tests must pass:\n{stdout}");
    assert!(
        stdout.contains("8 passed"),
        "expected 8 passing tests, got:\n{stdout}"
    );
}

#[test]
fn flow_task_spawn_rejects_non_sendable_function_payload_cross_module() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn main() {
    Task.spawn(fn() { fn(x) { x } })
}
"#,
    );

    assert!(
        !success,
        "Task.spawn must reject a function payload through the imported Flow.Task surface"
    );
    assert!(
        stderr.contains("E444") && stderr.contains("Sendable"),
        "expected E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
fn flow_task_spawn_rejects_opaque_tcp_connection_payload() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task
import Flow.Tcp exposing (..)

fn move_conn(c: Connection) {
    Task.spawn(fn() { c })
}

fn main() { () }
"#,
    );

    assert!(
        !success,
        "Task.spawn must reject opaque TCP connection handles as non-Sendable"
    );
    assert!(
        stderr.contains("E444") && stderr.contains("Sendable"),
        "expected E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_compiles_and_passes() {
    // Uses a native-specific fixture (Int/String payloads only) because
    // List<T> and tuple assert_eq is not yet supported on the native backend.
    let path = workspace_root()
        .join("tests")
        .join("flux")
        .join("flow_task_native.flx");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test --native: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    assert!(
        output.status.success(),
        "native Task tests must pass:\n{stdout}"
    );
    assert!(
        stdout.contains("6 passed"),
        "expected 6 passing native tests, got:\n{stdout}"
    );
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_await_suspends_fiber_scheduler() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Task as Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn wait_task() -> Int with Async {
    Task.await(Task.spawn(fn() { fib(36) }))
}

fn tick() -> Int with Async {
    sleep(100)
    7
}

fn pair_body() -> (Int, Int) with Async {
    // `both` schedules `tick` on worker 1 and `wait_task` on worker 0. The
    // old native Task.await shim blocked worker 0, so the root scheduler could
    // not route the 100ms timer until the task completed.
    both(tick, wait_task)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let solo = run_async(wait_task)
    let t1 = now_ms()
    let pair = run_async(pair_body)
    let t2 = now_ms()
    print(solo)
    print(pair.0)
    print(pair.1)
    print(t1 - t0)
    print(t2 - t1)
}
"#;
    let (stdout, stderr, success) = run_flux_source_native(source, "await_overlap");
    assert!(
        success,
        "native Task.await overlap fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(&lines[..3], ["14930352", "7", "14930352"]);
    let solo_ms: i64 = lines[3].parse().expect("solo task elapsed ms");
    let both_ms: i64 = lines[4].parse().expect("both elapsed ms");
    assert!(
        solo_ms >= 80,
        "fib(36) task completed too quickly to prove overlap: {solo_ms}ms"
    );
    assert!(
        both_ms < solo_ms + 75,
        "Task.await appears to have blocked scheduler timer routing: solo={solo_ms}ms both={both_ms}ms"
    );
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_await_completed_task_returns_value() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Task as Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn await_completed_body() -> Int with Async {
    let t = Task.spawn(fn() { 42 })
    let _ = fib(30)
    Task.await(t)
}

fn main() with IO {
    let r = run_async(await_completed_body)
    print(r)
}
"#;
    let (stdout, stderr, success) = run_flux_source_native(source, "await_completed");
    assert!(
        success,
        "native Task.await completed fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn flow_task_spawn_accepts_sendable_int_payload_cross_module() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn main() {
    let _ = Task.spawn(fn() { 42 });
    ()
}
"#,
    );

    assert!(
        success,
        "Task.spawn should accept an Int payload through the imported Flow.Task surface:\n{stderr}"
    );
}
