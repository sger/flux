//! VM `Async.both` / `Async.race` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2).
//!
//! With concurrent fiber dispatch, `both(sleep(50), sleep(50))` should
//! finish in ~50ms — both timers run on the same OS thread, parked on
//! the mio reactor; the dispatch loop wakes whichever fires first and
//! resumes the other only after its own completion. Sequential execution
//! would be ~100ms.
//!
//! `race(sleep(150), sleep(20))` should finish in ~20ms (the fast branch
//! wins). The slow branch keeps running in the background until its own
//! sleep completes — that's a small CPU waste but no observable
//! correctness issue (cancellation is 1b-vi-c work).

use crate::support::flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

/// Each fiber reports when its sleep started and ended, and the two runs must
/// overlap. That holds however loaded the machine is, where a time budget
/// for the whole program (sequential ~1000ms against ~500ms) failed on a
/// slow runner although the sleeps did overlap.
#[test]
fn both_overlap_runs_in_parallel() {
    let source = r#"
import Flow.Async exposing (..)

fn run_of(start: Int, stop: Int) -> (Int, Int) { (start, stop) }

fn slept() -> (Int, Int) with Async, Time {
    let start = now_ms()
    let _ = sleep(500)
    run_of(start, now_ms())
}

fn body() -> ((Int, Int), (Int, Int)) with Async, Time {
    both(slept, slept)
}

fn print_run(run: (Int, Int)) with IO {
    print(run.0)
    print(run.1)
}

fn main() with IO, Time {
    let t0 = now_ms()
    let runs = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    print_run(runs.0)
    print_run(runs.1)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "both_overlap");
    assert!(success, "both program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    // Lower bound: sleeps must actually wait — at least one 500ms sleep.
    assert!(
        elapsed >= Duration::from_millis(450),
        "elapsed {elapsed:?} too short — sleeps must wait at least ~500ms"
    );
    let times: Vec<u64> = stdout.lines().filter_map(|line| line.trim().parse().ok()).collect();
    let [left_start, left_end, right_start, right_end] = times[..] else {
        panic!("expected four times, start and end of each sleep:\nstdout:\n{stdout}");
    };
    assert!(
        left_start < right_end && right_start < left_end,
        "the sleeps ran one after the other ({left_start}..{left_end}, then \
         {right_start}..{right_end}), so `both` did not overlap them"
    );
}

#[test]
fn race_returns_first_finisher() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(1000)
    1
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    2
}

fn body() -> Int with Async {
    race(slow, fast)
}

fn main() with IO, Time {
    let t0 = now_ms()
    let winner = run_async(body)
    print("elapsed_ms=" + to_string(now_ms() - t0))
    print(winner)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "race_first_finisher");
    assert!(success, "race program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let (stdout, elapsed) = flux_runner::program_elapsed(&stdout);
    // Upper bound: parent must resume on fast's completion (~50ms),
    // not wait for slow (~1000ms).
    assert!(
        elapsed < Duration::from_millis(700),
        "elapsed {elapsed:?} — race didn't return on fast (waiting on slow \
         would be ~1000ms of sleep, fast-wins should be ~50ms)"
    );
    assert!(
        stdout.contains('2'),
        "expected fast branch's result (2) in output:\nstdout:\n{stdout}"
    );
}
