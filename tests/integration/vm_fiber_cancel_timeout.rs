//! Acid test: timeout body-child backend request is cancelled after the timer
//! fires (proposal 0174 Phase 1b-vi-c).
//!
//! `timeout(50, slow)` where slow sleeps 2000ms. The timer fires at ~50ms
//! returning `None`. Before 1b-vi-c the body fiber's 2s sleep request stayed
//! registered in mio until `exit_run_async`'s drain loop finally dropped it.
//! After 1b-vi-c, `backend.cancel` is called for the body child immediately
//! when `try_route_timer_for_timeout` fires.
//!
//! Observable difference: program completes well under 500ms, not ~2000ms.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-fiber-cancel-timeout-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn timeout_body_child_request_is_cancelled() {
    // timer fires at 50ms; body would run 2000ms if not cancelled.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
    99
}

fn body() -> String with Async {
    let result = timeout(50, slow)
    match result {
        None -> "timed_out",
        Some(_) -> "completed"
    }
}

fn main() with IO {
    let s = run_async(body)
    print(s)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_cancel");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("timed_out"),
        "expected 'timed_out' output:\nstdout:\n{stdout}"
    );
    // Must complete well before the body's 2s sleep. Allow 1800ms for
    // `--no-cache` startup + 50ms timer; without cancellation this takes ~3s.
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — body child's 2s timer was not cancelled \
         (should complete in ~50ms timer + startup)"
    );
}
