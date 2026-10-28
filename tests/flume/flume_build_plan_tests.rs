//! VM/native parity and fake-interpreter coverage for Flume.Build.Plan.

#[path = "../support/mod.rs"]
mod support;
use crate::support::stdlib_fixture;

use stdlib_fixture::assert_backends_agree;

#[test]
fn flume_build_plan_fixture_passes_on_both_backends() {
    assert_backends_agree("flume_build_plan.flx");
}
