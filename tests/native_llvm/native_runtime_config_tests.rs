//! Native `RuntimeConfig` end-to-end (proposal 0174 Phase 2 slice 2-vii cleanup).
//!
//! Mirrors [`tests/integration/vm_runtime_config.rs`] but drives the LLVM
//! native backend via `--native`. Verifies that:
//!
//!   - explicit `worker_count` flows from `run_async_with_workers` down
//!     through `flux_async_run_root_with` and the program returns the
//!     expected value;
//!   - the default-fallback path (`worker_count = None`) succeeds, which
//!     exercises `resolve_default_worker_count`'s
//!     env → `available_parallelism` → 2 chain;
//!   - `FLUX_WORKERS` env var is read on native (regression: prior to this
//!     slice it was VM-only).
//!
//! Direct introspection of the worker-thread count is not reachable from a
//! CLI test; the fact that the program succeeds and returns the right
//! value is sufficient evidence the wiring is intact.

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_source_with_env(
    source: &str,
    tag: &str,
    env: &[(&str, &str)],
) -> (String, String, bool, Duration) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!(
            "flux-native-runtime-config-{}-{}",
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
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flux"));
    cmd.current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .args(scratch.cache_args());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run flux native");
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success(), elapsed)
}

fn kill_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}

fn run_source_with_timeout(
    source: &str,
    tag: &str,
    timeout: Duration,
) -> (String, String, bool, Duration) {
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!(
            "flux-native-runtime-config-timeout-{}-{}",
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .args(scratch.cache_args())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn flux native");

    let timed_out = loop {
        if child.try_wait().expect("poll flux native").is_some() {
            break false;
        }
        if start.elapsed() >= timeout {
            kill_process_tree(&mut child);
            let _ = child.kill();
            break true;
        }
        thread::sleep(Duration::from_millis(50));
    };

    if timed_out {
        let _ = child.wait();
        let elapsed = start.elapsed();
        let _ = std::fs::remove_file(&path);
        return (
            String::new(),
            format!("timed out after {}s", timeout.as_secs()),
            false,
            elapsed,
        );
    }

    let output = child
        .wait_with_output()
        .expect("collect flux native output");
    let elapsed = start.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (
        stdout,
        stderr,
        output.status.success() && !timed_out,
        elapsed,
    )
}

#[test]
fn native_run_async_with_explicit_workers_returns_value() {
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    42
}

fn main() with IO {
    let v = run_async_with_workers(4, body)
    print(v)
}
"#;
    let (stdout, stderr, success, elapsed) = run_source_with_env(source, "explicit", &[]);
    assert!(
        success,
        "native run_async_with_workers(4, ...) must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("42"),
        "expected 42 on stdout, got: {stdout:?}"
    );
    assert!(elapsed < Duration::from_secs(30));
}

#[test]
fn native_current_worker_count_matches_explicit_setting() {
    // Strong assertion: with `worker_count = 4`, the runtime reports
    // exactly 4 active workers via `Async.current_worker_count()`.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    current_worker_count()
}

fn main() with IO {
    print(run_async_with_workers(4, body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, "wc-explicit", &[]);
    assert!(
        success,
        "current_worker_count must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("4"),
        "expected 4 on stdout, got: {stdout:?}"
    );
}

#[test]
fn native_current_worker_count_matches_flux_workers_env() {
    // FLUX_WORKERS=3 + default config → resolver reads env, so the
    // active scheduler should report exactly 3.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    current_worker_count()
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(source, "wc-env", &[("FLUX_WORKERS", "3")]);
    assert!(
        success,
        "FLUX_WORKERS=3 native run must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("3"),
        "expected 3 on stdout, got: {stdout:?}"
    );
}

#[test]
fn native_run_async_with_default_config_uses_resolver() {
    // worker_count = None → run_async_with(default_runtime_config(), ...)
    // exercises the `worker_count <= 0` branch in
    // `flux_async_run_root_with`, which calls `resolve_default_worker_count`.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    7
}

fn main() with IO {
    let v = run_async_with(default_runtime_config(), body)
    print(v)
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, "default", &[]);
    assert!(
        success,
        "native default-config run must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("7"),
        "expected 7 on stdout, got: {stdout:?}"
    );
}

#[test]
fn native_many_sequential_run_async_with_sites_compile_under_timeout() {
    let source = r#"
import Flow.Async exposing (..)

fn report() -> Int with Async { current_worker_count() }
fn slept() -> Int with Async { sleep(10); current_worker_count() }

fn main() with IO {
    print(run_async_with_workers(1, report))
    print(run_async_with_workers(2, report))
    print(run_async_with_workers(3, report))
    print(run_async_with_workers(4, report))
    print(run_async_with_workers(5, report))
    print(run_async_with_workers(6, report))
    print(run_async_with_workers(7, report))
    print(run_async_with_workers(8, report))
    print(run_async_with_workers(9, slept))
    print("done")
}
"#;
    let (stdout, stderr, success, elapsed) =
        run_source_with_timeout(source, "many-run-async-with", Duration::from_secs(60));
    assert!(
        success,
        "many run_async_with_workers sites must compile/run under timeout ({elapsed:?}):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "1\n2\n3\n4\n5\n6\n7\n8\n9\n\"done\"");
}

#[test]
fn native_plain_run_async_uses_resolver() {
    // Plain `run_async` path (no config) → `flux_async_run_root` →
    // `resolve_default_worker_count`. Regression for the change at
    // `flux_async_run_root` itself.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    13
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) = run_source_with_env(source, "plain", &[]);
    assert!(
        success,
        "native plain run_async must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("13"),
        "expected 13 on stdout, got: {stdout:?}"
    );
}

#[test]
fn native_flux_workers_env_var_does_not_break_default() {
    // FLUX_WORKERS=1 + default config → the env-var arm of
    // `resolve_default_worker_count` returns Some(1); program should
    // still complete and return the right value.
    let source = r#"
import Flow.Async exposing (..)

fn body() -> Int with Async {
    99
}

fn main() with IO {
    print(run_async(body))
}
"#;
    let (stdout, stderr, success, _elapsed) =
        run_source_with_env(source, "envvar", &[("FLUX_WORKERS", "1")]);
    assert!(
        success,
        "FLUX_WORKERS=1 native run must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("99"),
        "expected 99 on stdout, got: {stdout:?}"
    );
}
