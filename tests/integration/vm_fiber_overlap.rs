//! VM `Async.both` / `Async.race` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2).
//!
//! With concurrent fiber dispatch, `both(sleep(50), sleep(50))` should
//! finish in ~50ms — both timers run on the same OS thread, parked on
//! the mio reactor; the dispatch loop wakes whichever fires first and
//! resumes the other only after its own completion. Sequential execution
//! would be ~100ms.
//!
//! `race(sleep(150), sleep(20))` should finish in ~20ms (the fast branch
//! wins). The slow branch keeps running in the background until its own
//! sleep completes — that's a small CPU waste but no observable
//! correctness issue (cancellation is 1b-vi-c work).

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(1);

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-fiber-overlap-{}-{}-{}",
        std::process::id(),
        id,
        fixture_tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for fiber-overlap fixture");
    let path = dir.join("vm_fiber_overlap.flx");
    std::fs::write(&path, source).expect("write fiber-overlap fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on fiber-overlap fixture");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn both_overlap_runs_in_parallel() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async {
    let _ = sleep(500)
    1
}

fn right() -> Int with Async {
    let _ = sleep(500)
    2
}

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let pair = run_async(body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "both_overlap");
    assert!(
        success,
        "both program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Lower bound: sleeps must actually wait — at least one 500ms sleep.
    // (`--no-cache` adds ~1s of stdlib-recompile startup, so total is ~1.5s
    // when fibers overlap, ~2s when they run sequentially.)
    assert!(
        elapsed >= Duration::from_millis(450),
        "elapsed {elapsed:?} too short — sleeps must wait at least ~500ms"
    );
    // Upper bound: fibers must overlap. Sequential = ~1000ms of sleep (plus
    // startup); concurrent = ~500ms of sleep (plus startup). Choosing a bound
    // below the worst-case startup + sequential keeps the assertion robust.
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — fibers didn't overlap (sequential would be \
         ~1000ms of sleep + startup, concurrent should be ~500ms + startup)"
    );
    assert!(
        stdout.contains('1') && stdout.contains('2'),
        "expected both results in output:\nstdout:\n{stdout}"
    );
}

#[test]
fn race_returns_first_finisher() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(1000)
    1
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    2
}

fn body() -> Int with Async {
    race(slow, fast)
}

fn main() with IO {
    let winner = run_async(body)
    print(winner)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "race_first_finisher");
    assert!(
        success,
        "race program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // Upper bound: parent must resume on fast's completion (~50ms),
    // not wait for slow (~1000ms). Includes ~1s startup overhead from
    // `--no-cache` stdlib recompile, so generous bound at 1500ms.
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — race didn't return on fast (sequential or \
         waiting-on-slow would be ~1000ms of sleep + startup, fast-wins \
         should be ~50ms + startup)"
    );
    assert!(
        stdout.contains('2'),
        "expected fast branch's result (2) in output:\nstdout:\n{stdout}"
    );
}
