//! Integration tests for stackless flux_drop in the native LLVM backend.
//!
//! These tests verify that deeply nested / long data structures can be
//! freed without stack overflow.  The stackless drop traversal uses the
//! FluxHeader._reserved field to store field progress and overwrites
//! field[0] with the parent pointer (Koka-style parent chain).
//!
//! Each test runs a Flux program through the native backend (`--native`)
//! and checks for correct output + clean exit (no crash / STATUS_STACK_OVERFLOW).

#![cfg(feature = "native")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run a Flux program with the native backend and return (stdout, exit_success).
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

/// Run a Flux program with the VM backend and return stdout.
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

#[test]
fn stackless_drop_deep_list_native() {
    let (stdout, success) = run_native("stackless_drop_deep_list.flx");
    assert!(
        success,
        "native backend crashed (likely stack overflow in flux_drop)"
    );
    assert!(
        stdout.contains("ok"),
        "expected 'ok' in output, got: {stdout}"
    );
}

#[test]
fn stackless_drop_deep_list_parity() {
    let vm_out = run_vm("stackless_drop_deep_list.flx");
    let (native_out, success) = run_native("stackless_drop_deep_list.flx");
    assert!(success, "native backend crashed");
    // Compare just the program output lines (skip compilation messages).
    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();
    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for deep list"
    );
}

#[test]
fn stackless_drop_nested_adt_native() {
    let (stdout, success) = run_native("stackless_drop_nested_adt.flx");
    assert!(
        success,
        "native backend crashed (likely stack overflow in flux_drop)"
    );
    assert!(
        stdout.contains("150000"),
        "expected depth=150000 in output, got: {stdout}"
    );
}

#[test]
fn stackless_drop_nested_adt_parity() {
    let vm_out = run_vm("stackless_drop_nested_adt.flx");
    let (native_out, success) = run_native("stackless_drop_nested_adt.flx");
    assert!(success, "native backend crashed");
    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();
    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for nested ADT"
    );
}
