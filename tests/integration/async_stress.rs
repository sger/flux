//! VM async stress/soak harness.
//!
//! The late-0.0.6 concurrency bugs all clustered at the cancel × steal ×
//! completion boundary and were each found by luck, not by a harness. This is
//! that harness: programs that spawn *thousands* of fibers under forced
//! migration and work-stealing, hammering the spawn / steal / cancel / timeout
//! paths simultaneously, and assert the runtime's load-bearing invariants:
//!
//!   * **No panics** — a poisoned worker or a teardown assertion (leaked
//!     scheduler state, the Phase 0 "no leaked continuations" invariant)
//!     surfaces as a non-zero process exit, so `success` *is* the leak check.
//!   * **No lost or duplicated completions** — every fixture folds the result
//!     of N fibers into a single integer total with one known-correct value.
//!     A lost completion undershoots; a double-resumed continuation overshoots.
//!     An exact-equals assertion catches both.
//!
//! Timing is deliberately *not* asserted (that is T1.4's job to remove
//! elsewhere): the only wall-clock bound here is a generous hang guard that
//! fails a deadlock rather than a slow machine.
//!
//! Each test runs its fixture several times in-process to shake out
//! nondeterministic races; the documented way to push coverage further is to
//! loop the whole binary:
//!
//! ```sh
//! for i in $(seq 1 100); do cargo test --test async_stress --quiet || break; done
//! ```

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Force migration + stealing on so native fibers actually move across workers.
const CHAOS_ENV: &[(&str, &str)] = &[("FLUX_FIBER_MIGRATION", "1"), ("FLUX_WORK_STEALING", "1")];
const REPEATS: usize = 3;

/// Hard wall-clock kill deadline per run (native links + runs, so it is roomier
/// than the VM's). A deadlock fails the test loudly instead of hanging CI.
const HANG_KILL: Duration = Duration::from_secs(90);

/// Spawn native `flux` on `source`, polling until exit or `HANG_KILL` (then
/// kill). Returns `(stdout, stderr, exited_ok, timed_out)`.
fn run_until_deadline(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, bool) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-scratch")
        .join(format!("flux-async-stress-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .args([path.to_str().unwrap(), "--no-cache"])
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux");

    let start = Instant::now();
    let (mut exited_ok, mut timed_out) = (false, false);
    loop {
        match child.try_wait().expect("try_wait on flux child") {
            Some(status) => {
                exited_ok = status.success();
                break;
            }
            None => {
                if start.elapsed() > HANG_KILL {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    let mut stdout = String::new();
    let mut stderr = String::new();

    if let Some(mut o) = child.stdout.take() {
        let _ = o.read_to_string(&mut stdout);
    }
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut stderr);
    }
    let _ = std::fs::remove_file(&path);
    (
        stdout.replace("\r\n", "\n"),
        stderr.replace("\r\n", "\n"),
        exited_ok,
        timed_out,
    )
}

/// Run `source` `REPEATS` times under the chaos env, asserting on every run it
/// does not deadlock, succeeds, and emits exactly `expected_total`.
fn assert_stable_total(source: &str, tag: &str, expected_total: i64) {
    for i in 0..REPEATS {
        let (stdout, stderr, exited_ok, timed_out) =
            run_until_deadline(source, &format!("{tag}_{i}"), CHAOS_ENV);
        assert!(
            !timed_out,
            "stress fixture '{tag}' run {i} DEADLOCKED (killed after {HANG_KILL:?}) — \
             a lost completion / lost wakeup at the cancel×steal×completion \
             boundary\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            exited_ok,
            "stress fixture '{tag}' run {i} must succeed (no panic / no leaked \
             continuation):\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(
            stdout.trim(),
            expected_total.to_string(),
            "stress fixture '{tag}' run {i}: total drifted — a lost completion \
             undershoots, a double-resume overshoots\nstderr:\n{stderr}"
        );
    }
}

/// A depth-`d` `both`-tree spawns 2^d leaf fibers (plus the internal forks),
/// each contributing 1. Depth 12 = 4096 concurrent leaves across 8 workers
/// with migration on — pure spawn/steal/complete pressure, no cancellation.
/// Total must be exactly 4096: no fiber's result may be lost or counted twice.
#[test]
fn fanout_tree_completes_every_fiber_exactly_once() {
    let source = r#"
import Flow.Async exposing (..)

fn tree(depth: Int) -> Int with Async {
    if depth == 0 {
        yield_now()
        1
    } else {
        let p = both(fn() { tree(depth - 1) }, fn() { tree(depth - 1) })
        p.0 + p.1
    }
}

fn body() -> Int with Async {
    tree(12)
}

fn main() with IO {
    print(run_async_with_workers(8, body))
}
"#;
    assert_stable_total(source, "fanout_tree", 4096);
}

/// Every leaf is a `race(slow, fast)` where `fast` wins immediately and `slow`
/// (a 2s sleep) must be cancelled. A depth-10 tree runs 1024 such races
/// concurrently across 8 workers — 1024 loser cancellations racing against
/// 1024 winner completions and constant migration. `fast` always wins, so the
/// total is exactly 1024; the suite finishing well inside the hang guard also
/// proves the slow losers were actually cancelled rather than awaited.
#[test]
fn racing_cancel_under_migration_never_loses_a_winner() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async { sleep(2000) 0 }
fn fast() -> Int with Async { yield_now() 1 }

fn leaf() -> Int with Async {
    race(slow, fast)
}

fn tree(depth: Int) -> Int with Async {
    if depth == 0 {
        leaf()
    } else {
        let p = both(fn() { tree(depth - 1) }, fn() { tree(depth - 1) })
        p.0 + p.1
    }
}

fn body() -> Int with Async {
    tree(10)
}

fn main() with IO {
    print(run_async_with_workers(8, body))
}
"#;
    assert_stable_total(source, "racing_cancel", 1024);
}

/// Every leaf wraps a 2s sleeper in a 5ms `timeout`, so each times out and the
/// timed-out body must be cancelled. A depth-9 tree runs 512 timeouts
/// concurrently. Each contributes 1 (the `None` arm); a body that leaked past
/// its timeout, or a timeout that double-counted, would shift the total off
/// 512.
#[test]
fn timeout_churn_cancels_every_body_exactly_once() {
    let source = r#"
import Flow.Async exposing (..)

fn sleeper() -> Int with Async { sleep(2000) 99 }

fn leaf() -> Int with Async {
    match timeout(5, sleeper) {
        Some(_) -> 0,
        None -> 1
    }
}

fn tree(depth: Int) -> Int with Async {
    if depth == 0 {
        leaf()
    } else {
        let p = both(fn() { tree(depth - 1) }, fn() { tree(depth - 1) })
        p.0 + p.1
    }
}

fn body() -> Int with Async {
    tree(9)
}

fn main() with IO {
    print(run_async_with_workers(8, body))
}
"#;
    assert_stable_total(source, "timeout_churn", 512);
}
