#![cfg(feature = "llvm")]
//! Backend parity for the `Flow.Array` fixture.
//!
//! This target exists because array equality used to be broken in opposite
//! ways on the two backends: `==` raised `E1009` on the VM and answered
//! `false` for identical arrays natively. Every `assert_eq` over two arrays
//! therefore passed on the VM and failed natively. Running the fixture both
//! ways is what keeps that from silently returning.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_array_agrees_across_backends() {
    assert_backends_agree("stdlib_array.flx");
}
