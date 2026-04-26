#![cfg(feature = "llvm")]

#[path = "../support/primop_parity.rs"]
mod primop_parity;

use primop_parity::{assert_vm_native_parity, run_native};

const FIXTURE: &str = "bitwise_primops.flx";

#[test]
fn bitwise_primops_native_backend_produces_expected_outputs() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["2", "7", "5", "12", "-4"]);
}

#[test]
fn bitwise_primops_vm_native_parity() {
    assert_vm_native_parity(FIXTURE, "bitwise primops");
}
