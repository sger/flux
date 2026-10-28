//! Integration driver for the `tests/flux/stdlib_option.flx` fixture.
//!
//! The behavioural coverage lives in the Flux fixture; this target runs it
//! through `flux --test` on the VM and, with the `llvm` feature, natively,
//! and requires every case to pass on both.

use crate::support::stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_option_passes_on_the_vm_and_natively() {
    assert_backends_agree("stdlib_option.flx");
}
