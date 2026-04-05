//! Integration tests for mutually recursive nested functions.
//!
//! Verifies that the sibling reconstruction approach works correctly
//! on the VM backend for 2-way, 3-way, and captured-variable mutual
//! recursion groups.

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
fn mutual_recursion_vm() {
    let (stdout, success) = run_flux_test("mutual_recursion.flx");
    assert!(
        success,
        "mutual recursion tests failed:\n{stdout}"
    );
    assert!(
        stdout.contains("6 passed"),
        "expected all 6 tests to pass, got:\n{stdout}"
    );
}
