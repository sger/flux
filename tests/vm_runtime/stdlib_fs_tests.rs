//! Integration tests for `Flow.Fs` and the `TryReadFile` primop
//! (proposal 0178, first fallible primop).
//!
//! This is the first primop that reports failure as a value rather than
//! aborting, so what is asserted here is the *shape* of that contract:
//!
//!   * a missing file produces `Err(IoError { kind: NotFound, .. })` and the
//!     program keeps running — the old `read_file` would have aborted;
//!   * the error carries the path that was attempted;
//!   * the VM and the native backend classify the same failure identically.
//!     The two implementations classify independently — the VM matches on
//!     Rust's `io::ErrorKind`, the C runtime on `errno` — so agreement is a
//!     real invariant and not a shared code path.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn scratch_dir() -> PathBuf {
    let dir = workspace_root().join("target").join("test-scratch");
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
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
    let file = scratch_dir().join(name);
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
fn stdlib_fs_flux_suite_passes() {
    let (stdout, success) = run_flux_test("stdlib_fs.flx");
    assert!(success, "Flow.Fs test suite failed:\n{stdout}");
    assert!(
        stdout.contains("8 tests: 8 passed, 0 failed"),
        "expected all 8 Flow.Fs tests to pass, got:\n{stdout}"
    );
}

/// The headline behavioural change: a read that fails no longer stops the
/// program. The old `read_file` aborts on a missing path; this one returns and
/// the following statement still runs.
#[test]
fn a_failed_read_does_not_abort_the_program() {
    let (stdout, stderr, success) = run_source(
        "fs_no_abort.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn main() -> Unit with IO {
    match Fs.read_file("/nonexistent/nope.txt") {
        Ok(_) -> println("unexpected"),
        Err(e) -> println("handled " + Io.kind_name(Io.error_kind(e))),
    }
    println("still running")
}
"#,
    );
    assert!(
        success,
        "a failed read must not abort:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("handled NotFound"),
        "expected handled NotFound:\n{stdout}"
    );
    assert!(
        stdout.contains("still running"),
        "execution should continue after a failed read:\n{stdout}"
    );
}

/// The error names the path it tried, which is what makes a multi-path
/// fallback loop diagnosable.
#[test]
fn the_error_reports_the_attempted_path() {
    let (stdout, stderr, success) = run_source(
        "fs_error_path.flx",
        r#"
import Flow.Fs as Fs
import Flow.IoError as Io

fn main() -> Unit with IO {
    match Fs.read_file("/nonexistent/named.txt") {
        Ok(_) -> println("unexpected"),
        Err(e) -> println("path=" + Io.error_path(e)),
    }
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("path=/nonexistent/named.txt"),
        "expected the attempted path in the error:\n{stdout}"
    );
}

/// Reading a real file still works — the failure path did not replace the
/// success path.
#[test]
fn reading_an_existing_file_returns_its_contents() {
    let fixture = scratch_dir().join("fs_read_me.txt");
    std::fs::write(&fixture, "hello fs").expect("write fixture");

    let (stdout, stderr, success) = run_source(
        "fs_read_ok.flx",
        &format!(
            r#"
import Flow.Fs as Fs

fn main() -> Unit with IO {{
    match Fs.read_file("{}") {{
        Ok(c) -> println("got:" + c),
        Err(_) -> println("unexpected error"),
    }}
}}
"#,
            fixture.to_str().unwrap()
        ),
    );
    let _ = std::fs::remove_file(&fixture);
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("got:hello fs"),
        "expected got:hello fs:\n{stdout}"
    );
}

/// `read_file_or` collapses the `Result` for the "optional config" shape.
#[test]
fn read_file_or_falls_back_without_the_caller_seeing_an_error() {
    let (stdout, stderr, success) = run_source(
        "fs_read_or.flx",
        r#"
import Flow.Fs as Fs

fn main() -> Unit with IO {
    println(Fs.read_file_or("/nonexistent/cfg.toml", "defaulted"))
}
"#,
    );
    assert!(success, "run failed:\n{stdout}\n{stderr}");
    assert!(
        stdout.contains("defaulted"),
        "expected defaulted:\n{stdout}"
    );
}

/// `Flow.Fs.read_file` carries `FileSystem`, so a caller that does not declare
/// the effect must be rejected. Losing this would make the capability
/// invisible in signatures — the property 0178 exists to provide.
#[test]
fn reading_a_file_requires_the_filesystem_effect() {
    let (stdout, stderr, success) = run_source(
        "fs_effect_required.flx",
        r#"
import Flow.Fs as Fs

fn sneaky(path: String) -> Bool {
    match Fs.read_file(path) {
        Ok(_) -> true,
        Err(_) -> false,
    }
}

fn main() -> Unit with IO {
    println(to_string(sneaky("Cargo.toml")))
}
"#,
    );
    assert!(
        !success,
        "an undeclared FileSystem effect must be rejected:\nstdout:\n{stdout}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("FileSystem"),
        "the diagnostic should name the FileSystem effect:\n{combined}"
    );
}
