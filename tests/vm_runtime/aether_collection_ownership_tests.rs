//! VM coverage for Aether collection ownership and recursive rebuilds.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_fixture_passes;

#[test]
fn aether_collection_ownership_fixture_passes_on_the_vm() {
    assert_fixture_passes("aether_collection_ownership.flx");
}
