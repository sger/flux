//! Integration driver for the `tests/flux/stdlib_array.flx` fixture.
//!
//! The fixture runs on the VM and, with the `llvm` feature, natively. Array
//! equality used to be broken in opposite ways on the two backends: `==`
//! raised `E1009` on the VM and answered `false` for identical arrays
//! natively, so every `assert_eq` over two arrays passed on one and failed on
//! the other. Running it both ways keeps that from silently returning.

use crate::support::stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_array_passes_on_the_vm_and_natively() {
    assert_backends_agree("stdlib_array.flx");
}
