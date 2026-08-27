#![cfg(feature = "llvm")]
//! Backend parity for the `Flow.map` fixture: the same tests must pass on
//! the VM and the native backend.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_map_agrees_across_backends() {
    assert_backends_agree("stdlib_map.flx");
}
