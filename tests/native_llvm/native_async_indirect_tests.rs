//! Native async indirect closure propagation tests.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-native-async-indirect-{}-{}",
        std::process::id(),
        tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let start = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run flux native");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn native_async_function_parameter_call_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn call_it(f) -> Int with Async {
    f()
}

fn sleeper() -> Int with Async {
    sleep(10)
    21
}

fn body() -> Int with Async {
    call_it(sleeper)
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "param_call");
    assert!(
        success,
        "native async function parameter call must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "21");
}

#[test]
fn native_sequential_async_function_parameter_calls_resume_each_time() {
    let source = r#"
import Flow.Async exposing (..)

fn call_it(f) -> Int with Async {
    f()
}

fn sleeper() -> Int with Async {
    sleep(10)
    4
}

fn body() -> Int with Async {
    let a = call_it(sleeper)
    let b = call_it(sleeper)
    let c = call_it(sleeper)
    a + b + c
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "param_seq");
    assert!(
        success,
        "native sequential async parameter calls must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "12");
}

#[test]
fn native_async_closure_value_with_capture_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn call_it(f) -> Int with Async {
    f()
}

fn body() -> Int with Async {
    let base = 30
    let f = fn() {
        sleep(10)
        base + 2
    }
    call_it(f)
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "capture");
    assert!(
        success,
        "native async captured closure call must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "32");
}

#[test]
fn native_nested_indirect_async_chain_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn inner(f) -> Int with Async {
    f()
}

fn wrapper(f) -> Int with Async {
    inner(f)
}

fn sleeper() -> Int with Async {
    sleep(10)
    6
}

fn body() -> Int with Async {
    wrapper(sleeper) + 1
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "nested_chain");
    assert!(
        success,
        "native nested indirect async chain must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn native_async_try_suspended_body_returns_ok() {
    let source = r#"
import Flow.Async exposing (..)

fn body() with Async {
    try_(fn() {
        sleep(10)
        42
    })
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "try_body");
    assert!(
        success,
        "native async try_ body must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "Ok(42)");
}

#[test]
fn native_async_try_catches_panic_and_worker_reuses_afterward() {
    let source = r#"
import Flow.Async exposing (..)

fn boom() -> Int with Async {
    panic("boom")
}

fn ok() -> Int with Async {
    sleep(10)
    7
}

fn main() with IO {
    print(run_async(fn() { try_(boom) }))
    print(run_async(fn() { try_(ok) }))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "try_panic_reuse");
    assert!(
        success,
        "native async try_ panic must be caught and worker reused:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "Err(Panicked(\"boom\"))\nOk(7)");
}

#[test]
fn native_async_try_catches_both_child_panic() {
    let source = r#"
import Flow.Async exposing (..)

fn boom() -> Int with Async {
    panic("boom")
}

fn slow() -> Int with Async {
    sleep(50)
    1
}

fn body() with Async {
    try_(fn() { both(boom, slow) })
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "try_both_panic");
    assert!(
        success,
        "native async try_ must catch child panic from both:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "Err(Panicked(\"boom\"))");
}

#[test]
fn native_async_finally_suspended_cleanup_runs() {
    let source = r#"
import Flow.Async exposing (..)

fn body_value() -> Int { 9 }

fn cleanup() -> Unit with Async {
    sleep(10)
}

fn body() -> Int with Async {
    let v = finally(body_value, cleanup)
    v + 1
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "finally_cleanup");
    assert!(
        success,
        "native async finally cleanup must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "10");
}

#[test]
fn native_async_bracket_suspended_body_returns_value() {
    let source = r#"
import Flow.Async exposing (..)

fn acquire() -> Int { 5 }
fn release(_r) -> Int { 0 }

fn use_resource(r: Int) -> Int with Async {
    sleep(10)
    r + 8
}

fn body() -> Int with Async {
    bracket(acquire, release, use_resource)
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "bracket_body");
    assert!(
        success,
        "native async bracket body must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "13");
}

#[test]
fn native_async_scope_suspended_body_resumes() {
    let source = r#"
import Flow.Async exposing (..)

fn scoped(_s) -> Int with Async {
    sleep(10)
    77
}

fn body() -> Int with Async {
    scope(scoped)
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "scope_body");
    assert!(
        success,
        "native async scope body must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "77");
}

#[test]
fn native_async_fork_scoped_does_not_block_parent() {
    let source = r#"
import Flow.Async exposing (..)

fn child() -> Int with Async {
    sleep(1000)
    99
}

fn scoped(s) -> Int with Async {
    fork(s, child)
    sleep(50)
    5
}

fn body() -> Int with Async {
    scope(scoped)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let v = run_async(body)
    let t1 = now_ms()
    print(v)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "fork_nonblocking");
    assert!(
        success,
        "native async scoped fork must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "5");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "fork waited for scoped child instead of returning parent result: {measured_ms}ms"
    );
}

#[test]
fn native_async_cancel_scope_drops_pending_child() {
    let source = r#"
import Flow.Async exposing (..)

fn child() -> Int with Async {
    sleep(1000)
    99
}

fn scoped(s) -> Int with Async {
    fork(s, child)
    sleep(20)
    cancel(s)
    sleep(20)
    11
}

fn body() -> Int with Async {
    scope(scoped)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let v = run_async(body)
    let t1 = now_ms()
    print(v)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "cancel_scope");
    assert!(
        success,
        "native async cancel scope must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "11");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "cancelled scoped child kept the run_async boundary alive: {measured_ms}ms"
    );
}

#[test]
fn native_async_cancel_scope_suppresses_child_at_next_suspend() {
    let source = r#"
import Flow.Async exposing (..)

fn child() -> Int with Async {
    let pair = (1, 2)
    let _ = pair.0 + pair.1
    sleep(1000)
    99
}

fn scoped(s) -> Int with Async {
    fork(s, child)
    cancel(s)
    sleep(20)
    11
}

fn body() -> Int with Async {
    scope(scoped)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let v = run_async(body)
    let t1 = now_ms()
    print(v)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "cancel_scope_running");
    assert!(
        success,
        "native async running scoped child cancellation must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "11");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "cancelled running scoped child parked and kept run_async alive: {measured_ms}ms"
    );
}
