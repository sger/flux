//! Acid test: race-loser backend request is cancelled immediately after the
//! winner is determined (proposal 0174 Phase 1b-vi-c).
//!
//! `race(fast, slow)` where fast sleeps 50ms and slow sleeps 10s.
//! Before 1b-vi-c the slow fiber's timer was abandoned and kept running
//! until `run_async` returned — total wall-clock was ~50ms of sleep but the
//! mio reactor timer lingered. After 1b-vi-c, `backend.cancel` is called for
//! the loser immediately after the winner resolves, and `exit_run_async`
//! cancels any remaining outstanding requests.
//!
//! The observable difference: the program returns before slow's 10s sleep
//! could end; waiting for it would take at least that long.

use crate::support::flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn race_loser_backend_request_is_cancelled() {
    // fast wins in ~50ms; slow would take 10s if not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn slow() -> Int with Async {
    let _ = sleep(10000)
    0
}

fn body() -> Int with Async {
    race(fast, slow)
}

fn main() with IO, Time {
    let t0 = now_ms()
    let result = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    print(result)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "race_loser");
    assert!(success, "race program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("42"), "expected fast branch result 42:\nstdout:\n{stdout}");
    // Waiting for the loser takes at least its 10s sleep.
    assert!(
        elapsed < Duration::from_millis(10_000),
        "elapsed {elapsed:?} — loser's 10s timer was not cancelled \
         (should complete in ~50ms of sleep)"
    );
}
