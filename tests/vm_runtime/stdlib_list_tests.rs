//! Integration tests for `Flow.List`.
//!
//! The behavioural coverage lives in `tests/flux/stdlib_list.flx` and is
//! driven here through the `flux --test` runner. `Flow.List` is pure Flux
//! built on cons cells with no primops of its own, so what matters is that
//! every combinator behaves at the edges — the empty list, the single
//! element, predicates matching all or nothing — which the fixture asserts.
//!
//! Native parity for the same fixture is covered by
//! `tests/native_llvm/stdlib_list_native_tests.rs`, which is gated on the
//! `llvm` feature.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_fixture_passes;

#[test]
fn stdlib_list_fixture_passes_on_the_vm() {
    assert_fixture_passes("stdlib_list.flx");
}
