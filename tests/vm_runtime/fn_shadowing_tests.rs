//! Integration driver for the `tests/flux/fn_shadowing.flx` fixture.
//!
//! KI-088's two halves both produced programs that type-checked and then
//! misbehaved — one at the compiler's contract boundary, one in the VM — so
//! the coverage has to run the program rather than compile it. The native leg
//! matters too: the nested-`fn` defect was in bytecode emission, which is the
//! VM's path, and only running both says the two agree.

#[path = "../support/mod.rs"]
mod support;
use crate::support::stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn fn_shadowing_passes_on_the_vm_and_natively() {
    assert_backends_agree("fn_shadowing.flx");
}
