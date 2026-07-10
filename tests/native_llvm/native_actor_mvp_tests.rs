//! Actor MVP end-to-end tests (proposal 0177 T4.4) — native (LLVM) twin.
//!
//! Runs the shipped `examples/actors/*.flx` under `--native` and asserts the
//! same documented output as the VM half (`tests/integration/actor_mvp.rs`),
//! giving example-level VM↔native parity on top of the gated
//! `tests/parity/async_actor_receive_reply.flx` fixture.
//!
//! The deterministic-scheduler ordering tests are VM-only: native threads
//! `det_seed` through its ABI but does not yet honor it (proposal 0177 T6.3).

#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_example_native(rel_path: &str) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([rel_path, "--native", "--no-cache"])
        .output()
        .expect("run flux --native");
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

fn assert_example(rel_path: &str, expected: &str) {
    let (stdout, stderr, ok) = run_example_native(rel_path);
    assert!(
        ok,
        "{rel_path} must succeed on native:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), expected, "{rel_path} native output");
}

#[test]
fn actor_counter_example_native() {
    assert_example("examples/actors/counter.flx", "\"count = 2\"");
}

#[test]
fn actor_ping_pong_example_native() {
    assert_example("examples/actors/ping_pong.flx", "\"rallies = 3\"");
}

#[test]
fn actor_fan_out_example_native() {
    assert_example("examples/actors/fan_out.flx", "\"sum of squares = 30\"");
}

/// KI-6 twin: `stop` an actor parked in `receive`, then return from
/// `run_async` immediately. This shape used to SIGSEGV on native.
#[test]
fn actor_stop_then_immediate_return_native() {
    let source = r#"
import Flow.Async exposing (..)
import Flow.Channel as Channel
import Flow.Channel exposing (Channel)
import Flow.Actor exposing (..)

fn looping(mb: Mailbox<Int>, acc: Int) -> Unit with Async {
    let x = receive(mb)
    looping(mb, acc + x)
}

fn body() -> Int with Async {
    let c = spawn(fn(mb) { looping(mb, 0) })
    tell(c, 1)
    yield_now()
    stop(c)
    7
}

fn main() with IO { print(to_string(run_async(body))) }
"#;
    let dir = workspace_root()
        .join("target")
        .join("test-scratch")
        .join(format!("flux-native-ki6-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("ki6_actor_stop.flx");
    std::fs::write(&path, source).expect("write fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .expect("run flux --native");
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "native stop-then-exit must succeed (KI-6):\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "\"7\"", "native stop-then-exit output");
}
