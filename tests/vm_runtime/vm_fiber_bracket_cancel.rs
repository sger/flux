//! Acid test: cancelled fibers leave the run cleanly (proposal 0174 Phase
//! 1b-vi-c). Since 0188 S9d a cancelled fiber is dropped and never runs
//! again, on the VM as natively.
//!
//! A slow fiber races a fast fiber.  After the fast fiber wins, the slow
//! fiber is cancelled and dropped.
//!
//! We verify:
//! 1. The program succeeds (no runtime error from the cancelled fiber).
//! 2. The fast result is returned correctly.
//! 3. Elapsed time is well below the slow fiber's 2s sleep (cancelled, not
//!    waited for), proving `exit_run_async` doesn't hang on cancelled fibers.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, tag)
}

#[test]
fn cancelled_fiber_leaves_cleanly() {
    // fast wins in ~50ms; slow sleeps 2000ms and is cancelled.
    // The slow fiber is dropped — no runtime panic, correct result returned.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
    0
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
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
    let (stdout, stderr, success, _) = run_source(source, "cancelled_clean");
    assert!(success, "cancelled-fiber program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("42"), "expected fast result 42:\nstdout:\n{stdout}");
    // Must complete well before the slow fiber's 2s sleep.
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — slow fiber not cancelled"
    );
}

#[test]
fn both_cancelled_fibers_leave_cleanly() {
    // timeout(50, slow): the timer fires at ~50ms, body child is cancelled.
    // The cancelled body is dropped — must not crash.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
    99
}

fn body() -> String with Async {
    let r = timeout(50, slow)
    match r {
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
    let (stdout, stderr, success, _) = run_source(source, "timeout_clean");
    assert!(
        success,
        "timeout-cancelled program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("timed_out"), "expected 'timed_out':\nstdout:\n{stdout}");
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — body child not cancelled"
    );
}
