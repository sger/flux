//! Acid tests: real `scope`/`fork`/`cancel`.
//!
//! `scope` now allocates a real cancellation boundary; `fork` registers child
//! fibers under it; `cancel` stops them immediately by cancelling their backend
//! requests and resuming their continuations with `AsyncError.Canceled`.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn scope_cancel_stops_forked_fibers() {
    // Fork two slow sleeps inside a scope; cancel after 100ms.
    // Without cancellation this would block 30s; with it, ~100ms.
    let source = r#"
import Flow.Async exposing (..)

fn slow_work() -> Int with Async {
    let _ = sleep(30000)
    0
}

fn scoped_body(s) -> Int with Async {
    let _f1 = fork(s, slow_work)
    let _f2 = fork(s, slow_work)
    let _w = sleep(100)
    let _c = cancel(s)
    42
}

fn body() -> Int with Async {
    scope(scoped_body)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "scope_cancel");
    assert!(
        success,
        "scope-cancel program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected result 42:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard (proposal 0177 T1.4): a working run finishes in
    // compile + ~100ms cancel delay; a regressed run blocks on the forks' 30s
    // sleeps and trips this. Not load-sensitive. See vm_fiber_cancel_loser.rs
    // and docs/internals/concurrency_model.md §1.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — scope cancel did not stop forked fibers"
    );
}

#[test]
fn fork_runs_body() {
    // fork spawns a child fiber and runs its body; verify result is correct.
    let source = r#"
import Flow.Async exposing (..)

fn child_work() -> Int with Async {
    let _ = sleep(10)
    0
}

fn scoped_body(s) -> Int with Async {
    let _f = fork(s, child_work)
    99
}

fn body() -> Int with Async {
    scope(scoped_body)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "fork_runs");
    assert!(
        success,
        "fork program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("99"),
        "expected result 99:\nstdout:\n{stdout}"
    );
}
