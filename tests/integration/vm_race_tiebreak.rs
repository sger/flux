//! `race` / `first_of` source-order tie-break contract (proposal 0177 T2.4).
//!
//! **Decision (T2.4):** `race` stays **2-way** and delegates the n-way case to
//! `first_of` (`race(f, g)` is `first_of([f, g])` projected to the winner's
//! value). Both share one deterministic tie-break: **source order**. When more
//! than one branch is simultaneously *runnable* (ready in the same cooperative
//! round, including across `yield_now`), the earliest branch in source order
//! wins; a later branch wins only once every earlier branch is *suspended on a
//! real async wait* (e.g. `sleep` / I/O), not merely yielding.
//!
//! These tests **lock** that contract under the seedable single-worker
//! deterministic scheduler (T1.1): the invariant must hold for *every* seed, so
//! it is a genuine semantic guarantee rather than an artifact of one
//! interleaving. (`first_of`'s sleep-driven fastest-wins path is covered by
//! `vm_fiber_first_of.rs`; the deterministic scheduler does not virtualize
//! timers, so the sleep cases stay there.)

#[path = "../support/flux_runner.rs"]
mod flux_runner;

/// Seeds swept for every invariant. Seed 0 is strict FIFO; the rest permute the
/// ready-pick order, so identical results across all of them prove the tie-break
/// is decided by source order, not by scheduling luck.
const SEEDS: &[i64] = &[0, 1, 2, 3, 5, 7, 11, 42, 99, 123, 7777];

fn run_seeded(template: &str, seed: i64, tag: &str) -> String {
    let source = template.replace("SEED", &seed.to_string());
    let (stdout, stderr, success) = flux_runner::run_flux(&source, tag);
    assert!(
        success,
        "tie-break program (seed {seed}) must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout.trim().to_string()
}

#[test]
fn race_immediate_tie_resolves_in_source_order_for_every_seed() {
    // Both branches return immediately, so both are runnable at once: race must
    // return the FIRST branch's value (10), never the second (20), on any seed.
    let template = r#"
import Flow.Async exposing (..)
fn first() -> Int with Async { 10 }
fn second() -> Int with Async { 20 }
fn body() -> Int with Async { race(first, second) }
fn main() with IO { print(run_async_with(with_deterministic_scheduler(SEED), body)) }
"#;
    for &seed in SEEDS {
        let out = run_seeded(template, seed, &format!("race_tie_{seed}"));
        assert_eq!(
            out, "10",
            "seed {seed}: race must return the source-order-first branch"
        );
    }
}

#[test]
fn first_of_immediate_tie_resolves_in_source_order_for_every_seed() {
    // All three are immediately runnable: first_of must report index 0 / value
    // 10 (the source-order-first), encoded as index*1000 + value = 10.
    let template = r#"
import Flow.Async exposing (..)
fn ten() -> Int with Async { 10 }
fn twenty() -> Int with Async { 20 }
fn thirty() -> Int with Async { 30 }
fn body() -> Int with Async {
    let r = first_of([ten, twenty, thirty])
    r.0 * 1000 + r.1
}
fn main() with IO { print(run_async_with(with_deterministic_scheduler(SEED), body)) }
"#;
    for &seed in SEEDS {
        let out = run_seeded(template, seed, &format!("first_of_tie_{seed}"));
        assert_eq!(
            out, "10",
            "seed {seed}: first_of must pick index 0 (value 10) on a tie"
        );
    }
}

#[test]
fn earlier_source_branch_keeps_priority_across_a_yield_for_every_seed() {
    // The earlier branch cooperatively yields once before returning 1; the later
    // branch returns 2 immediately. Because the earlier branch stays *runnable*
    // (it yields, it does not suspend on a real wait), it keeps source-order
    // priority and wins — index 0 / value 1, encoded as 1 — regardless of seed.
    // A regression that let the immediately-ready later branch steal the win
    // would yield index 1 / value 2 (encoded 1002).
    let template = r#"
import Flow.Async exposing (..)
fn slow_first() -> Int with Async {
    yield_now()
    1
}
fn fast_second() -> Int with Async { 2 }
fn body() -> Int with Async {
    let r = first_of([slow_first, fast_second])
    r.0 * 1000 + r.1
}
fn main() with IO { print(run_async_with(with_deterministic_scheduler(SEED), body)) }
"#;
    for &seed in SEEDS {
        let out = run_seeded(template, seed, &format!("yield_prio_{seed}"));
        assert_eq!(
            out, "1",
            "seed {seed}: earlier source branch must keep priority across a yield"
        );
    }
}
