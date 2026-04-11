//! Integration tests for tail-recursive List stdlib functions.
//!
//! Verifies that map, filter, take, take_while handle large lists
//! without stack overflow on the VM backend.

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
fn stdlib_list_large_vm() {
    let (stdout, success) = run_flux_test("stdlib_list_large.flx");
    assert!(
        success,
        "list stdlib tests failed (likely stack overflow):\n{stdout}"
    );
    assert!(
        stdout.contains("8 passed"),
        "expected all 8 tests to pass, got:\n{stdout}"
    );
}
