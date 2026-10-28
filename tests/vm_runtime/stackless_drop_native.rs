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

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use flux_runner::{run_native, run_vm};

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
