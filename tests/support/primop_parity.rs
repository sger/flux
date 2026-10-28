//! Shared helpers for VM-vs-native parity tests that run a `.flx` fixture
//! from `tests/parity/` on both backends and compare their output.
//!
//! This module is included by multiple test binaries via `#[path = ...]`,
//! each of which uses a different subset of the helpers. Rust's dead-code
//! analysis runs per binary, so every helper looks unused from at least
//! one call site. Silence the structural false positive at the module
//! level.
#![allow(dead_code)]

// The runners live in `flux_runner`; they are re-exported so tests that
// include this file keep their imports.
#[path = "flux_runner.rs"]
mod flux_runner;
#[allow(unused_imports)]
pub use flux_runner::{run_native, run_native_with_env, run_vm, scratch, workspace_root};

/// Compare VM and native stdout line-by-line, filtering runtime `[...]`
/// annotations from the native path.
pub fn assert_vm_native_parity(fixture: &str, context: &str) {
    let vm_out = run_vm(fixture);
    let (native_out, success) = run_native(fixture);
    assert!(success, "native backend failed:\n{native_out}");

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();

    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for {context}"
    );
}
