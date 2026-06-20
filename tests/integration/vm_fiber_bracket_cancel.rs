//! Acid test: cancelled fibers are resumed with `AsyncError.Canceled` rather
//! than being silently dropped (proposal 0174 Phase 1b-vi-c).
//!
//! A slow fiber races a fast fiber.  After the fast fiber wins, the slow
//! fiber is cancelled: its continuation is resumed with `Canceled` so that
//! any `bracket`/`finally` cleanup arms execute before the fiber exits.
//!
//! We verify:
//! 1. The program succeeds (no runtime error from the cancelled fiber).
//! 2. The fast result is returned correctly.
//! 3. Elapsed time is well below the slow fiber's 2s sleep (cancelled, not
//!    waited for), proving `exit_run_async` doesn't hang on cancelled fibers.
//!
//! De-flake: the slow fiber sleeps a large fixed amount
//! (30s) and we assert completion well under it (8s), a wide-gap deadlock
//! guard that is not load-sensitive. See vm_fiber_cancel_loser.rs and
//! docs/internal/concurrency_model.md §1 for the full rationale.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn cancelled_fiber_resumes_cleanly() {
    // fast wins in ~50ms; slow sleeps 30s and is cancelled.
    // After 1b-vi-c, the slow fiber's continuation is resumed with Canceled
    // and the fiber exits cleanly — no runtime panic, correct result returned.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(30000)
    0
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Int with Async {
    race(fast, slow)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "cancelled_clean");
    assert!(
        success,
        "cancelled-fiber program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected fast result 42:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard (see header): regressed run blocks on slow's 30s sleep.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — slow fiber not cancelled"
    );
}

#[test]
fn both_cancelled_fibers_resume_cleanly() {
    // timeout(50, slow): the timer fires at ~50ms, body child is cancelled.
    // The cancelled body continuation is resumed with Canceled — must not crash.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(30000)
    99
}

fn body() -> String with Async {
    let r = timeout(50, slow)
    match r {
        None -> "timed_out",
        Some(_) -> "completed"
    }
}

fn main() with IO {
    let s = run_async(body)
    print(s)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_clean");
    assert!(
        success,
        "timeout-cancelled program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("timed_out"),
        "expected 'timed_out':\nstdout:\n{stdout}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — body child not cancelled"
    );
}
