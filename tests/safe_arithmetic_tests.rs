//! Integration tests for safe_div and safe_mod (Proposal 0135 Phase 1).
//!
//! Verifies that these total arithmetic functions return `Some(result)` on
//! valid inputs and `None` on division/modulo by zero, on both the VM and
//! native LLVM backends, with output parity between them.

#![cfg(feature = "native")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_native(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_vm(fixture: &str) -> String {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {fixture}: {e}"));

    String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}

const FIXTURE: &str = "safe_arithmetic.flx";

#[test]
fn safe_div_returns_some_on_valid_input() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    assert!(
        stdout.contains("Some(3)"),
        "safe_div(10, 3) should be Some(3)"
    );
    assert!(
        stdout.contains("Some(-5)"),
        "safe_div(-15, 3) should be Some(-5)"
    );
}

#[test]
fn safe_div_returns_none_on_zero() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    // The first None in the output is from safe_div(10, 0)
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[1], "None", "safe_div(10, 0) should be None");
}

#[test]
fn safe_mod_returns_some_on_valid_input() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    assert!(
        stdout.contains("Some(1)"),
        "safe_mod(10, 3) should be Some(1)"
    );
}

#[test]
fn safe_mod_returns_none_on_zero() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    // safe_mod(10, 0) is the 6th line (index 5)
    assert_eq!(lines[5], "None", "safe_mod(10, 0) should be None");
}

#[test]
fn safe_div_pattern_matching_works() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    assert!(
        stdout.contains("100/4 = 25"),
        "pattern matching on Some should work"
    );
    assert!(
        stdout.contains("division by zero handled"),
        "pattern matching on None should work"
    );
}

#[test]
fn safe_arithmetic_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();

    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for safe arithmetic"
    );
}
