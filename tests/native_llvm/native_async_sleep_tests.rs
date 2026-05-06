//! Native async sleep bridge tests (proposal 0174 Phase 1b-vi-d).

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = std::env::temp_dir().join(format!(
        "flux-native-async-sleep-{}-{}",
        std::process::id(),
        tag,
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let start = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run flux native");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

#[test]
fn native_run_async_sleep_resumes_after_timer() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    sleep(50)
    99
}

fn main() with IO {
    let v = run_async(body)
    print(v)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source(source, "sleep");
    assert!(
        success,
        "native async sleep must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "99");
    assert!(
        elapsed >= Duration::from_millis(40),
        "native sleep returned too early: {elapsed:?}"
    );
}

#[test]
fn native_sleep_path_does_not_use_blocking_os_sleep() {
    let tasks = std::fs::read_to_string(workspace_root().join("runtime/c/tasks.c"))
        .expect("read runtime/c/tasks.c");
    let start = tasks
        .find("int64_t flux_fiber_sleep")
        .expect("find flux_fiber_sleep");
    let tail = &tasks[start..];
    let end = tail
        .find("/* ── Fiber combinators")
        .expect("find next section");
    let sleep_impl = &tail[..end];

    assert!(
        !sleep_impl.contains("nanosleep") && !sleep_impl.contains("Sleep("),
        "flux_fiber_sleep must route through the async ABI, not block the OS thread"
    );
    assert!(sleep_impl.contains("flux_async_timer_start"));
    assert!(sleep_impl.contains("flux_async_suspend"));
}
