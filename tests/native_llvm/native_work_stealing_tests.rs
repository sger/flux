//! Native scheduler load-balancing tests (proposal 0174 Phase 2 follow-up).
//!
//! Native fibers use least-loaded-queue spawn placement and real
//! cross-worker stealing. Each fiber carries a C effect-context snapshot so
//! a stolen fiber can resume with the handler/evidence state it had on its
//! previous worker.
//!
//! These tests cover the **escape hatch**: every async program must still
//! pass with `FLUX_WORK_STEALING=0` so a future regression rooted in the
//! stealing path has a clean fallback. They run the same program twice —
//! once with placement on (default), once off — and assert identical
//! output. Race tie-breaking is the highest-risk surface; covered both by
//! `native_race_is_fifo_for_immediate_children` (default settings) and
//! the explicit-off case below.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source_with_env(source: &str, tag: &str, env: &[(&str, &str)]) -> (String, String, bool) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!("flux-load-balance-{}-{}", std::process::id(), tag,));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flux"));
    cmd.current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"]);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run flux native");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

/// Multi-fiber program runs identically with default placement and with
/// `FLUX_WORK_STEALING=0`. Validates the env-var opt-out is wired.
#[test]
fn placement_opt_out_preserves_program_output() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async { 100 }
fn right() -> Int with Async { 200 }

fn pair() -> (Int, Int) with Async {
    both(left, right)
}

fn body() -> Int with Async {
    let p1 = pair()
    let p2 = pair()
    p1.0 + p1.1 + p2.0 + p2.1
}

fn main() with IO {
    print(run_async_with_workers(2, body))
}
"#;
    let (on_stdout, on_stderr, on_ok) = run_source_with_env(source, "placement_on", &[]);
    assert!(
        on_ok,
        "default (least-loaded placement) must succeed:\nstdout:\n{on_stdout}\nstderr:\n{on_stderr}"
    );
    assert_eq!(on_stdout.trim(), "600");

    let (off_stdout, off_stderr, off_ok) =
        run_source_with_env(source, "placement_off", &[("FLUX_WORK_STEALING", "0")]);
    assert!(
        off_ok,
        "FLUX_WORK_STEALING=0 must still succeed:\nstdout:\n{off_stdout}\nstderr:\n{off_stderr}"
    );
    assert_eq!(off_stdout.trim(), "600");
}

/// Race tie-breaking is the highest-risk semantic for any change to
/// scheduler placement. Mirror `native_race_is_fifo_for_immediate_children`
/// but force the env opt-out so we have a regression that fails LOUDLY if
/// the round-robin fallback path ever stops preserving FIFO source order.
#[test]
fn race_immediate_fifo_holds_under_round_robin_fallback() {
    let source = r#"
import Flow.Async exposing (..)

fn first() -> Int with Async { 10 }
fn second() -> Int with Async { 20 }

fn body() -> Int with Async {
    race(first, second)
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, ok) = run_source_with_env(
        source,
        "race_fifo_round_robin",
        &[("FLUX_WORK_STEALING", "0")],
    );
    assert!(
        ok,
        "race must succeed under round-robin fallback:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "10");
}

/// Stress: many concurrent fibers complete with the right total under
/// the new placement. `both(one, one)` repeated 8 times spawns 16 child
/// fibers, exercising the placement chooser repeatedly.
#[test]
fn many_concurrent_fibers_produce_correct_total() {
    let source = r#"
import Flow.Async exposing (..)

fn one() -> Int with Async { 1 }

fn pair() -> (Int, Int) with Async {
    both(one, one)
}

fn body() -> Int with Async {
    let a = pair()
    let b = pair()
    let c = pair()
    let d = pair()
    let e = pair()
    let f = pair()
    let g = pair()
    let h = pair()
    a.0 + a.1 + b.0 + b.1 + c.0 + c.1 + d.0 + d.1 +
    e.0 + e.1 + f.0 + f.1 + g.0 + g.1 + h.0 + h.1
}

fn main() with IO {
    print(run_async_with_workers(2, body))
}
"#;
    let (stdout, stderr, ok) = run_source_with_env(source, "many_fibers", &[]);
    assert!(
        ok,
        "many concurrent fibers must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "16");
}

/// Race children are intentionally queued on the caller's worker for FIFO
/// tie-breaking. Once a child suspends, it becomes stealable; after the fast
/// child's timer fires, an idle worker can resume it and still run a
/// parameterized handler correctly on the stealing worker.
#[test]
fn stolen_fiber_runs_parameterized_handler() {
    let source = r#"
import Flow.Async exposing (..)

effect Counter {
    next: () -> Int
}

fn count_twice() -> Int with Counter {
    let _first = perform Counter.next()
    perform Counter.next()
}

fn slow() -> Int with Async {
    sleep(20)
    99
}

fn fast() -> Int with Async {
    sleep(1)
    count_twice() handle Counter(5) {
        next(resume, state) -> resume(state, state + 1)
    }
}

fn body() -> Int with Async {
    race(slow, fast)
}

fn main() with IO {
    print(run_async_with_workers(2, body))
}
"#;
    let (stdout, stderr, ok) = run_source_with_env(source, "stolen_handler", &[]);
    assert!(
        ok,
        "stolen fiber must run parameterized handler:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "6");
}

/// Many same-parent candidates should still complete correctly under the
/// default scheduler, even when source-order-sensitive immediate candidates
/// are protected from stealing until they suspend.
#[test]
fn imbalanced_ready_queue_completes_under_stealing() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(20)
    1
}

fn fast1() -> Int with Async { 41 }
fn fast2() -> Int with Async { 42 }
fn fast3() -> Int with Async { 43 }
fn fast4() -> Int with Async { 44 }

fn body() -> Int with Async {
    first_of([slow, fast1, fast2, fast3, fast4]).1
}

fn main() with IO {
    print(run_async_with_workers(4, body))
}
"#;
    let (stdout, stderr, ok) = run_source_with_env(source, "imbalanced_ready", &[]);
    assert!(
        ok,
        "imbalanced ready queue must complete under stealing:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(["41", "42", "43", "44"].contains(&stdout.trim()));
}
