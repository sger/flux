#![cfg(feature = "llvm")]
//! Backend parity for the `Flow.List` fixture.
//!
//! `Flow.List` compiles to entirely different code on the two backends, and a
//! divergence in something as ordinary as `fold` would be a silent wrong
//! answer rather than a crash. Running the same fixture both ways is the
//! cheapest way to keep the two honest.

#[path = "../support/stdlib_fixture.rs"]
mod stdlib_fixture;

use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_list_agrees_across_backends() {
    assert_backends_agree("stdlib_list.flx");
}
