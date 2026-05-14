//! VM `FiberSleep` real-suspend integration test (proposal 0174 Phase 1b-vi-a).
//!
//! Drives the full `flux` CLI on a synthetic source that calls
//! `Async.sleep(50)`. Asserts the call takes roughly the requested duration —
//! lower bound proves the timer actually waited; upper bound guards against
//! the completion pump getting stuck.
//!
//! With one fiber on the VM, the `FiberSleep` primop registers a one-shot
//! mio timer and pumps `next_completion()` until it fires. The OS-thread
//! call stack is the continuation; "suspend" reduces to a blocking pump.
//! Multi-fiber suspend (where `both`/`race` actually overlap) lands in
//! 1b-vi-b alongside continuation capture.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, "sleep")
}

#[test]
fn fiber_sleep_routes_through_mio_timer_and_returns_in_range() {
    let source = r#"
import Flow.Async as Async

fn main() with Async {
    Async.sleep(50)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source);
    assert!(
        success,
        "fiber_sleep program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Lower bound: proves the timer actually waited (≥45ms allows for
    // ±5ms scheduling jitter on slower CI hosts).
    assert!(
        elapsed >= Duration::from_millis(45),
        "elapsed {elapsed:?} too short — sleep should have parked for ~50ms",
    );
    // Upper bound: guards against the pump getting stuck. The flux binary
    // also incurs startup time, so the budget is generous.
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — pump or backend lifecycle likely stuck",
    );
}

#[test]
fn fiber_sleep_zero_returns_promptly() {
    // Zero-millisecond sleep still goes through the timer service but
    // should fire immediately.  Sanity check that the pump doesn't hang
    // on a zero-deadline timer (mio timer heap should fire right away).
    let source = r#"
import Flow.Async as Async

fn main() with Async {
    Async.sleep(0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source);
    assert!(
        success,
        "fiber_sleep(0) program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn forked_child_sleep_does_not_corrupt_parent_locals_across_helper_sleep() {
    let source = r#"
import Flow.Async exposing (..)

data Handle { Handle(Int) }

fn child() -> Unit with Async {
    sleep(20)
}

fn delay() -> Unit with Async {
    sleep(10)
}

fn body() -> Int with Async {
    let h = Handle(123)
    let s = new_scope()
    let _f = fork(s, child)
    let _d = delay()
    match h {
        Handle(id) -> id
    }
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source);
    assert!(
        success,
        "parent locals must survive helper sleep with a forked child:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "123", "unexpected stdout:\n{stdout}");
}
