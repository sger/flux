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

#[test]
fn native_both_overlaps_and_preserves_source_order() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async {
    sleep(500)
    3
}

fn right() -> Int with Async {
    sleep(500)
    4
}

fn body() -> (Int, Int) with Async {
    both(left, right)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let pair = run_async(body)
    let t1 = now_ms()
    print(pair.0)
    print(pair.1)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "both");
    assert!(
        success,
        "native both must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(&lines[..2], ["3", "4"]);
    let measured_ms: i64 = lines[2].parse().expect("elapsed ms");
    assert!(
        measured_ms >= 450,
        "both returned too early: {measured_ms}ms"
    );
    assert!(measured_ms < 900, "both did not overlap: {measured_ms}ms");
}

#[test]
fn native_race_is_fifo_for_immediate_children() {
    let source = r#"
import Flow.Async exposing (..)

fn first() -> Int with Async { 10 }
fn second() -> Int with Async { 20 }

fn body() -> Int with Async {
    race(first, second)
}

fn main() with IO {
    let v = run_async(body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "race_fifo");
    assert!(
        success,
        "native race must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "10");
}

#[test]
fn native_race_cancels_slow_loser() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(2000)
    1
}

fn fast() -> Int with Async {
    sleep(50)
    2
}

fn body() -> Int with Async {
    race(slow, fast)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let v = run_async(body)
    let t1 = now_ms()
    print(v)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "race_cancel");
    assert!(
        success,
        "native race cancellation must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "2");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "native race waited for the slow loser: {measured_ms}ms"
    );
}

#[test]
fn native_timeout_returns_none_when_timer_wins() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(2000)
    7
}

fn body() -> Option<Int> with Async {
    timeout(50, slow)
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let opt = run_async(body)
    let t1 = now_ms()
    match opt {
        Some(v) -> print(v),
        None    -> print(-1)
    }
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "timeout_none");
    assert!(
        success,
        "native timeout timer-win must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "-1");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "native timeout waited for the slow body: {measured_ms}ms"
    );
}

#[test]
fn native_timeout_returns_some_when_body_wins() {
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    sleep(50)
    7
}

fn body() -> Option<Int> with Async {
    timeout(1000, fast)
}

fn main() with IO {
    let opt = run_async(body)
    match opt {
        Some(v) -> print(v),
        None    -> print(-1)
    }
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "timeout_some");
    assert!(
        success,
        "native timeout body-win must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn native_nested_combinators_smoke() {
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    sleep(10)
    5
}

fn slow() -> Int with Async {
    sleep(100)
    9
}

fn left() -> Option<Int> with Async {
    timeout(1000, fast)
}

fn right() -> Int with Async {
    race(slow, fast)
}

fn body() -> (Option<Int>, Int) with Async {
    both(left, right)
}

fn main() with IO {
    let pair = run_async(body)
    match pair.0 {
        Some(v) -> print(v),
        None    -> print(-1)
    }
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "nested");
    assert!(
        success,
        "native nested combinators must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "5\n5");
}
