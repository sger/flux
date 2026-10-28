//! Integration tests for NonZero type-safe division (Proposal 0135 Phase 2).

// Gated on `llvm`, not `native`: `llvm = ["native"]` but not the reverse, so
// `--features native` alone compiles these and then fails at runtime with
// "native backend features require `llvm`".
#![cfg(feature = "llvm")]

#[path = "../support/mod.rs"]
mod support;
use crate::support::flux_runner;
use flux_runner::{run_native, run_vm};

const FIXTURE: &str = "non_zero_arithmetic.flx";

#[test]
fn non_zero_smart_constructor_and_division() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "5", "from_non_zero(non_zero(5)) should be 5");
    assert_eq!(lines[1], "\"rejected\"", "non_zero(0) should be None");
    assert_eq!(lines[2], "25", "div_nz(100, 4) should be 25");
    assert_eq!(lines[3], "0", "div_nz(0, 4) should be 0");
    assert_eq!(lines[4], "1", "mod_nz(17, 4) should be 1");
}

#[test]
fn non_zero_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");
    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();
    assert_eq!(vm_lines, native_lines, "VM and native output differ for NonZero arithmetic");
}
