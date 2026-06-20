//! VM deterministic test scheduler end-to-end.
//!
//! Exercises `Async.with_deterministic_scheduler(seed)` through the full `flux`
//! CLI: the `RuntimeConfig.det_seed` field → the arity-5 `FiberRunAsyncWith`
//! primop → `FiberScheduler::new_deterministic` selection in `enter_run_async`.
//!
//! Four fibers each send a tag to a shared channel (no `sleep`); because all
//! four are ready at the same scheduling point, the seeded ready-pick decides
//! the order they run, so the **channel drain order is the observable fiber
//! interleaving**. We assert (a) the order is byte-identical across many runs for
//! a fixed seed (reproducibility — the T1.1 done-criterion, with zero timing
//! thresholds) and (b) different seeds yield different, each-stable orders
//! (the seed actually selects an interleaving). The fibers capture the channel
//! in un-annotated closures — only possible after the closure-effect fix.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

const FOUR_FIBER_SOURCE: &str = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel

fn body() -> String with Async {
    let ch = Channel.make(16)
    both(
        fn() { both(fn() { Channel.send(ch, "A") }, fn() { Channel.send(ch, "B") }) },
        fn() { both(fn() { Channel.send(ch, "C") }, fn() { Channel.send(ch, "D") }) }
    )
    let a = Channel.recv(ch)
    let b = Channel.recv(ch)
    let c = Channel.recv(ch)
    let d = Channel.recv(ch)

    match (a, b, c, d) {
        (Some(w), Some(x), Some(y), Some(z)) -> w + x + y + z,
        _ -> "closed"
    }
}

fn main() with IO {
    print(run_async_with(with_deterministic_scheduler(SEED), body))
}
"#;

fn source_with_seed(seed: i64) -> String {
    FOUR_FIBER_SOURCE.replace("SEED", &seed.to_string())
}

fn run_once(seed: i64, tag: &str) -> String {
    let (stdout, stderr, success) = flux_runner::run_flux(&source_with_seed(seed), tag);
    assert!(
        success,
        "deterministic-scheduler program (seed {seed}) must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout.trim().to_string()
}

#[test]
fn deterministic_scheduler_output_is_byte_identical_across_runs() {
    let first = run_once(42, "det_first");
    // A genuine permutation of the four tags not the trivial source orde).
    // The exact permutation encodes the order children enter the ready queue;
    // it shifted (from "ADBC") made synthetic-await
    // children reserve-then-activate-after-park instead of spawning eagerly.
    assert_eq!(
        first, "\"CADB\"",
        "unexpected interleaving for seed 42: {first}"
    );
    for i in 0..20 {
        let again = run_once(42, &format!("det_repeat_{i}"));
        assert_eq!(
            again, first,
            "seed 42 must replay the same interleaving every run"
        );
    }
}

#[test]
fn deterministic_scheduler_seed_selects_the_interleaving() {
    // Each seed is internally stable and the seeds disagree proof the seed
    // actually drives the schedule (not just incidental determinism).
    let s0 = run_once(0, "det_seed0"); // strict FIFO
    let s1 = run_once(1, "det_seed1");
    let s99 = run_once(99, "det_seed99");
    assert_eq!(s0, "\"ABCD\"", "seed 0 is strict FIFO");
    assert_eq!(s1, "\"BADC\"");
    assert_eq!(s99, "\"DABC\"");
    // Stable on repeat.
    assert_eq!(run_once(1, "det_seed1_again"), s1);
}
