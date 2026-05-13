//! Acid tests: real `scope`/`fork`/`cancel` (proposal 0174 Phase 1b-vi-c).
//!
//! `scope` now allocates a real cancellation boundary; `fork` registers child
//! fibers under it; `cancel` stops them immediately by cancelling their backend
//! requests and resuming their continuations with `AsyncError.Canceled`.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-fiber-scope-cancel-{}-{}-{}",
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
fn scope_cancel_stops_forked_fibers() {
    // Fork two slow sleeps inside a scope; cancel after 100ms.
    // Without cancellation this would take ~2000ms. With it, ~100ms.
    let source = r#"
import Flow.Async exposing (..)

fn slow_work() -> Int with Async {
    let _ = sleep(2000)
    0
}

fn scoped_body(s) -> Int with Async {
    let _f1 = fork(s, slow_work)
    let _f2 = fork(s, slow_work)
    let _w = sleep(100)
    let _c = cancel(s)
    42
}

fn body() -> Int with Async {
    scope(scoped_body)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "scope_cancel");
    assert!(
        success,
        "scope-cancel program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected result 42:\nstdout:\n{stdout}"
    );
    // Must complete well before the forked fibers' 2s sleeps.
    assert!(
        elapsed < Duration::from_millis(1800),
        "elapsed {elapsed:?} — scope cancel did not stop forked fibers"
    );
}

#[test]
fn fork_runs_body() {
    // fork spawns a child fiber and runs its body; verify result is correct.
    let source = r#"
import Flow.Async exposing (..)

fn child_work() -> Int with Async {
    let _ = sleep(10)
    0
}

fn scoped_body(s) -> Int with Async {
    let _f = fork(s, child_work)
    99
}

fn body() -> Int with Async {
    scope(scoped_body)
}

fn main() with IO {
    let result = run_async(body)
    print(result)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "fork_runs");
    assert!(
        success,
        "fork program must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("99"),
        "expected result 99:\nstdout:\n{stdout}"
    );
}
