//! Integration tests for NonZero type-safe division (Proposal 0135 Phase 2).

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

const FIXTURE: &str = "non_zero_arithmetic.flx";

#[test]
fn non_zero_smart_constructor_and_division() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "5", "from_non_zero(non_zero(5)) should be 5");
    assert_eq!(lines[1], "\"rejected\"", "non_zero(0) should be None");
    assert_eq!(lines[2], "25", "div_nz(100, 4) should be 25");
    assert_eq!(lines[3], "0", "div_nz(0, 4) should be 0");
    assert_eq!(lines[4], "1", "mod_nz(17, 4) should be 1");
}

#[test]
fn non_zero_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");
    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();
    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for NonZero arithmetic"
    );
}
