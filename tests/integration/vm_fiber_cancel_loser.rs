//! Acid test: race-loser backend request is cancelled immediately after the
//! winner is determined (proposal 0174 Phase 1b-vi-c).
//!
//! `race(fast, slow)` where fast sleeps 50ms and slow sleeps 2000ms.
//! Before 1b-vi-c the slow fiber's 2s timer was abandoned and kept running
//! until `run_async` returned — total wall-clock was ~50ms of sleep but the
//! mio reactor timer lingered. After 1b-vi-c, `backend.cancel` is called for
//! the loser immediately after the winner resolves, and `exit_run_async`
//! cancels any remaining outstanding requests.
//!
//! The observable difference: the program should complete in well under 500ms
//! total (fast sleep 50ms + startup), not approach 2000ms.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn race_loser_backend_request_is_cancelled() {
    // fast wins in ~50ms; slow would take 2000ms if not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn slow() -> Int with Async {
    let _ = sleep(2000)
    0
}

fn body() -> Int with Async {
    race(fast, slow)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "race_loser");
    assert!(
        success,
        "race program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected fast branch result 42:\nstdout:\n{stdout}"
    );
    // Must complete well before the loser's 2s timer. Allow 1800ms for
    // `--no-cache` startup + fast sleep; if the loser is NOT cancelled the
    // program takes ~2000ms of sleep + startup (~3s total).
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — loser's 2s timer was not cancelled \
         (should complete in ~50ms sleep + startup)"
    );
}
