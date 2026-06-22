//! VM multi-OS-worker fiber parallelism tests (proposal 0174 §A-5 Phase 4).
//!
//! These tests verify that `run_async_with_workers(N, ...)` on the VM backend
//! actually spawns real OS worker threads and runs fibers in parallel.
//!
//! Tests:
//!   1. Correctness — `both` with 2 workers returns the right values.
//!   2. Parallelism — two CPU-bound fibers across 2 workers run faster than
//!      sequentially.
//!   3. Channel correctness — fibers on different workers exchange values over
//!      a `Channel<Int>` without deadlock.
//!   4. Single-worker parity — `run_async_with_workers(1, ...)` still works
//!      (exercises the unchanged single-threaded path).
//!   5. Four-worker race returns the fast branch.
//!   6. Load-aware spawn placement (least-loaded queue) and the
//!      `FLUX_WORK_STEALING=0` round-robin fallback both produce correct output.

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_with_env(source, tag, &[])
}

fn run_source_with_env(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, Duration) {
    flux_runner::run_flux_with_env(source, tag, env)
}

// ── Test 1: correctness ───────────────────────────────────────────────────

#[test]
fn multiworker_both_returns_correct_values() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async { 11 }
fn right() -> Int with Async { 22 }

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let pair = run_async_with_workers(2, body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "both_correct");
    assert!(
        success,
        "multiworker both must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["11", "22"],
        "expected left=11 right=22 in order:\nstdout:\n{stdout}"
    );
}

// ── Test 2: parallelism ───────────────────────────────────────────────────

#[test]
fn multiworker_sleep_both_overlaps() {
    // Parallelism proof via a channel rendezvous across 2 OS workers
    // Each `both` child announces itself and waits for the
    // other; both can only complete if they run simultaneously on the two
    // workers. Sequential execution would block the first child forever. This
    // replaces the old "<2800ms" timing margin, which flaked under CI load.
    // See vm_fiber_overlap.rs and docs/internals/concurrency_model.md §1.
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> (Int, Int) with Async {
    let a_started = Channel.make(1)
    let b_started = Channel.make(1)
    let slow_left = fn() -> Int with Async {
        let _ls = Channel.send(a_started, 1)
        let _lr = Channel.recv(b_started)
        1
    }
    let slow_right = fn() -> Int with Async {
        let _rr = Channel.recv(a_started)
        let _rs = Channel.send(b_started, 1)
        2
    }
    both(slow_left, slow_right)
}

fn main() with IO {
    let pair = run_async_with_workers(2, body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "sleep_parallel");
    assert!(
        success,
        "multiworker sleep both must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines,
        ["1", "2"],
        "expected both fibers' results in order:\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard: the rendezvous completes near-instantly when the
    // fibers overlap; a regression that serialises them deadlocks and trips this.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — fibers didn't overlap across workers (no rendezvous)"
    );
}

// ── Test 3: channel across workers ────────────────────────────────────────

#[test]
fn multiworker_channel_exchange() {
    // Sender and receiver on different workers exchange a value via a channel.
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> Int with Async {
    let c = Channel.make(1)
    Channel.send(c, 42)
    let got = Channel.recv(c)
    match got {
        Some(v) -> v,
        None -> 0
    }
}

fn main() with IO {
    let v = run_async_with_workers(2, body)
    print(v)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "channel_exchange");
    assert!(
        success,
        "multiworker channel exchange must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected channel value (42) in output:\nstdout:\n{stdout}"
    );
}

// ── Test 4: single-worker parity ─────────────────────────────────────────

#[test]
fn single_worker_path_unchanged() {
    // run_async_with_workers(1, ...) must use the unchanged single-threaded
    // dispatch_loop and produce correct results.
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async { 7 }
fn right() -> Int with Async { 8 }

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let pair = run_async_with_workers(1, body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _) = run_source(source, "single_worker");
    assert!(
        success,
        "single-worker path must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains('7') && stdout.contains('8'),
        "expected both results in output:\nstdout:\n{stdout}"
    );
}

// ── Test 5: four workers, multiple fibers ─────────────────────────────────

#[test]
fn four_workers_race_returns_winner() {
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    99
}

fn slow() -> Int with Async {
    let _ = sleep(30000)
    0
}

fn body() -> Int with Async {
    race(fast, slow)
}

fn main() with IO {
    let v = run_async_with_workers(4, body)
    print(v)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "four_workers_race");
    assert!(
        success,
        "four-worker race must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("99"),
        "expected fast branch result (99):\nstdout:\n{stdout}"
    );
    // Wide-gap deadlock guard (proposal 0177 T1.4): working run returns on the
    // fast branch in ~50ms + compile (well under 8s on any CI load); a
    // regression that waits for the loser blocks on its 30s sleep and trips this.
    assert!(
        elapsed < Duration::from_secs(8),
        "elapsed {elapsed:?} — race didn't return on fast branch"
    );
}

// ── Test 6: load-aware spawn placement + FLUX_WORK_STEALING escape hatch ──
//
// The VM child-spawn path places fresh fibers on the least-loaded worker queue
// by default, and falls back to round-robin when FLUX_WORK_STEALING=0. Placement
// must never change program results — these run the same multi-worker fixture
// under both policies and assert identical, correct output.

const PLACEMENT_FIXTURE: &str = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async { 100 }
fn right() -> Int with Async { 200 }

fn pair() -> (Int, Int) with Async { both(left, right) }

fn body() -> Int with Async {
    let p1 = pair()
    let p2 = pair()
    let p3 = pair()
    let p4 = pair()
    p1.0 + p1.1 + p2.0 + p2.1 + p3.0 + p3.1 + p4.0 + p4.1
}

fn main() with IO {
    print(run_async_with_workers(2, body))
}
"#;

#[test]
fn placement_default_least_loaded_is_correct() {
    let (stdout, stderr, success, _) = run_source(PLACEMENT_FIXTURE, "placement_default");
    assert!(
        success,
        "default least-loaded placement must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "1200");
}

#[test]
fn placement_round_robin_fallback_is_correct() {
    let (stdout, stderr, success, _) = run_source_with_env(
        PLACEMENT_FIXTURE,
        "placement_round_robin",
        &[("FLUX_WORK_STEALING", "0")],
    );
    assert!(
        success,
        "FLUX_WORK_STEALING=0 round-robin fallback must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "1200");
}

#[test]
fn single_worker_placement_round_robin_fallback_is_correct() {
    // With one logical worker, least-loaded and round-robin coincide (everything
    // lands on worker 0), so race FIFO tie-break is deterministic either way.
    let source = r#"
import Flow.Async exposing (..)

fn first() -> Int with Async { 10 }
fn second() -> Int with Async { 20 }

fn body() -> Int with Async { race(first, second) }

fn main() with IO {
    print(run_async_with_workers(1, body))
}
"#;
    let (stdout, stderr, success, _) = run_source_with_env(
        source,
        "single_worker_round_robin",
        &[("FLUX_WORK_STEALING", "0")],
    );
    assert!(
        success,
        "single-worker race under round-robin fallback must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "10");
}
