#![cfg(feature = "llvm")]

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

const FIXTURE: &str = "bitwise_primops.flx";

#[test]
fn bitwise_primops_native_backend_produces_expected_outputs() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["2", "7", "5", "12", "-4"]);
}

#[test]
fn bitwise_primops_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> = native_out.lines().filter(|l| !l.starts_with('[')).collect();

    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for bitwise primops"
    );
}
