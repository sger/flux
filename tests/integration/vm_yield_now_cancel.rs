//! `yield_now` cancellation-checkpoint confirmation (proposal 0177 T2.3).
//!
//! `yield_now` already performs a real cancellation checkpoint: a fiber that
//! cooperatively yields observes its enclosing scope's cancellation and stops
//! at the yield point instead of running to completion. This pins that
//! semantics *deterministically* — under the seedable single-worker scheduler
//! (proposal 0177 T1.1), with zero `sleep`, so the result is reproducible and
//! load-independent.
//!
//! ## Why this is a real test (and not a false positive)
//!
//! The loser runs a **finite** cooperative loop (`spin`, up to `LOOP_BOUND`
//! ticks) that records each tick on a channel; `race`'s winner cancels it once
//! it resolves. We assert the loser was curtailed to far fewer than
//! `LOOP_BOUND` ticks. Crucially, this isolates `yield_now`'s checkpoint: with
//! the `is_current_cancelled()` guard in `FiberYieldNow` temporarily removed,
//! the very same seeds run the loser to the full `LOOP_BOUND` (200) instead of
//! ~1 — so a regression that drops the checkpoint flips the assertion. The
//! bound is finite, so a regression fails the assertion rather than hanging.
//!
//! `check_cancelled`'s analogous checkpoint is covered by
//! `vm_fiber_check_cancelled.rs`.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

/// Upper bound on the loser's cooperative loop. A run where `yield_now` stops
/// observing cancellation drives the loser to exactly this many ticks.
const LOOP_BOUND: i64 = 200;

/// `spin` is a finite cooperative loop recording each tick on a channel and
/// yielding between ticks; `winner` yields twice (so the loser gets to run and
/// park at least once) then resolves the race, cancelling the loser. `body`
/// returns how many ticks the loser actually recorded before being cancelled.
const SOURCE: &str = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn spin(ch: Channel<Int>, n: Int) -> Int with Async {
    if n >= 200 { n }
    else {
        let _ = Channel.send(ch, n)
        yield_now()
        spin(ch, n + 1)
    }
}

fn winner() -> Int with Async {
    yield_now()
    yield_now()
    7
}

fn body() -> Int with Async {
    let ch = Channel.make(1024)
    let _ = race(winner, fn() { spin(ch, 0) })
    Channel.len(ch)
}

fn main() with IO {
    print(run_async_with(with_deterministic_scheduler(SEED), body))
}
"#;

fn ticks_for_seed(seed: i64, tag: &str) -> i64 {
    let source = SOURCE.replace("SEED", &seed.to_string());
    let (stdout, stderr, success) = flux_runner::run_flux(&source, tag);
    assert!(
        success,
        "yield_now cancellation program (seed {seed}) must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
        .trim()
        .parse::<i64>()
        .unwrap_or_else(|_| panic!("expected an integer tick count, got: {stdout:?}"))
}

#[test]
fn yield_now_curtails_cancelled_cooperative_loop() {
    // Seeds whose deterministic interleaving lets `winner` win the race after
    // the loser has run and parked at least once. For each, the loser must be
    // curtailed *well* before LOOP_BOUND — proof that `yield_now` raised
    // Canceled at its checkpoint. (With the checkpoint removed, these same
    // seeds run the loser to the full LOOP_BOUND.)
    for seed in [0_i64, 7, 123] {
        let ticks = ticks_for_seed(seed, &format!("yield_cancel_seed_{seed}"));
        assert!(
            ticks >= 1,
            "seed {seed}: the loser should have run at least one cooperative tick \
             before cancellation, got {ticks}"
        );
        assert!(
            ticks < LOOP_BOUND,
            "seed {seed}: the loser ran to completion ({ticks}/{LOOP_BOUND}) — \
             `yield_now` did not observe cancellation at its checkpoint"
        );
    }
}

#[test]
fn yield_now_cancellation_is_seed_stable() {
    // The same seed must replay the identical curtailment every run — the
    // deterministic-scheduler guarantee, with zero `sleep`.
    let first = ticks_for_seed(0, "yield_cancel_stable_first");
    for i in 0..10 {
        let again = ticks_for_seed(0, &format!("yield_cancel_stable_{i}"));
        assert_eq!(
            again, first,
            "seed 0 must replay the same cancellation curtailment every run"
        );
    }
}
