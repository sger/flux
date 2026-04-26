//! VM-vs-native parity for the algebraic-effect runtime (Proposal 0162 Phase 3).
//!
//! Each test runs a `tests/parity/effect_*.flx` fixture on both backends with
//! `FLUX_YIELD_CHECKS=1` so the native path takes the new yield-based
//! dispatch, and asserts stdout matches.
//!
#![cfg(feature = "llvm")]

#[path = "../support/primop_parity.rs"]
mod primop_parity;

use primop_parity::{run_native_with_env, run_vm};
use std::process::Command;

fn assert_parity_with_yield_checks(fixture: &str, expected: &str) {
    let vm_out = run_vm(fixture);
    let (native_out, success) = run_native_with_env(fixture, &[("FLUX_YIELD_CHECKS", "1")]);
    assert!(
        success,
        "native backend failed with FLUX_YIELD_CHECKS=1 on {fixture}:\n{native_out}"
    );

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();

    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for {fixture}"
    );
    assert_eq!(
        vm_lines.last().copied().unwrap_or_default(),
        expected,
        "unexpected final line for {fixture}"
    );
}

fn run_guide_fixture(path: &str, native: bool) -> (Vec<String>, bool) {
    let full_path = primop_parity::workspace_root().join(path);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_flux"));
    cmd.current_dir(primop_parity::workspace_root())
        .arg(full_path.to_str().unwrap())
        .arg("--no-cache");
    if native {
        cmd.arg("--native");
    }
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {path}: {e}"));
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let lines = stdout
        .lines()
        .filter(|line| !line.starts_with('['))
        .map(str::to_string)
        .collect();
    (lines, output.status.success())
}

#[test]
fn effect_handle_basic_parity() {
    assert_parity_with_yield_checks("effect_handle_basic.flx", "\"Hello, world!\"");
}

#[test]
fn effect_deep_nesting_parity() {
    assert_parity_with_yield_checks("effect_deep_nesting.flx", "\"7\"");
}

#[test]
fn effect_yield_conts_overflow_parity() {
    assert_parity_with_yield_checks("effect_yield_conts_overflow.flx", "\"11\"");
}

#[test]
fn effect_tr_loop_parity() {
    assert_parity_with_yield_checks("effect_tr_loop.flx", "\"done\"");
}

#[test]
fn effect_tr_nested_parity() {
    assert_parity_with_yield_checks("effect_tr_nested.flx", "\"42\"");
}

#[test]
fn effect_tr_reader_parity() {
    assert_parity_with_yield_checks("effect_tr_reader.flx", "\"flux-server\"");
}

#[test]
fn effect_tr_state_parity() {
    assert_parity_with_yield_checks("effect_tr_state.flx", "\"42\"");
}

#[test]
fn effect_parameterized_state_parity() {
    assert_parity_with_yield_checks("effect_parameterized_state.flx", "\"1\"");
}

#[test]
fn effect_state_parameterized_parity() {
    assert_parity_with_yield_checks("effect_state_parameterized.flx", "\"1\"");
}

#[test]
fn effect_reader_parameterized_parity() {
    assert_parity_with_yield_checks("effect_reader_parameterized.flx", "\"flux-server\"");
}

#[test]
fn effect_parameterized_console_capture_parity() {
    assert_parity_with_yield_checks("effect_parameterized_console_capture.flx", "\"visible\"");
}

#[test]
fn effect_parameterized_fallthrough_parity() {
    assert_parity_with_yield_checks("effect_parameterized_fallthrough.flx", "\"41\"");
}

#[test]
fn effect_non_tr_discard_parity() {
    assert_parity_with_yield_checks("effect_non_tr_discard.flx", "\"-1\"");
}

#[test]
fn effect_conditional_resume_parity() {
    assert_parity_with_yield_checks("effect_conditional_resume.flx", "\"100\"");
}

#[test]
fn effect_multi_shot_parity() {
    assert_parity_with_yield_checks("effect_multi_shot.flx", "\"3\"");
}

#[test]
fn guide_io_and_time_native_matches_vm() {
    let path = "examples/guide_type_system/04_with_io_and_with_time.flx";
    let (vm_lines, vm_ok) = run_guide_fixture(path, false);
    let (native_lines, native_ok) = run_guide_fixture(path, true);

    assert!(vm_ok, "VM run failed for {path}");
    assert!(native_ok, "native run failed for {path}");
    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for {path}"
    );
    assert_eq!(vm_lines, vec!["\"tick=ok\"", "\"2\""]);
}
