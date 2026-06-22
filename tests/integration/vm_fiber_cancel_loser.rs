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
//! The observable difference: with cancellation working the program completes
//! in ~50ms of sleep + startup; with the loser's timer left running it blocks
//! until that timer expires.
//!
//! De-flake note: the loser sleeps a *large* fixed amount
//! (30s) and we assert completion well under it (8s). This widens the
//! signal-to-noise gap so the threshold is no longer load-sensitive — a working
//! run finishes in hundreds of ms (compile + ~50ms sleep) regardless of CI
//! load, while a regressed run blocks ~30s and trips the guard unmistakably.
//! The earlier `< 1800ms` margin had only ~100ms of headroom over a slow
//! `--no-cache` startup and flaked under load. A fully time-free version awaits
//! the virtual-time scheduler backend (T1.1 follow-up); until then this is a
//! robust deadlock guard rather than a timing race. See
//! docs/internals/concurrency_model.md §1.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn race_loser_backend_request_is_cancelled() {
    // fast wins in ~50ms; slow would block 30s if its timer is not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn slow() -> Int with Async {
    let _ = sleep(30000)
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
    // Deadlock guard with a wide gap: a working run finishes in compile +
    // ~50ms sleep (well under 8s on any CI load); a regressed run blocks on the
    // loser's 30s timer and trips this. See the de-flake note above.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — loser's 30s timer was not cancelled \
         (should complete in ~50ms sleep + startup)"
    );
}
