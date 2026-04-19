#![cfg(feature = "llvm")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_native(fixture: &str) -> String {
    let path = workspace_root()
        .join("examples")
        .join("runtime_errors")
        .join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--native", "--no-cache"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {fixture}: {e}"));

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.replace("\r\n", "\n")
}

#[test]
fn native_division_by_zero_reports_precise_span() {
    let output = run_native("division_by_zero_int.flx");
    assert!(
        output.contains("examples/runtime_errors/division_by_zero_int.flx:2:19"),
        "expected native runtime error to report precise divide span, got:\n{output}"
    );
    assert!(
        !output.contains("Stack trace:"),
        "expected native runtime error to omit synthetic stack trace, got:\n{output}"
    );
}

#[test]
fn native_modulo_by_zero_reports_precise_span() {
    let output = run_native("division_by_zero_modulo.flx");
    assert!(
        output.contains("examples/runtime_errors/division_by_zero_modulo.flx:2:19"),
        "expected native runtime error to report precise modulo span, got:\n{output}"
    );
    assert!(
        !output.contains("Stack trace:"),
        "expected native runtime error to omit synthetic stack trace, got:\n{output}"
    );
}
