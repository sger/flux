//! Integration tests for mutually recursive nested functions.
//!
//! Verifies that the sibling reconstruction approach works correctly
//! on the VM backend for 2-way, 3-way, and captured-variable mutual
//! recursion groups.

#[path = "../support/mod.rs"]
mod support;
use crate::support::flux_runner;
use flux_runner::run_flux_test;

#[test]
fn mutual_recursion_vm() {
    let (stdout, success) = run_flux_test("mutual_recursion.flx");
    assert!(success, "mutual recursion tests failed:\n{stdout}");
    assert!(stdout.contains("11 passed"), "expected all 6 tests to pass, got:\n{stdout}");
}
