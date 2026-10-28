//! Integration driver for the `tests/flux/generics.flx` fixture.
//!
//! The generics corpus in `examples/generics/working/` is pinned by a
//! compile-only snapshot, which proves those programs are accepted. The
//! behavioural coverage lives in the Flux fixture: it proves a generic
//! definition used at two types produces the right value at each, and that the
//! VM and the native backend agree about it.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn generics_passes_on_the_vm_and_natively() {
    assert_backends_agree("generics.flx");
}
