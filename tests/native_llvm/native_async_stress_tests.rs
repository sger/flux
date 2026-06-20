//! Native async stress/soak harness.
//!
//! The native twin of `tests/integration/async_stress.rs`. Native fibers use
//! real cross-worker stealing with a C effect-context snapshot per fiber, so
//! the migration/cancel paths are a *different* implementation from the VM's
//! and need their own stress coverage. Same contract, same fixtures, same
//! exact-total invariants — see the VM file's module doc for the rationale
//! (no panics ⇒ no leaked continuations; exact total ⇒ no lost/duplicated
//! completions; the only timing bound is a deadlock guard, not a margin).
//!
//! Soak coverage is the looped binary:
//!
//! ```sh
//! for i in $(seq 1 100); do \
//!   cargo test --features llvm --test native_async_stress_tests --quiet || break; \
//! done
//! ```

#![cfg(feature = "llvm")]

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

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
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!("flux-async-stress-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux native");

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

/// Run `source` `REPEATS` times under the chaos env on the native backend,
/// asserting no deadlock + success + exact `expected_total` on every run.
fn assert_stable_total(source: &str, tag: &str, expected_total: i64) {
    for i in 0..REPEATS {
        let (stdout, stderr, exited_ok, timed_out) =
            run_until_deadline(source, &format!("{tag}_{i}"), CHAOS_ENV);
        assert!(
            !timed_out,
            "native stress fixture '{tag}' run {i} DEADLOCKED (killed after \
             {HANG_KILL:?})\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            exited_ok,
            "native stress fixture '{tag}' run {i} must succeed (no panic / no \
             leaked continuation):\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert_eq!(
            stdout.trim(),
            expected_total.to_string(),
            "native stress fixture '{tag}' run {i}: total drifted — a lost \
             completion undershoots, a double-resume overshoots\nstderr:\n{stderr}"
        );
    }
}

/// 4096 leaf fibers across 8 workers, migration on, no cancellation — pure
/// spawn/steal/complete pressure. Total must be exactly 4096.
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
    assert_stable_total(source, "native_fanout_tree", 4096);
}

/// 1024 concurrent `race(slow, fast)` leaves: `fast` wins, the 2s `slow` must
/// be cancelled. Total exactly 1024; finishing inside the hang guard proves
/// the losers were cancelled, not awaited.
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
    assert_stable_total(source, "native_racing_cancel", 1024);
}

/// 512 concurrent 5ms timeouts wrapping a 2s sleeper: every body times out and
/// must be cancelled. Each contributes 1; total exactly 512.
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
    assert_stable_total(source, "native_timeout_churn", 512);
}
