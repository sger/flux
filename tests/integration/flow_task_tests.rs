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

#[cfg(feature = "llvm")]
use std::time::Duration;

use crate::support::flux_runner;
use flux_runner::flux_source;
use flux_runner::scratch::Scratch;
use flux_runner::workspace_root;
use std::process::Command;

// Every run uses four workers, so the task tests see real parallelism even
// on a small machine.
const WORKERS: (&str, &str) = ("FLUX_WORKERS", "4");

fn run_flux_test(fixture: &str) -> (String, bool) {
    flux_runner::run_flux_test_with(fixture, &[], &[WORKERS])
}

fn run_flux_source(source: &str) -> (String, String, bool) {
    let ran = flux_source(source, "flow-task").envs(&[WORKERS]).run();
    (ran.stdout, ran.stderr, ran.success)
}

#[cfg(feature = "llvm")]
fn run_flux_source_native(source: &str, tag: &str) -> (String, String, bool) {
    let ran = flux_source(source, tag).native().envs(&[WORKERS]).run();
    (ran.stdout, ran.stderr, ran.success)
}

// Kills the run after `timeout`, to catch deadlocks such as a rendezvous
// channel that never unblocks.
#[cfg(feature = "llvm")]
fn run_flux_source_native_with_timeout(
    source: &str,
    tag: &str,
    timeout: Duration,
) -> (String, String, bool) {
    let ran = flux_source(source, tag).native().envs(&[WORKERS]).timeout(timeout).run();
    (ran.stdout, ran.stderr, ran.success)
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
    assert!(stdout.contains("12 passed"), "expected 12 passing tests, got:\n{stdout}");
}

#[test]
fn flow_task_vm_spawn_runs_cpu_jobs_in_parallel() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() with IO {
    let a = Task.spawn(fn() { fib(36) })
    let b = Task.spawn(fn() { fib(36) })
    let ra = Task.blocking_join(a)
    let rb = Task.blocking_join(b)
    print(ra)
    print(rb)
}
"#,
    );

    assert!(
        success,
        "VM Task.spawn parallel fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["14930352", "14930352"],
        "expected both fib(36) results:\nstdout:\n{stdout}"
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

fn main() with IO {
    let solo = run_async(wait_task)
    let pair = run_async(pair_body)
    print(solo)
    print(pair.0)
    print(pair.1)
}
"#,
    );

    assert!(
        success,
        "VM Task.await overlap fixture must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["14930352", "7", "14930352"],
        "expected solo fib result, tick result, and paired fib result:\nstdout:\n{stdout}"
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

    assert!(!success, "Task.spawn must reject opaque TCP connection handles as non-Sendable");
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
fn flow_task_vm_many_small_tasks_complete() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

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
    let tasks = spawn_many(512, [])
    print(join_all(tasks, 0))
}
"#,
    );

    assert!(
        success,
        "VM Task.spawn should handle many small tasks:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "512");
}

#[test]
fn flow_task_vm_many_tasks_call_shared_helper_function() {
    let (stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn bump(x: Int) -> Int { x + 1 }

fn spawn_many(n, acc) {
    if n <= 0 { acc } else { spawn_many(n - 1, [Task.spawn(fn() { bump(1) }) | acc]) }
}

fn join_all(tasks, acc) {
    match tasks {
        [h | t] -> join_all(t, acc + Task.blocking_join(h)),
        _ -> acc
    }
}

fn main() with IO {
    let tasks = spawn_many(256, [])
    print(join_all(tasks, 0))
}
"#,
    );

    assert!(
        success,
        "VM Task.spawn should reuse shared function constants safely:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "512");
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

    assert!(!success, "manual Sendable instances must not allow unsafe Task.spawn captures");
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

    assert!(!success, "Task.spawn must reject captured local function values");
    assert!(
        stderr.contains("E444")
            && stderr.contains("Task.spawn closure captures non-Sendable value")
            && stderr.contains("f:"),
        "expected capture-specific E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
fn non_task_spawn_member_does_not_get_task_capture_constraints() {
    let scratch = Scratch::new("non-task-spawn");
    scratch.write(
        "Other.flx",
        r#"
module Other {
    public fn spawn(action: () -> Int) -> Int {
        action()
    }
}
"#,
    );
    let main_path = scratch.write(
        "main.flx",
        r#"
import Other

fn main() with IO {
    let f = fn(x) { x + 1 }
    print(Other.spawn(fn() { f(41) }))
}
"#,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([main_path.to_str().unwrap(), "--no-cache", "--dump-cfg"])
        .args(scratch.cache_args())
        .output()
        .expect("run non-task spawn fixture");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");

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
    let (stdout, success) =
        flux_runner::run_flux_test_with("flow_task_native.flx", &["--native"], &[]);
    assert!(success, "native Task tests must pass:\n{stdout}");
    assert!(stdout.contains("8 passed"), "expected 8 passing native tests, got:\n{stdout}");
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_await_suspends_fiber_scheduler() {
    // Scheduler non-blocking proof via concurrent Task.await.
    //
    // `both` runs `counter` and `waiter` concurrently. `waiter` suspends on
    // Task.await while a background OS thread computes fib(36). If Task.await
    // ever blocks the scheduler thread, `counter` cannot run either and the
    // whole program deadlocks. The 30-second process timeout detects this.
    // No wall-clock timing ratios are involved.
    let source = r#"
import Flow.Async exposing (..)
import Flow.Task as Task

fn fib(n: Int) -> Int {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn waiter() -> Int with Async {
    Task.await(Task.spawn(fn() { fib(36) }))
}

fn counter() -> Int with Async { 1 }

fn pair_body() -> (Int, Int) with Async {
    both(counter, waiter)
}

fn main() with IO {
    let pair = run_async_with_workers(4, pair_body)
    print(pair.0)
    print(pair.1)
}
"#;
    // 30-second budget: fib(36) is ~100-300ms even on slow CI; only a genuine
    // scheduler deadlock would approach this ceiling.
    let (stdout, stderr, success) =
        run_flux_source_native_with_timeout(source, "await_overlap", Duration::from_secs(30));
    assert!(
        success,
        "Task.await blocked the scheduler (deadlock or process error):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some("1"),
        "counter fiber result mismatch: stdout:\n{stdout}"
    );
    assert_eq!(
        lines.get(1).copied(),
        Some("14930352"),
        "fib(36) result mismatch: stdout:\n{stdout}"
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
