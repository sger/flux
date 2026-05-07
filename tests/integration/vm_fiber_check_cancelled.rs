//! VM `FiberCheckCancelled` integration tests (proposal 0174 Phase 2 slice 2-iv).
//!
//! Verifies that `Async.check_cancelled` reads the per-fiber cancel flag
//! correctly:
//!
//! 1. In a non-cancelled fiber it returns `false`.
//! 2. After a `timeout` cancels its body (the timer wins first), the body
//!    fiber's resume from `sleep` runs through the dispatch-loop cancel
//!    path; the post-sleep code observes `check_cancelled() == true`.
//!
//! These tests are timing-light: only an upper bound is asserted on the
//! wall clock since `check_cancelled` itself is a flag read, not a sleep.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-check-cancelled-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for check_cancelled fixture");
    let path = dir.join("vm_check_cancelled.flx");
    std::fs::write(&path, source).expect("write check_cancelled fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on check_cancelled fixture");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn check_cancelled_returns_false_in_non_cancelled_fiber() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Bool with Async {
    check_cancelled()
}

fn main() with IO {
    let _ = run_async(body)
    print("ok")
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source);
    assert!(
        success,
        "check_cancelled() program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("ok"),
        "expected 'ok' on stdout, got: {stdout:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — flag check should be ~instant"
    );
}

#[test]
fn check_cancelled_observes_timeout_cancellation_after_resume() {
    // The body fiber is started by `timeout(20, body)`; while body is
    // parked on sleep(50), the 20ms timer fires first. The timeout path
    // calls cancel_losers on the body fiber, which marks its id in the
    // CANCELLED_IDS set. The dispatch loop resumes the cancelled body
    // with Value::None; the post-sleep code calls check_cancelled() and
    // observes the cancel flag.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Bool with Async {
    sleep(50)
    check_cancelled()
}

fn outer() -> Bool with Async {
    let _ = timeout(20, body)
    true
}

fn main() with IO {
    let _ = run_async(outer)
    print("done")
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source);
    assert!(
        success,
        "timeout-cancelled program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    // The body fiber's post-sleep run-through is bounded by the original
    // 50ms sleep duration plus pump latency.
    assert!(
        elapsed < Duration::from_secs(5),
        "elapsed {elapsed:?} too long — dispatch loop likely stuck"
    );
    assert!(
        stdout.contains("done"),
        "expected 'done' on stdout, got: {stdout:?}"
    );
}
