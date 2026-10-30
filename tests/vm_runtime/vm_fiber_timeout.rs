//! VM `Async.timeout` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2 follow-up).
//!
//! `timeout(ms, f)` returns `Some(f())` if `f` completes within `ms`,
//! otherwise `None`. Both branches run concurrently via the fiber
//! scheduler — the body fiber and a backend timer race for the same
//! request id; whichever fires first wins.

use crate::support::flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

/// The scale that stretches the tests' hang limits on slow machines.
#[test]
fn the_test_timeout_scale_is_one_unless_set_to_a_positive_number() {
    let scale = flux_runner::timeout_scale;
    assert!((scale(None) - 1.0).abs() < f64::EPSILON);
    assert!((scale(Some("2")) - 2.0).abs() < f64::EPSILON);
    assert!((scale(Some(" 1.5 ")) - 1.5).abs() < f64::EPSILON);
    for bad in ["", "fast", "0", "-2", "NaN", "inf"] {
        assert!((scale(Some(bad)) - 1.0).abs() < f64::EPSILON, "{bad:?}");
    }
}

#[test]
fn timeout_fires_when_body_too_slow() {
    // Body would take 10s; timeout cuts it off at 50ms. Should return
    // None (printed as -1) long before slow's sleep could end.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(10000)
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
    // Waiting for slow takes at least its 10s sleep, so any earlier return
    // came from the timer, however loaded the machine.
    assert!(
        elapsed < Duration::from_millis(10_000),
        "elapsed {elapsed:?} — timeout waited for slow's 10s sleep instead of \
         returning on its 50ms timer"
    );
}

#[test]
fn timeout_returns_some_when_body_in_time() {
    // Body completes in 50ms; timeout is 10s. Should return Some(42).
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Option<Int> with Async {
    timeout(10000, fast)
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
    // Waiting for the timer takes at least its 10s, so any earlier return
    // came from the body's completion.
    assert!(
        elapsed < Duration::from_millis(10_000),
        "elapsed {elapsed:?} — body should resume parent on completion (~50ms), \
         not wait for the 10s timeout"
    );
}
