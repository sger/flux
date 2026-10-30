//! Acid test: timeout body-child backend request is cancelled after the timer
//! fires (proposal 0174 Phase 1b-vi-c).
//!
//! `timeout(50, slow)` where slow sleeps 10s. The timer fires at ~50ms
//! returning `None`. Before 1b-vi-c the body fiber's 2s sleep request stayed
//! registered in mio until `exit_run_async`'s drain loop finally dropped it.
//! After 1b-vi-c, `backend.cancel` is called for the body child immediately
//! when `try_route_timer_for_timeout` fires.
//!
//! Observable difference: the program returns before slow's 10s sleep could
//! end; waiting for it would take at least that long.

use crate::support::flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn timeout_body_child_request_is_cancelled() {
    // timer fires at 50ms; body would run 10s if not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(10000)
    99
}

fn body() -> String with Async {
    let result = timeout(50, slow)
    match result {
        None -> "timed_out",
        Some(_) -> "completed"
    }
}

fn main() with IO, Time {
    let t0 = now_ms()
    let s = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    print(s)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "timeout_cancel");
    assert!(success, "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("timed_out"), "expected 'timed_out' output:\nstdout:\n{stdout}");
    // Waiting for the body takes at least its 10s sleep.
    assert!(
        elapsed < Duration::from_millis(10_000),
        "elapsed {elapsed:?} — body child's 10s timer was not cancelled \
         (should complete in ~50ms timer)"
    );
}
