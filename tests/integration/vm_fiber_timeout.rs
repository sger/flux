//! VM `Async.timeout` overlap acid test (proposal 0174 Phase 1b-vi-b₂.2 follow-up).
//!
//! `timeout(ms, f)` returns `Some(f())` if `f` completes within `ms`,
//! otherwise `None`. Both branches run concurrently via the fiber
//! scheduler — the body fiber and a backend timer race for the same
//! request id; whichever fires first wins.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, fixture_tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-fiber-timeout-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        fixture_tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for fiber-timeout fixture");
    let path = dir.join("vm_fiber_timeout.flx");
    std::fs::write(&path, source).expect("write fiber-timeout fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on fiber-timeout fixture");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn timeout_fires_when_body_too_slow() {
    // Body would take 1000ms; timeout cuts it off at 50ms. Should return
    // None (printed as -1) and finish ~50ms after run_async start (well
    // before slow's 1000ms would complete).
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let _ = sleep(1000)
    99
}

fn body() -> Option<Int> with Async {
    timeout(50, slow)
}

fn main() with IO {
    let r = run_async(body)
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_fires");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("-1"),
        "expected None (-1) in output:\nstdout:\n{stdout}"
    );
    // Timer must fire well before slow's 1000ms. Includes ~1s startup
    // overhead from --no-cache stdlib recompile.
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — timeout didn't return on timer (slow's full \
         1000ms + startup would be >2000ms; timer + startup should be ~1100ms)"
    );
}

#[test]
fn timeout_returns_some_when_body_in_time() {
    // Body completes in 50ms; timeout is 1000ms. Should return Some(42).
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    let _ = sleep(50)
    42
}

fn body() -> Option<Int> with Async {
    timeout(1000, fast)
}

fn main() with IO {
    let r = run_async(body)
    match r {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "timeout_succeeds");
    assert!(
        success,
        "timeout program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected Some(42) result in output:\nstdout:\n{stdout}"
    );
    // Body must wake parent before timeout fires (~50ms vs 1000ms).
    assert!(
        elapsed < Duration::from_millis(1500),
        "elapsed {elapsed:?} — body should resume parent on completion (~50ms), \
         not wait for timeout (~1000ms)"
    );
}
