//! VM `Async.timeout` overlap acid test.
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
    // Body would block 30s; timeout cuts it off at 50ms. Should return
    // None (printed as -1) and finish ~50ms after run_async start (well
    // before slow's sleep would complete).
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(30000)
    99
}

fn body() -> Option<Int> with Async {
    timeout(50, slow)
}

fn main() with IO {
    let r = run_async(body)
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_fires");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("-1"),
        "expected None (-1) in output:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard (proposal 0177 T1.4): working run finishes in
    // compile + ~50ms timer (well under 8s on any CI load); a regressed run
    // blocks on slow's 30s sleep and trips this. See vm_fiber_cancel_loser.rs
    // and docs/internals/concurrency_model.md §1.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — timeout didn't return on timer (slow's full \
         30s sleep would block; timer + startup should be ~50ms + compile)"
    );
}

#[test]
fn timeout_returns_some_when_body_in_time() {
    // Body completes in 50ms; timeout window is 30s. Should return Some(42)
    // as soon as the body finishes, without waiting out the window.
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Option<Int> with Async {
    timeout(30000, fast)
}

fn main() with IO {
    let r = run_async(body)
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_succeeds");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected Some(42) result in output:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard (proposal 0177 T1.4): the body resumes the parent
    // on completion (~50ms + compile, well under 8s); a regression that waits
    // out the full 30s timeout window would trip this. Not load-sensitive.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — body should resume parent on completion (~50ms), \
         not wait for the 30s timeout window"
    );
}
