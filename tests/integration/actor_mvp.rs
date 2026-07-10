//! Actor MVP end-to-end tests (proposal 0177 T4.4) — VM backend.
//!
//! Two halves:
//!
//! 1. The shipped `examples/actors/*.flx` run on the VM with their documented
//!    output — the examples are read from disk so they stay the single source
//!    of truth (the native twin, `native_actor_mvp_tests.rs`, runs the same
//!    files with `--native` and must agree).
//! 2. Deterministic-scheduler ordering: actor fibers participate in the T1.1
//!    seeded ready-pick, so a fixed seed replays a fixed spawn-wake
//!    interleaving. Mailbox wake-ups (channel publishes) resume in publish
//!    order by design — the seed permutes which *ready* fiber runs first, not
//!    the completion routing — so the reply half of the string is stable
//!    across seeds while the wake half varies.

#[path = "../support/flux_runner.rs"]
mod flux_runner;

use std::process::Command;

/// Run a repo-relative `.flx` file on the VM via the `flux` CLI.
fn run_example(rel_path: &str) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(flux_runner::workspace_root())
        .args([rel_path, "--no-cache"])
        .output()
        .expect("run flux");
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

fn assert_example(rel_path: &str, expected: &str) {
    let (stdout, stderr, ok) = run_example(rel_path);
    assert!(
        ok,
        "{rel_path} must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), expected, "{rel_path} output");
}

#[test]
fn actor_counter_example() {
    assert_example("examples/actors/counter.flx", "\"count = 2\"");
}

#[test]
fn actor_ping_pong_example() {
    assert_example("examples/actors/ping_pong.flx", "\"rallies = 3\"");
}

#[test]
fn actor_fan_out_example() {
    assert_example("examples/actors/fan_out.flx", "\"sum of squares = 30\"");
}

/// Three actors each announce their tag when first scheduled (the seeded
/// ready-pick decides that order), then each echoes one mailbox message
/// (publish-order, seed-independent). Output: `<wake order>-<reply order>`.
const DET_ACTOR_SOURCE: &str = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel
import Flow.Channel exposing (Channel)
import Flow.Actor exposing (..)

fn echo(mb: Mailbox<String>, out: Channel<String>, tag: String) -> Unit with Async {
    let _ = Channel.send(out, tag)
    let m = receive(mb)
    Channel.send(out, m)
}

fn drain(out: Channel<String>, n: Int, acc: String) -> String with Async {
    if n == 0 {
        acc
    } else {
        match Channel.recv(out) {
            Some(s) -> drain(out, n - 1, acc + s),
            None    -> acc
        }
    }
}

fn body() -> String with Async {
    let out = Channel.make(8)
    let a = spawn(fn(mb) { echo(mb, out, "A") })
    let b = spawn(fn(mb) { echo(mb, out, "B") })
    let c = spawn(fn(mb) { echo(mb, out, "C") })
    let wakes = drain(out, 3, "")
    tell(a, "x")
    tell(b, "y")
    tell(c, "z")
    let replies = drain(out, 3, "")
    wakes + "-" + replies
}

fn main() with IO { print(run_async_with(with_deterministic_scheduler(SEED), body)) }
"#;

fn run_det(seed: i64, tag: &str) -> String {
    let source = DET_ACTOR_SOURCE.replace("SEED", &seed.to_string());
    let (stdout, stderr, ok) = flux_runner::run_flux(&source, tag);
    assert!(
        ok,
        "deterministic actor program (seed {seed}) must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout.trim().to_string()
}

#[test]
fn actor_wake_order_is_seed_selected_and_reproducible() {
    // Distinct interleavings per seed prove the seed drives actor scheduling;
    // seed 0 is the strict-FIFO contract. Replies are publish-ordered (xyz)
    // regardless of seed.
    let s0 = run_det(0, "actor_det_seed0");
    let s1 = run_det(1, "actor_det_seed1");
    let s99 = run_det(99, "actor_det_seed99");
    assert_eq!(s0, "\"ABC-xyz\"", "seed 0 is strict FIFO");
    assert_eq!(s1, "\"CAB-xyz\"");
    assert_eq!(s99, "\"BAC-xyz\"");
}

#[test]
fn actor_interleaving_replays_byte_identically() {
    let first = run_det(1, "actor_det_replay_first");
    for i in 0..10 {
        let again = run_det(1, &format!("actor_det_replay_{i}"));
        assert_eq!(again, first, "seed 1 must replay the same interleaving");
    }
}
