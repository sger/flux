//! Aether collection ownership and recursive rebuilds, on the VM and, with
//! the `llvm` feature, natively.

#[path = "../support/mod.rs"]
mod support;
use crate::support::stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn aether_collection_ownership_passes_on_the_vm_and_natively() {
    assert_backends_agree("aether_collection_ownership.flx");
}
