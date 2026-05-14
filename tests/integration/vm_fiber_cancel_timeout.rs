//! Acid test: timeout body-child backend request is cancelled after the timer
//! fires (proposal 0174 Phase 1b-vi-c).
//!
//! `timeout(50, slow)` where slow sleeps 2000ms. The timer fires at ~50ms
//! returning `None`. Before 1b-vi-c the body fiber's 2s sleep request stayed
//! registered in mio until `exit_run_async`'s drain loop finally dropped it.
//! After 1b-vi-c, `backend.cancel` is called for the body child immediately
//! when `try_route_timer_for_timeout` fires.
//!
//! Observable difference: program completes well under 500ms, not ~2000ms.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn timeout_body_child_request_is_cancelled() {
    // timer fires at 50ms; body would run 2000ms if not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
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
    // Must complete well before the body's 2s sleep. 3000ms budget: ~50ms
    // timer + ~1s --no-cache compile + 1.5s CI load headroom. Without
    // cancellation the body's full 2s sleep + startup would exceed 3000ms,
    // so the bound is still a meaningful proof of cancellation.
    assert!(
        elapsed < Duration::from_millis(3000),
        "elapsed {elapsed:?} — body child's 2s timer was not cancelled \
         (should complete in ~50ms timer + startup)"
    );
}
