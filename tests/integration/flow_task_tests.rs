//! Flow.Task surface integration tests (proposal 0174 Phase 1a-vi follow-up).
//!
//! Drives the full `flux` CLI rather than the bare [`Compiler`] because
//! [`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) only resolves through
//! the driver's stdlib root.
//!
//! The fixture in [`tests/flux/flow_task_surface.flx`] runs through
//! `flux --test` and proves the positive type-level surface for `Int`,
//! `List<Int>`, tuples, and `cancel<a>` (no `Sendable` bound).
//!
//! D1 closes the cross-module class-bound gap for `Task.spawn<a: Sendable>`:
//! function-typed payloads are rejected through the imported `Flow.Task`
//! surface, while concrete sendable payloads still type-check.
//!
//! Phase 1a-vi follow-up scope: type-level surface only. The runtime FFI
//! that would let `spawn`/`blocking_join`/`cancel` actually run on workers
//! is a later slice; today the bodies panic.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_flux_source(source: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!(
        "flux-flow-task-d1-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for Flow.Task D1 fixture");
    let path = dir.join("flow_task_d1.flx");
    std::fs::write(&path, source).expect("write Flow.Task D1 fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on Flow.Task D1 fixture");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

#[test]
fn flow_task_surface_compiles_and_passes() {
    let (stdout, success) = run_flux_test("flow_task_surface.flx");
    assert!(success, "Flow.Task surface tests must pass:\n{stdout}");
    assert!(
        stdout.contains("6 passed"),
        "expected 6 passing tests, got:\n{stdout}"
    );
}

#[test]
fn flow_task_spawn_rejects_non_sendable_function_payload_cross_module() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn main() {
    Task.spawn(fn() { fn(x) { x } })
}
"#,
    );

    assert!(
        !success,
        "Task.spawn must reject a function payload through the imported Flow.Task surface"
    );
    assert!(
        stderr.contains("E444") && stderr.contains("Sendable"),
        "expected E444 Sendable diagnostic, got:\n{stderr}"
    );
}

#[test]
#[cfg(feature = "llvm")]
fn flow_task_native_compiles_and_passes() {
    // Uses a native-specific fixture (Int/String payloads only) because
    // List<T> and tuple assert_eq is not yet supported on the native backend.
    let path = workspace_root()
        .join("tests")
        .join("flux")
        .join("flow_task_native.flx");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test --native: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    assert!(
        output.status.success(),
        "native Task tests must pass:\n{stdout}"
    );
    assert!(
        stdout.contains("4 passed"),
        "expected 4 passing native tests, got:\n{stdout}"
    );
}

#[test]
fn flow_task_spawn_accepts_sendable_int_payload_cross_module() {
    let (_stdout, stderr, success) = run_flux_source(
        r#"
import Flow.Task as Task

fn main() {
    let _ = Task.spawn(fn() { 42 });
    ()
}
"#,
    );

    assert!(
        success,
        "Task.spawn should accept an Int payload through the imported Flow.Task surface:\n{stderr}"
    );
}
