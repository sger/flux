//! Shared helpers for VM-vs-native parity tests that run a `.flx` fixture
//! from `tests/parity/` on both backends and compare their output.

use std::path::Path;
use std::process::Command;

pub fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Run a parity fixture through the native/LLVM backend.
/// Returns `(trimmed stdout, process success flag)`.
pub fn run_native(fixture: &str) -> (String, bool) {
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

/// Run a parity fixture through the bytecode VM backend. Returns trimmed stdout.
pub fn run_vm(fixture: &str) -> String {
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
