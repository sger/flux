//! Integration tests for `Flow.List`.
//!
//! The behavioural coverage lives in `tests/flux/stdlib_list.flx` and is
//! driven here through the `flux --test` runner. `Flow.List` is pure Flux
//! built on cons cells with no primops of its own, so what matters is that
//! every combinator behaves at the edges — the empty list, the single
//! element, predicates matching all or nothing — which the fixture asserts.
//!
//! `Flow.List` compiles to entirely different code on the two backends, and a
//! divergence in something as ordinary as `fold` would be a silent wrong
//! answer rather than a crash, so the fixture runs natively as well.

use crate::support::stdlib_fixture;
use stdlib_fixture::assert_backends_agree;

#[test]
fn stdlib_list_passes_on_the_vm_and_natively() {
    assert_backends_agree("stdlib_list.flx");
}
