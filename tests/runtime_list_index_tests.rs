//! Integration tests for runtime-dispatching list index (FLUX_OBJ_ADT case).
//!
//! The `flux_rt_index` function in `runtime/c/flux_rt.c` dispatches on the
//! heap object tag.  Before the `FLUX_OBJ_ADT` case was added, indexing a
//! cons list always fell through to `flux_hamt_get_option`, which returned
//! `None` for every key — causing programs that combine `to_list` with
//! positional access (e.g. `lines[idx]`) to silently return zeros.
//!
//! These tests verify correct VM and native-backend behaviour, plus parity
//! between the two, across a variety of indexing patterns.

#![cfg(feature = "native")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_native(fixture: &str) -> (String, bool) {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_vm(fixture: &str) -> String {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {fixture}: {e}"));

    String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}

const FIXTURE: &str = "collection_list_index.flx";

/// Native backend produces the right values for in-bounds list indexing.
#[test]
fn list_index_in_bounds_native() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    // xs[0..4] on [1,2,3,4,5]
    assert!(stdout.contains("Some(1)"), "expected Some(1), got:\n{stdout}");
    assert!(stdout.contains("Some(3)"), "expected Some(3), got:\n{stdout}");
    assert!(stdout.contains("Some(5)"), "expected Some(5), got:\n{stdout}");
}

/// Out-of-bounds and negative indices return None on native.
#[test]
fn list_index_out_of_bounds_native() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    // xs[5], xs[10], xs[-1] → None; ls[3] → None; one[1] → None
    let none_count = stdout.lines().filter(|l| l.trim() == "None").count();
    assert_eq!(
        none_count, 5,
        "expected 5 None lines (out-of-bounds + negative), got:\n{stdout}"
    );
}

/// `to_list` followed by indexed access works on native (the primary regression case).
#[test]
fn list_index_after_to_list_native() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    assert!(
        stdout.contains("Some(10)"),
        "expected Some(10) from to_list result, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Some(20)"),
        "expected Some(20) from to_list result, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Some(30)"),
        "expected Some(30) from to_list result, got:\n{stdout}"
    );
}

/// VM and native backend produce identical output for all list index patterns.
#[test]
fn list_index_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    // Strip compilation progress lines ([N of M] ...) from native output.
    let native_lines: Vec<&str> = native_out
        .lines()
        .filter(|l| !l.starts_with('['))
        .collect();

    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for list index fixture"
    );
}
