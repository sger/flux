//! VM `FiberScheduler` registry plumbing tests (proposal 0174 Phase 1b-vi-b₁).
//!
//! Phase 1b-vi-b₁ wires every `FiberRunAsync` boundary and every `FiberFork`
//! through a real `FiberScheduler`, but executes bodies inline so observable
//! behaviour is unchanged. These tests pin the user-visible contract: an
//! `Async.run_async` wrapping a body of `fork`s succeeds and finishes
//! promptly, with no scheduler leaks across boundaries.
//!
//! The actual park/resume cycle and overlap of `both` / `race` are b₂ work;
//! tests for that live in `vm_fiber_overlap.rs` (added then).

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use std::time::Duration;

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    flux_runner::run_flux_timed(source, fixture_tag)
}

#[test]
fn fiber_run_async_creates_scheduler_and_root() {
    // Bare `Async.run_async`-wrapped body: scheduler initialises on entry,
    // tears down on exit, no panics.  The assertion is implicit in process
    // success — any leaked scheduler state would surface as a panic from
    // `enter_run_async` / `exit_run_async`.
    let source = r#"
import Flow.Async as Async

fn main() with Async {
    Async.sleep(0)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "run_async_root");
    assert!(
        success,
        "run_async program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — scheduler init/teardown likely stuck",
    );
}

#[test]
fn fiber_fork_allocates_child_id() {
    // `Async.both` lowers to a pair of `fork` calls. b₁ runs each body
    // inline but routes them through `scheduler.spawn`, allocating a real
    // `FiberId` each time. Sequential semantics preserved: total wall
    // clock ≈ sum, not max.  Note the `both` signature requires the second
    // function to carry `with Async`; named functions are the simplest
    // shape (parser limitation noted in lib/Flow/Async.flx).
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int { 1 }
fn right() -> Int with Async { 2 }

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let _ = run_async(body)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "fork_alloc_id");
    assert!(
        success,
        "both/fork program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — fork registry path likely stuck",
    );
}

#[test]
fn nested_run_async_boundaries_dont_leak() {
    // Two consecutive top-level `run_async` calls on the same OS thread:
    // the scheduler must drop cleanly after the first so the second can
    // re-init without seeing leftover state.
    let source = r#"
import Flow.Async exposing (..)

fn first() -> Int with Async { 1 }
fn second() -> Int with Async { 2 }

fn main() with IO {
    let a = run_async(first)
    let b = run_async(second)
    print(a)
    print(b)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "nested");
    assert!(
        success,
        "consecutive run_async program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — boundary teardown path likely stuck",
    );
}
