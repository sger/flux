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
//! VM and native task execution both use worker threads. `Task.await` parks
//! only the calling fiber while task workers publish completions back into
//! the async scheduler.

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

#[cfg(feature = "llvm")]
fn generated_native_many_task_source(count: usize, mode: &str) -> String {
    let mut source = String::from("import Flow.Async exposing (..)\nimport Flow.Task as Task\n\n");
    source.push_str(&format!("fn task_count() -> Int {{ {count} }}\n\n"));
    match mode {
        "join" => {
            source.push_str(
                r#"
fn spawn_many(n, acc) {
    if n <= 0 { acc } else { spawn_many(n - 1, [Task.spawn(fn() { 1 }) | acc]) }
}

fn join_all(tasks, acc) {
    match tasks {
        [h | t] -> join_all(t, acc + Task.blocking_join(h)),
        _ -> acc
    }
}

fn main() with IO {
    let tasks = spawn_many(task_count(), [])
    print(join_all(tasks, 0))
}
"#,
            );
        }
        "await" => {
            source.push_str(
                r#"
fn spawn_many(n, acc) {
    if n <= 0 { acc } else { spawn_many(n - 1, [Task.spawn(fn() { 1 }) | acc]) }
}

fn join_all(tasks, acc) {
    match tasks {
        [h | t] -> join_all(t, acc + Task.blocking_join(h)),
        _ -> acc
    }
}

fn await_one_then_join_rest(tasks) with Async {
    match tasks {
        [h | t] -> Task.await(h) + join_all(t, 0),
        _ -> 0
    }
}

fn body() -> Int with Async {
    await_one_then_join_rest(spawn_many(task_count(), []))
}

fn main() with IO {
    print(run_async_with_workers(4, body))
}
"#,
            );
        }
        "cancel" => {
            source.push_str(
                r#"
fn spawn_many(n, acc) {
    if n <= 0 { acc } else { spawn_many(n - 1, [Task.spawn(fn() { 1 }) | acc]) }
}

fn cancel_all(tasks) {
    match tasks {
        [h | t] -> do {
            Task.cancel(h);
            cancel_all(t)
        },
        _ -> ()
    }
}

fn expect_cancelled(tasks) {
    match tasks {
        [h | t] -> do {
            assert_throws(fn() { Task.blocking_join(h) });
            expect_cancelled(t)
        },
        _ -> ()
    }
}

fn main() {
    let tasks = spawn_many(task_count(), [])
    cancel_all(tasks);
    expect_cancelled(tasks)
}
"#,
            );
        }
        other => panic!("unknown many-task source mode: {other}"),
    }
    source
}

#[test]
fn flow_task_surface_compiles_and_passes() {
    let (stdout, success) = run_flux_test("flow_task_surface.flx");
    assert!(success, "Flow.Task surface tests must pass:\n{stdout}");
    assert!(
        stdout.contains("12 passed"),
        "expected 12 passing tests, got:\n{stdout}"
    );
}

#[test]
fn flow_task_vm_spawn_runs_cpu_jobs_in_parallel() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let solo = fib(36)
    let t1 = now_ms()
    let a = Task.spawn(fn() { fib(36) })
    let b = Task.spawn(fn() { fib(36) })
    let ra = Task.blocking_join(a)
    let rb = Task.blocking_join(b)
    let t2 = now_ms()
    print(solo)
    print(ra)
    print(rb)
    print(t1 - t0)
    print(t2 - t1)
}
"#,
    );

    assert!(
        success,
        "VM Task.spawn parallel fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(&lines[..3], ["14930352", "14930352", "14930352"]);
    let solo_ms: i64 = lines[3].parse().expect("solo task elapsed ms");
    let pair_ms: i64 = lines[4].parse().expect("pair task elapsed ms");
    assert!(
        solo_ms >= 20,
        "fib(36) completed too quickly to prove VM task overlap: {solo_ms}ms"
    );
    assert!(
        pair_ms < solo_ms * 2 - 10,
        "VM Task.spawn appears sequential: solo={solo_ms}ms pair={pair_ms}ms"
    );
}

#[test]
fn flow_task_vm_await_does_not_block_fiber_scheduler() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
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
"#,
    );

    assert!(
        success,
        "VM Task.await overlap fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(&lines[..3], ["14930352", "7", "14930352"]);
    let solo_ms: i64 = lines[3].parse().expect("solo task elapsed ms");
    let both_ms: i64 = lines[4].parse().expect("both elapsed ms");
    assert!(
        solo_ms >= 20,
        "fib(36) completed too quickly to prove VM await overlap: {solo_ms}ms"
    );
    assert!(
        both_ms < solo_ms + 1000,
        "VM Task.await appears to block scheduler timer routing: solo={solo_ms}ms both={both_ms}ms"
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
fn flow_task_spawn_accepts_sendable_closure_captures_cross_module() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

data Packet {
    Packet(Int, String),
}

fn main() with IO {
    let offset = 40
    let label = "ok"
    let nums = [1, 2, 3]
    let packet = Packet(9, label)
    let t = Task.spawn(fn() { (offset + 2, label, nums, packet) })
    let r = Task.blocking_join(t)
    print(r.0)
    print(r.1)
}
"#,
    );

    assert!(
        success,
        "Task.spawn should accept Sendable captured values:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.lines().collect::<Vec<_>>(), ["42", "\"ok\""]);
}

#[test]
fn flow_task_spawn_accepts_exposed_import_closure_capture() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task exposing (spawn, blocking_join)

fn main() with IO {
    let base = 41
    let t = spawn(fn() { base + 1 })
    print(blocking_join(t))
}
"#,
    );

    assert!(
        success,
        "exposed Flow.Task.spawn should accept a Sendable captured value:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn flow_task_spawn_rejects_non_sendable_capture_even_with_sendable_result() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task
import Flow.Tcp exposing (..)

fn move_conn(c: Connection) {
    Task.spawn(fn() {
        let _captured = c
        1
    })
}

fn main() { () }
"#,
    );

    assert!(
        !success,
        "Task.spawn must reject non-Sendable captured values even when the result is Sendable"
    );
    assert!(
        stderr.contains("E444")
            && stderr.contains("Task.spawn closure captures non-Sendable value")
            && stderr.contains("c:"),
        "expected capture-specific E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
fn flow_task_spawn_rejects_bogus_manual_sendable_instance() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

data WithFn {
    WithFn(Int -> Int),
}

instance Sendable<WithFn> {}

fn main() {
    let f = fn(x) { x + 1 }
    let boxed = WithFn(f)
    Task.spawn(fn() {
        let _captured = boxed
        1
    })
}
"#,
    );

    assert!(
        !success,
        "manual Sendable instances must not allow unsafe Task.spawn captures"
    );
    assert!(
        stderr.contains("E453") && stderr.contains("Sendable"),
        "expected sealed Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
fn flow_task_spawn_rejects_captured_local_function_value() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn main() {
    let f = fn(x) { x + 1 }
    Task.spawn(fn() { f(41) })
}
"#,
    );

    assert!(
        !success,
        "Task.spawn must reject captured local function values"
    );
    assert!(
        stderr.contains("E444")
            && stderr.contains("Task.spawn closure captures non-Sendable value")
            && stderr.contains("f:"),
        "expected capture-specific E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
fn non_task_spawn_member_does_not_get_task_capture_constraints() {
    let dir = std::env::temp_dir().join(format!(
        "flux-flow-task-non-task-spawn-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for non-task spawn fixture");
    let other_path = dir.join("Other.flx");
    let main_path = dir.join("main.flx");
    std::fs::write(
        &other_path,
        r#"
module Other {
    public fn spawn(action: () -> Int) -> Int {
        action()
    }
}
"#,
    )
    .expect("write Other.flx fixture");
    std::fs::write(
        &main_path,
        r#"
import Other

fn main() with IO {
    let f = fn(x) { x + 1 }
    print(Other.spawn(fn() { f(41) }))
}
"#,
    )
    .expect("write main.flx fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([main_path.to_str().unwrap(), "--no-cache", "--dump-cfg"])
        .output()
        .expect("run non-task spawn fixture");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&other_path);
    let _ = std::fs::remove_file(&main_path);

    assert!(
        output.status.success(),
        "non-Flow.Task spawn members must not get Task.spawn capture constraints:\nstdout:\n{stdout}\nstderr:\n{stderr}"
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
        stdout.contains("8 passed"),
        "expected 8 passing native tests, got:\n{stdout}"
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
#[cfg(feature = "llvm")]
fn flow_task_native_registry_grows_past_old_slot_limit_for_blocking_join() {
    let count = 1025;
    let source = generated_native_many_task_source(count, "join");
    let (stdout, stderr, success) = run_flux_source_native(&source, "many_join");
    assert!(
        success,
        "native Task registry must grow past old slot limit for blocking_join:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), count.to_string());
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_registry_grows_past_old_slot_limit_for_await() {
    let count = 1025;
    let source = generated_native_many_task_source(count, "await");
    let (stdout, stderr, success) = run_flux_source_native(&source, "many_await");
    assert!(
        success,
        "native Task registry must grow past old slot limit for Task.await:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), count.to_string());
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_cancel_above_old_slot_limit_remains_join_observable() {
    let count = 1025;
    let source = generated_native_many_task_source(count, "cancel");
    let (stdout, stderr, success) = run_flux_source_native(&source, "many_cancel");
    assert!(
        success,
        "native Task cancel must work past old slot limit:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
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
