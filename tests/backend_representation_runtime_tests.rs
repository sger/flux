//! Integration tests for backend representation-family parity.

#![cfg(feature = "native")]

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
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

fn normalized_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| !line.starts_with('['))
        .collect()
}

#[test]
fn array_does_not_match_list_pattern_on_native() {
    let (stdout, success) = run_native("backend_repr_array_not_list.flx");
    assert!(success, "native backend failed:\n{stdout}");
    assert_eq!(normalized_lines(&stdout), vec!["\"array\""]);
}

#[test]
fn tuple_does_not_match_option_pattern_on_native() {
    let (stdout, success) = run_native("backend_repr_tuple_not_option.flx");
    assert!(success, "native backend failed:\n{stdout}");
    assert_eq!(normalized_lines(&stdout), vec!["\"tuple\""]);
}

#[test]
fn representation_fixtures_hold_vm_native_parity() {
    for fixture in [
        "backend_repr_array_not_list.flx",
        "backend_repr_tuple_not_option.flx",
        "backend_repr_list_match_ok.flx",
        "backend_repr_array_api_ok.flx",
    ] {
        let vm_out = run_vm(fixture);
        let (native_out, success) = run_native(fixture);
        assert!(
            success,
            "native backend failed for {fixture}:\n{native_out}"
        );
        assert_eq!(
            vm_out.lines().collect::<Vec<_>>(),
            normalized_lines(&native_out),
            "VM and native output differ for {fixture}"
        );
    }
}
