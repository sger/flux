//! Multi-shot effect-handler behavior across backends.
//!
//! A clause that invokes `resume` more than once is a multi-shot handler.
//! The maintained VM and default native yield path both support this shape:
//!
//! - **VM**: non-tail `resume(v)` runs the captured continuation to the
//!   handler boundary, returns that result to the handler arm, and leaves the
//!   continuation reusable for the second resume.
//! - **Native, legacy opt-out (`FLUX_YIELD_CHECKS=0`)**:
//!   `flux_perform_direct`'s `flux_resume_called` counter detects
//!   multi-shot and reports a structured E1201. Exits non-zero.
//! - **Native, default yield-based path**: the prompt loop handles
//!   re-yields from resume re-entries (Proposal 0162 Phase 3 slice
//!   5-tr-fix + 5-tr-nested). For `resume(true) + resume(false)` the
//!   handler correctly evaluates both branches and prints `"3"`. Exits
//!   zero.
//!
#![cfg(feature = "llvm")]

#[path = "../support/primop_parity.rs"]
mod primop_parity;

use primop_parity::{run_native, run_native_with_env};
use std::path::Path;
use std::process::Command;

const FIXTURE: &str = "effect_multi_shot.flx";

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run the VM and return `(combined stdout+stderr, exit-success flag)`.
fn run_vm_with_status() -> (String, bool) {
    let path = workspace_root().join("tests").join("parity").join(FIXTURE);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {FIXTURE}: {e}"));
    let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (combined, output.status.success())
}

fn final_output_line(out: &str) -> &str {
    out.lines()
        .filter(|line| !line.starts_with('['))
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
}

#[test]
fn vm_prints_3_on_multi_shot() {
    let (out, ok) = run_vm_with_status();
    assert!(ok, "VM must handle multi-shot resume cleanly, got:\n{out}");
    assert_eq!(final_output_line(&out), "\"3\"");
}

#[test]
fn native_legacy_opt_out_exits_nonzero_on_multi_shot() {
    let (_out, ok) = run_native_with_env(FIXTURE, &[("FLUX_YIELD_CHECKS", "0")]);
    assert!(
        !ok,
        "native backend (legacy, FLUX_YIELD_CHECKS=0) must report E1201 on multi-shot, \
         but exited cleanly"
    );
}

#[test]
fn native_default_prints_3_on_multi_shot() {
    let (out, ok) = run_native(FIXTURE);
    assert!(
        ok,
        "native backend (default yield path) must handle multi-shot cleanly, got:\n{out}"
    );
    assert_eq!(
        final_output_line(&out),
        "\"3\"",
        "native yield path should print 3 (= resume(true)=1 + resume(false)=2)"
    );
}
