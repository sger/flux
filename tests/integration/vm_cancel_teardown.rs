//! Cancel-then-immediate-teardown regression tests (KI-6).
//!
//! Cancelling a fiber that is parked on a channel `recv` and then returning
//! from `run_async` straight away used to corrupt the boundary: the dispatch
//! loop resumed the cancelled fiber's continuation on the shared worker-0 VM
//! *after* the root fiber's final tick had already unwound the stack to the
//! `run_async` boundary. Depending on the shape, `run_async` then returned a
//! stale stack slot (`None` instead of the root's value) or surfaced the
//! cancelled fiber's `Canceled` error as a phantom `E1009 __fiber_error__`
//! crash at `main`. The fix: once the root has completed, the dispatch loop
//! ticks no further fibers — remaining fibers are reaped by `exit_run_async`,
//! exactly as they already were when the root exited while children were
//! still parked.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

/// Plain scope/fork shape: the cancelled fiber completes with Unit — pre-fix
/// its resume clobbered the root result (printed `"None"` instead of `"7"`).
const FORK_CANCEL_RETURN: &str = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn blocked(ch) -> Unit with Async {
    match Channel.recv(ch) {
        Some(_) -> yield_now(),
        None    -> yield_now()
    }
}

fn body() -> Int with Async {
    let ch = Channel.make(1)
    let s = new_scope()
    fork(s, fn() { blocked(ch) })
    yield_now()
    cancel(s)
    7
}

fn main() with IO { print(to_string(run_async(body))) }
"#;

/// Actor shape: `stop` cancels an actor parked in `receive`, whose `Canceled`
/// error pre-fix escaped `run_async` as an `E1009 __fiber_error__` crash.
const ACTOR_STOP_RETURN: &str = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel
import Flow.Channel exposing (Channel)
import Flow.Actor exposing (..)

fn looping(mb: Mailbox<Int>, acc: Int) -> Unit with Async {
    let x = receive(mb)
    looping(mb, acc + x)
}

fn body() -> Int with Async {
    let c = spawn(fn(mb) { looping(mb, 0) })
    tell(c, 1)
    yield_now()
    stop(c)
    7
}

fn main() with IO { print(to_string(run_async(body))) }
"#;

fn assert_seven(source: &str, tag: &str) {
    let (stdout, stderr, ok) = flux_runner::run_flux(source, tag);
    assert!(
        ok,
        "{tag} must succeed (KI-6: cancel-blocked-recv before teardown):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        stdout.trim(),
        "\"7\"",
        "{tag}: run_async must return the root's value"
    );
}

#[test]
fn cancel_blocked_recv_then_immediate_return_keeps_root_result() {
    assert_seven(FORK_CANCEL_RETURN, "ki6_fork_cancel");
}

#[test]
fn actor_stop_then_immediate_return_does_not_error() {
    assert_seven(ACTOR_STOP_RETURN, "ki6_actor_stop");
}

#[test]
fn cancel_teardown_is_stable_across_runs_and_workers() {
    for i in 0..5 {
        assert_seven(FORK_CANCEL_RETURN, &format!("ki6_loop_{i}"));
    }
    let (stdout, stderr, ok, _elapsed) = flux_runner::run_flux_with_env(
        FORK_CANCEL_RETURN,
        "ki6_workers4",
        &[("FLUX_WORKERS", "4")],
    );
    assert!(
        ok,
        "multi-worker cancel-teardown must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "\"7\"");
}
