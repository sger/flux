//! Flow.Task surface integration tests (proposal 0174 Phase 1a-vi follow-up).
//!
//! Drives the full `flux` CLI rather than the bare [`Compiler`] because
//! [`lib/Flow/Task.flx`](../../lib/Flow/Task.flx) only resolves through
//! the driver's stdlib root.
//!
//! The fixture in [`tests/flux/flow_task_surface.flx`] runs through
//! `flux --test` and proves the positive type-level surface for `Int`,
//! `List<Int>`, tuples, and `cancel<a>` (no `Sendable` bound).
//!
//! **Negative-case caveat.** The inline `Sendable` solver does fail
//! correctly when a constrained generic is *defined locally* and applied
//! to a function type (this is what [`tests/type_inference/sendable_tests.rs`]
//! verifies). When the same constrained generic is **defined in a module
//! and called through an import**, the constraint solver currently does
//! not flag the function-type case at the call site — a pre-existing
//! compiler gap unrelated to this slice. So the runtime safety of
//! `Task.spawn` against function-typed payloads still rests on the
//! eventual native FFI bridge / Aether boundary, not the type system, in
//! that path. A separate slice will close the cross-module class-bound
//! enforcement gap; until then, the positive surface tests are the
//! load-bearing assertion.
//!
//! Phase 1a-vi follow-up scope: type-level surface only. The runtime FFI
//! that would let `spawn`/`blocking_join`/`cancel` actually run on workers
//! is a later slice; today the bodies panic.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_test(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("flux").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args(["--test", path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --test on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

#[test]
fn flow_task_surface_compiles_and_passes() {
    let (stdout, success) = run_flux_test("flow_task_surface.flx");
    assert!(success, "Flow.Task surface tests must pass:\n{stdout}");
    assert!(
        stdout.contains("6 passed"),
        "expected 6 passing tests, got:\n{stdout}"
    );
}
