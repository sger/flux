#![cfg(feature = "llvm")]
//! VM/native parity for Aether collection ownership and recursive rebuilds.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_backends_agree;

#[test]
fn aether_collection_ownership_agrees_across_backends() {
    assert_backends_agree("aether_collection_ownership.flx");
}
