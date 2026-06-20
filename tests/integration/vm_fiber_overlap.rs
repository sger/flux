//! VM `Async.both` / `Async.race` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2).
//!
//! De-flake: these used to assert on elapsed-time margins
//! (e.g. "both finished in <1800ms, so the fibers overlapped"), which flaked
//! under CI load because the working and broken durations sat only a few hundred
//! ms apart. They now prove the same properties *semantically*, with no
//! load-sensitive threshold:
//!
//! - **Overlap** is proven by a channel **rendezvous**: each `both` child
//!   announces itself and then waits for the other. Both children can only
//!   complete if they are alive *simultaneously*; if `both` ran them
//!   sequentially the first child would block forever waiting for the second,
//!   so completion ⟺ overlap. No sleeps, so a passing run is fast; a regression
//!   deadlocks and trips a wide deadlock guard.
//! - **Race early-exit** is proven with the wide-gap pattern: the slow branch
//!   sleeps a large fixed amount (30s); a working `race` returns on the fast
//!   branch in ~50ms, a regression that waits for the loser blocks ~30s.
//!
//! See vm_fiber_cancel_loser.rs and docs/internals/concurrency_model.md §1.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

#[test]
fn both_overlap_runs_in_parallel() {
    // Rendezvous overlap proof: `left` announces on `a_started` then waits on
    // `b_started`; `right` waits on `a_started` then announces on `b_started`.
    // Both can only complete if `both` runs them concurrently — sequential
    // execution would block `left` forever on `recv(b_started)`. Completion ⟺
    // overlap, with no timing threshold.
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> (Int, Int) with Async {
    let a_started = Channel.make(1)
    let b_started = Channel.make(1)
    let left = fn() -> Int with Async {
        let _ls = Channel.send(a_started, 1)
        let _lr = Channel.recv(b_started)
        1
    }
    let right = fn() -> Int with Async {
        let _rr = Channel.recv(a_started)
        let _rs = Channel.send(b_started, 1)
        2
    }
    both(left, right)
}

fn main() with IO {
    let pair = run_async(body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "both_overlap");
    assert!(
        success,
        "both program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains('1') && stdout.contains('2'),
        "expected both results in output:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard: the rendezvous completes near-instantly when the
    // fibers overlap; a regression that serialises them deadlocks and trips this.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — both children did not rendezvous (no overlap)"
    );
}

#[test]
fn race_returns_first_finisher() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(30000)
    1
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    2
}

fn body() -> Int with Async {
    race(slow, fast)
}

fn main() with IO {
    let winner = run_async(body)
    print(winner)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "race_first_finisher");
    assert!(
        success,
        "race program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Wide-gap deadlock guard: parent resumes on fast's completion (~50ms +
    // compile, well under 8s); a regression that waits for the loser blocks on
    // slow's 30s sleep and trips this. Not load-sensitive.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — race didn't return on fast (waiting on the \
         loser would block on its 30s sleep; fast-wins should be ~50ms + compile)"
    );
    assert!(
        stdout.contains('2'),
        "expected fast branch's result (2) in output:\nstdout:\n{stdout}"
    );
}
