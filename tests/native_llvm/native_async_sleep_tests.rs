//! Native async sleep bridge tests (proposal 0174 Phase 1b-vi-d).

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source(source: &str, tag: &str) -> (String, String, bool, Duration) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!(
            "flux-native-async-sleep-{}-{}",
            std::process::id(),
            tag,
        ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("fixture.flx");
    std::fs::write(&path, source).expect("write fixture");

    let start = Instant::now();
    // Private cache root: `--no-cache` does not isolate native
    // builds, which write shared artifacts regardless (KI-010).
    let scratch = Scratch::new("native-llvm");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .args(scratch.cache_args())
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
fn native_run_async_with_workers_convenience_returns_value() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    321
}

fn main() with IO {
    let v = run_async_with_workers(1, body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "run_async_with_workers");
    assert!(
        success,
        "native run_async_with_workers must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "321");
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
fn native_parallel_reentry_has_no_global_execution_lock() {
    let native_abi =
        std::fs::read_to_string(workspace_root().join("src/runtime/async/native_abi.rs"))
            .expect("read native async ABI");

    assert!(
        !native_abi.contains("EXECUTION_LOCK") && !native_abi.contains("execution_lock()"),
        "native generated-code re-entry must not be serialized by a global execution lock"
    );
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
fn native_parallel_reentry_repeated_both_allocates_on_workers() {
    let source = r#"
import Flow.Async exposing (..)

fn one() -> Int with Async {
    sleep(10)
    let pair = (1, 2)
    pair.0 + pair.1
}

fn round() -> Int with Async {
    let pair = both(one, one)
    pair.0 + pair.1
}

fn body() -> Int with Async {
    round() + round() + round() + round() + round() + round() +
    round() + round() + round() + round() + round() + round()
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "parallel_reentry_alloc");
    assert!(
        success,
        "native parallel re-entry allocation stress must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "72");
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
fn native_race_cancels_loser_when_it_reaches_next_suspend() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let pair = (1, 2)
    let _ = pair.0 + pair.1
    sleep(2000)
    1
}

fn fast() -> Int with Async {
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
    let (stdout, stderr, success, _elapsed) = run_source(source, "race_cancel_running");
    assert!(
        success,
        "native race running-loser cancellation must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "2");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "cancelled running loser parked and kept run_async alive: {measured_ms}ms"
    );
}

#[test]
fn native_first_of_returns_fastest_index_and_cancels_losers() {
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

fn body() -> (Int, Int) with Async {
    first_of([slow, fast, slow])
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
    let (stdout, stderr, success, _elapsed) = run_source(source, "first_of_fastest");
    assert!(
        success,
        "native first_of fastest must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(&lines[..2], ["1", "2"]);
    let measured_ms: i64 = lines[2].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "first_of waited for slow losers: {measured_ms}ms"
    );
}

#[test]
fn native_first_of_immediate_children_are_source_ordered() {
    let source = r#"
import Flow.Async exposing (..)

fn ten() -> Int with Async { 10 }
fn twenty() -> Int with Async { 20 }
fn thirty() -> Int with Async { 30 }

fn body() -> (Int, Int) with Async {
    first_of([ten, twenty, thirty])
}

fn main() with IO {
    let pair = run_async(body)
    print(pair.0)
    print(pair.1)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "first_of_fifo");
    assert!(
        success,
        "native first_of tie must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "0\n10");
}

#[test]
fn native_first_returns_fastest_value() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(2000)
    1
}

fn fast() -> Int with Async {
    sleep(50)
    7
}

fn body() -> Int with Async {
    first([slow, fast, slow])
}

fn main() with IO, Clock {
    let t0 = now_ms()
    let v = run_async(body)
    let t1 = now_ms()
    print(v)
    print(t1 - t0)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "first_fastest");
    assert!(
        success,
        "native first fastest must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "7");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "first waited for slow losers: {measured_ms}ms"
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
fn native_timeout_cancels_body_when_it_reaches_next_suspend() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    let pair = (1, 2)
    let _ = pair.0 + pair.1
    sleep(2000)
    7
}

fn body() -> Option<Int> with Async {
    timeout(1, slow)
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
    let (stdout, stderr, success, _elapsed) = run_source(source, "timeout_cancel_running");
    assert!(
        success,
        "native timeout running-body cancellation must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines[0], "-1");
    let measured_ms: i64 = lines[1].parse().expect("elapsed ms");
    assert!(
        measured_ms < 500,
        "cancelled timeout body parked and kept run_async alive: {measured_ms}ms"
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

#[test]
fn native_direct_helper_sleep_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn helper() -> Int with Async {
    sleep(10)
    11
}

fn body() -> Int with Async {
    let v = helper()
    v + 1
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_sleep");
    assert!(
        success,
        "native direct helper sleep must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "12");
}

#[test]
fn native_sequential_direct_helper_sleeps_resume_each_call() {
    let source = r#"
import Flow.Async exposing (..)

fn helper() -> Int with Async {
    sleep(10)
    3
}

fn body() -> Int with Async {
    let a = helper()
    let b = helper()
    let c = helper()
    a + b + c
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_seq_sleep");
    assert!(
        success,
        "native sequential helper sleeps must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "9");
}

#[test]
fn native_direct_helper_wrapping_both_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn left() -> Int with Async {
    sleep(10)
    2
}

fn right() -> Int with Async {
    sleep(10)
    5
}

fn helper() -> Int with Async {
    let pair = both(left, right)
    pair.0 + pair.1
}

fn body() -> Int with Async {
    helper()
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_both");
    assert!(
        success,
        "native helper wrapping both must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn native_direct_helper_wrapping_race_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn slow() -> Int with Async {
    sleep(100)
    1
}

fn fast() -> Int with Async {
    sleep(10)
    8
}

fn helper() -> Int with Async {
    race(slow, fast)
}

fn body() -> Int with Async {
    helper()
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_race");
    assert!(
        success,
        "native helper wrapping race must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "8");
}

#[test]
fn native_direct_helper_wrapping_timeout_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn fast() -> Int with Async {
    sleep(10)
    13
}

fn helper() -> Int with Async {
    let opt = timeout(1000, fast)
    match opt {
        Some(v) -> v,
        None    -> -1
    }
}

fn body() -> Int with Async {
    helper()
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_timeout");
    assert!(
        success,
        "native helper wrapping timeout must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "13");
}

#[test]
fn native_nested_direct_helper_chain_propagates_yield() {
    let source = r#"
import Flow.Async exposing (..)

fn b() -> Int with Async {
    sleep(10)
    6
}

fn a() -> Int with Async {
    b()
}

fn body() -> Int with Async {
    a() + 1
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_chain");
    assert!(
        success,
        "native nested direct helper chain must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "7");
}

#[test]
fn native_direct_helper_rounds_resume_after_each_combinator() {
    let source = r#"
import Flow.Async exposing (..)

fn one() -> Int with Async {
    sleep(10)
    1
}

fn round() -> Int with Async {
    let pair = both(one, one)
    let winner = race(one, one)
    let opt = timeout(1000, one)
    match opt {
        Some(v) -> pair.0 + pair.1 + winner + v,
        None    -> 0
    }
}

fn body() -> Int with Async {
    let a = round()
    let b = round()
    let c = round()
    let d = round()
    let e = round()
    a + b + c + d + e
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source(source, "helper_rounds");
    assert!(
        success,
        "native direct helper rounds must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "20");
}
