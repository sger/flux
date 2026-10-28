//! VM `Async.timeout` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2 follow-up).
//!
//! `timeout(ms, f)` returns `Some(f())` if `f` completes within `ms`,
//! otherwise `None`. Both branches run concurrently via the fiber
//! scheduler — the body fiber and a backend timer race for the same
//! request id; whichever fires first wins.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

#[test]
fn timeout_fires_when_body_too_slow() {
    // Body would take 1000ms; timeout cuts it off at 50ms. Should return
    // None (printed as -1) and finish ~50ms after run_async start (well
    // before slow's 1000ms would complete).
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(1000)
    99
}

fn body() -> Option<Int> with Async {
    timeout(50, slow)
}

fn main() with IO, Time {
    let t0 = now_ms()
    let r = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "timeout_fires");
    assert!(success, "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("-1"), "expected None (-1) in output:\nstdout:\n{stdout}");
    // Timer must fire well before slow's 1000ms: ~50ms timer, against
    // ~1000ms on the non-firing path.
    assert!(
        elapsed < Duration::from_millis(700),
        "elapsed {elapsed:?} — timeout didn't return on timer (slow's full \
         1000ms would be ~1000ms; the timer should be ~50ms)"
    );
}

#[test]
fn timeout_returns_some_when_body_in_time() {
    // Body completes in 50ms; timeout is 1000ms. Should return Some(42).
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Option<Int> with Async {
    timeout(1000, fast)
}

fn main() with IO, Time {
    let t0 = now_ms()
    let r = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "timeout_succeeds");
    assert!(success, "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    assert!(stdout.contains("42"), "expected Some(42) result in output:\nstdout:\n{stdout}");
    // Body must wake parent before timeout fires (~50ms vs 1000ms).
    assert!(
        elapsed < Duration::from_millis(700),
        "elapsed {elapsed:?} — body should resume parent on completion (~50ms), \
         not wait for timeout (~1000ms)"
    );
}
