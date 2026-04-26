#![cfg(feature = "llvm")]

#[path = "../support/primop_parity.rs"]
mod primop_parity;

use primop_parity::{assert_vm_native_parity, run_native};

const FIXTURE: &str = "math_primops.flx";

#[test]
fn math_primops_native_backend_produces_expected_outputs() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["3", "0", "1", "1", "0", "3", "4", "4"]);
}

#[test]
fn math_primops_vm_native_parity() {
    assert_vm_native_parity(FIXTURE, "math primops");
}
