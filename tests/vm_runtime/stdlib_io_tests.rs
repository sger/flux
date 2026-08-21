//! Integration tests for `Flow.IoError`.
//!
//! `Flow.IoError` declares the error type every fallible OS capability reports:
//! `Result<a, IoError>`, with a machine-readable `kind` beside the human
//! message. The behavioural coverage lives in the Flux fixture; asserted here
//! are the properties that cannot be checked from inside Flux.

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

fn run_source(name: &str, source: &str) -> (String, String, bool) {
    let dir = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    let file = dir.join(name);
    std::fs::write(&file, source).expect("write scratch fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["run", file.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {name}: {e}"));

    let _ = std::fs::remove_file(&file);
    (
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
        String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
        output.status.success(),
    )
}

#[test]
fn stdlib_io_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_io.flx");
    assert!(success, "Flow.IoError test suite failed:\n{stdout}");
    assert!(
        stdout.contains("12 tests: 12 passed, 0 failed"),
        "expected all 12 Flow.IoError tests to pass, got:\n{stdout}"
    );
}

/// The whole point of a structured `kind`: a caller recovers from one failure
/// and propagates another without ever inspecting the message text.
#[test]
fn a_caller_can_branch_on_kind_without_reading_the_message() {
    let (stdout, stderr, success) = run_source(
        "io_branch_on_kind.flx",
        r#"
import Flow.IoError as Io
import Flow.Result as Result

fn recover(r: Result<Int, IoError>) -> String {
    match r {
        Ok(v) -> "ok:" + to_string(v),
        Err(e) -> match Io.is_not_found(e) {
            true -> "default",
            false -> "fatal:" + Io.kind_name(Io.error_kind(e)),
        },
    }
}

fn main() -> Unit {
    println(recover(Ok(3)))
    println(recover(Err(Io.io_error(NotFound, "gone", "/p"))))
    println(recover(Err(Io.io_error(PermissionDenied, "denied", "/p"))))
}
"#,
    );
    assert!(success, "branching on kind failed:\n{stdout}\n{stderr}");
    assert!(stdout.contains("ok:3"), "expected ok:3:\n{stdout}");
    assert!(stdout.contains("default"), "expected default:\n{stdout}");
    assert!(
        stdout.contains("fatal:PermissionDenied"),
        "expected fatal:PermissionDenied:\n{stdout}"
    );
}

/// `IoError` is a record constructor exported from a stdlib module, so callers
/// matching it directly exercise cross-module named-field patterns — the exact
/// combination that used to fail with a bogus arity mismatch.
#[test]
fn callers_may_pattern_match_the_record_across_the_module_boundary() {
    let (stdout, stderr, success) = run_source(
        "io_named_pattern.flx",
        r#"
import Flow.IoError as Io

fn main() -> Unit {
    match Io.io_error(AlreadyExists, "exists", "/d") {
        IoError { kind, message, path } ->
            println(Io.kind_name(kind) + "|" + message + "|" + path),
    }
}
"#,
    );
    assert!(
        success,
        "named-field pattern on Flow.IoError's IoError failed:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("AlreadyExists|exists|/d"),
        "expected AlreadyExists|exists|/d:\n{stdout}"
    );
}

/// `Flow.Async` declares a *positional* `IoError(Int, String, String)` of its
/// own. Importing both must not collide.
#[test]
fn flow_async_io_error_coexists_with_flow_io_error() {
    let (stdout, stderr, success) = run_source(
        "io_async_coexist.flx",
        r#"
import Flow.Async as Async
import Flow.IoError as Io

fn main() -> Unit {
    println(Io.describe(Io.io_error(NotFound, "m", "/p")))
}
"#,
    );
    assert!(
        success,
        "Flow.Async's IoError must not collide with Flow.IoError's:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("NotFound: m (/p)"),
        "expected NotFound: m (/p):\n{stdout}"
    );
}

/// `Flow.IoError` is pure — declaring and inspecting an error touches nothing, so a
/// function that only builds one needs no effect annotation.
#[test]
fn flow_io_is_effect_free() {
    let (stdout, stderr, success) = run_source(
        "io_pure.flx",
        r#"
import Flow.IoError as Io

fn classify(path: String) -> String {
    Io.describe(Io.io_error(NotFound, "missing", path))
}

fn main() -> Unit {
    println(classify("/a"))
}
"#,
    );
    assert!(success, "Flow.IoError must be pure:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("NotFound: missing (/a)"),
        "expected NotFound: missing (/a):\n{stdout}"
    );
}
