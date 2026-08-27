//! Integration driver for the `tests/flux/stdlib_option.flx` fixture.
//!
//! The behavioural coverage lives in the Flux fixture; this target runs it
//! through `flux --test` and requires every case to pass.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_fixture_passes;

#[test]
fn stdlib_option_fixture_passes_on_the_vm() {
    assert_fixture_passes("stdlib_option.flx");
}
