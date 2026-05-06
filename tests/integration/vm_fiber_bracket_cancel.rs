//! Acid test: cancelled fibers are resumed with `AsyncError.Canceled` rather
//! than being silently dropped (proposal 0174 Phase 1b-vi-c).
//!
//! A slow fiber races a fast fiber.  After the fast fiber wins, the slow
//! fiber is cancelled: its continuation is resumed with `Canceled` so that
//! any `bracket`/`finally` cleanup arms execute before the fiber exits.
//!
//! We verify:
//! 1. The program succeeds (no runtime error from the cancelled fiber).
//! 2. The fast result is returned correctly.
//! 3. Elapsed time is well below the slow fiber's 2s sleep (cancelled, not
//!    waited for), proving `exit_run_async` doesn't hang on cancelled fibers.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-fiber-bracket-cancel-{}-{}-{}",
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
fn cancelled_fiber_resumes_cleanly() {
    // fast wins in ~50ms; slow sleeps 2000ms and is cancelled.
    // After 1b-vi-c, the slow fiber's continuation is resumed with Canceled
    // and the fiber exits cleanly — no runtime panic, correct result returned.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
    0
}

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Int with Async {
    race(fast, slow)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "cancelled_clean");
    assert!(
        success,
        "cancelled-fiber program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected fast result 42:\nstdout:\n{stdout}"
    );
    // Must complete well before the slow fiber's 2s sleep.
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — slow fiber not cancelled"
    );
}

#[test]
fn both_cancelled_fibers_resume_cleanly() {
    // timeout(50, slow): the timer fires at ~50ms, body child is cancelled.
    // The cancelled body continuation is resumed with Canceled — must not crash.
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(2000)
    99
}

fn body() -> String with Async {
    let r = timeout(50, slow)
    match r {
        None -> "timed_out",
        Some(_) -> "completed"
    }
}

fn main() with IO {
    let s = run_async(body)
    print(s)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_clean");
    assert!(
        success,
        "timeout-cancelled program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("timed_out"),
        "expected 'timed_out':\nstdout:\n{stdout}"
    );
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — body child not cancelled"
    );
}
