//! Acid test: timeout body-child backend request is cancelled after the timer
//! fires (proposal 0174 Phase 1b-vi-c).
//!
//! `timeout(50, slow)` where slow sleeps 2000ms. The timer fires at ~50ms
//! returning `None`. Before 1b-vi-c the body fiber's 2s sleep request stayed
//! registered in mio until `exit_run_async`'s drain loop finally dropped it.
//! After 1b-vi-c, `backend.cancel` is called for the body child immediately
//! when `try_route_timer_for_timeout` fires.
//!
//! Observable difference: with the body child cancelled the program completes
//! in ~50ms of timer + startup; otherwise it blocks until the body's timer
//! expires.
//!
//! De-flake: the body sleeps a large fixed amount (30s)
//! and we assert completion well under it (8s), a wide-gap deadlock guard that
//! is not load-sensitive. See vm_fiber_cancel_loser.rs and
//! docs/internals/concurrency_model.md §1 for the full rationale.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn timeout_body_child_request_is_cancelled() {
    // timer fires at 50ms; body would block 30s if its timer is not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(30000)
    99
}

fn body() -> String with Async {
    let result = timeout(50, slow)
    match result {
        None -> "timed_out",
        Some(_) -> "completed"
    }
}

fn main() with IO {
    let s = run_async(body)
    print(s)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_cancel");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("timed_out"),
        "expected 'timed_out' output:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard: working run finishes in compile + ~50ms timer
    // (well under 8s on any CI load); a regressed run blocks on the body's 30s
    // timer and trips this.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — body child's 30s timer was not cancelled \
         (should complete in ~50ms timer + startup)"
    );
}
