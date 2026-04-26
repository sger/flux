//! Integration tests for safe_div and safe_mod (Proposal 0135 Phase 1).
//!
//! Verifies that these total arithmetic functions return `Some(result)` on
//! valid inputs and `None` on division/modulo by zero, on both the VM and
//! native LLVM backends, with output parity between them.

#![cfg(feature = "native")]

#[path = "../support/primop_parity.rs"]
mod primop_parity;

use primop_parity::{assert_vm_native_parity, run_native};

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
    assert_vm_native_parity(FIXTURE, "safe arithmetic");
}
