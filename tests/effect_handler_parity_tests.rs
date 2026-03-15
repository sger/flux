//! VM/JIT parity tests for effect handlers.
//!
//! Verifies that tail-resumptive (direct dispatch) and non-tail-resumptive
//! (continuation-based) handlers produce identical results on both backends.

#![cfg(feature = "jit")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn run_flux(file: &str, jit: bool) -> (i32, String, String) {
    let mut args = vec!["--no-cache", file];
    if jit {
        args.push("--jit");
    }
    let output = Command::new(flux_bin())
        .current_dir(workspace_root())
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux for `{file}` (jit={jit}): {e}"));

    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    (status, stdout, stderr)
}

fn assert_effect_parity(file: &str) {
    let (vm_rc, vm_out, vm_err) = run_flux(file, false);
    let (jit_rc, jit_out, jit_err) = run_flux(file, true);

    assert_eq!(
        vm_rc, jit_rc,
        "{file}: exit code mismatch (vm={vm_rc}, jit={jit_rc})\nVM stderr:\n{vm_err}\nJIT stderr:\n{jit_err}"
    );
    assert_eq!(
        vm_out, jit_out,
        "{file}: stdout mismatch\nVM:  {vm_out}\nJIT: {jit_out}"
    );
}

// ── Tail-resumptive handlers (direct dispatch) ──────────────────────────

#[test]
fn effect_parity_tr_logging() {
    assert_effect_parity("tests/flux/effect_tr_state.flx");
}

#[test]
fn effect_parity_tr_reader() {
    assert_effect_parity("tests/flux/effect_tr_reader.flx");
}

#[test]
fn effect_parity_tr_nested_handlers() {
    assert_effect_parity("tests/flux/effect_tr_nested.flx");
}

#[test]
fn effect_parity_tr_loop() {
    assert_effect_parity("tests/flux/effect_tr_loop.flx");
}

// ── Non-tail-resumptive handlers (continuation path) ────────────────────

#[test]
fn effect_parity_non_tr_exception() {
    assert_effect_parity("tests/flux/effect_non_tr_exception.flx");
}

// ── Existing effect examples ────────────────────────────────────────────

#[test]
fn effect_parity_handle_basic() {
    assert_effect_parity("examples/type_system/18_handle_basic.flx");
}

#[test]
fn effect_parity_handle_discharges() {
    assert_effect_parity("examples/type_system/22_handle_discharges_effect.flx");
}

#[test]
fn effect_parity_main_handles_custom() {
    assert_effect_parity("examples/type_system/29_main_handles_custom_effect.flx");
}

#[test]
fn effect_parity_guide_basics() {
    assert_effect_parity("examples/guide_type_system/05_perform_handle_basics.flx");
}
