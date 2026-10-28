//! Integration tests for tail-recursive List stdlib functions.
//!
//! Verifies that map, filter, take, take_while handle large lists
//! without stack overflow on the VM backend.

#[path = "../support/mod.rs"]
mod support;
use crate::support::flux_runner;
use flux_runner::run_flux_test;

#[test]
fn stdlib_list_large_vm() {
    let (stdout, success) = run_flux_test("stdlib_list_large.flx");
    assert!(success, "list stdlib tests failed (likely stack overflow):\n{stdout}");
    assert!(stdout.contains("8 passed"), "expected all 8 tests to pass, got:\n{stdout}");
}
