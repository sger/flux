//! Integration tests for backend representation-family parity.

#![cfg(feature = "native")]

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn unique_run_dir(fixture: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let stem = Path::new(fixture)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("fixture");
    let pid = std::process::id();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("flux_native_{stem}_{pid}_{counter}"))
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
    let run_dir = unique_run_dir(fixture);
    let _ = std::fs::create_dir_all(&run_dir);
    let output_path = run_dir.join("program");
    let cache_dir = run_dir.join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            path.to_str().unwrap(),
            "--native",
            "--no-cache",
            "-o",
            output_path.to_string_lossy().as_ref(),
            "--cache-dir",
            cache_dir.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, output.status.success())
}

fn run_native_full(fixture: &str) -> (String, String, bool) {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let run_dir = unique_run_dir(fixture);
    let _ = std::fs::create_dir_all(&run_dir);
    let output_path = run_dir.join("program");
    let cache_dir = run_dir.join("cache");
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([
            path.to_str().unwrap(),
            "--native",
            "--no-cache",
            "-o",
            output_path.to_string_lossy().as_ref(),
            "--cache-dir",
            cache_dir.to_string_lossy().as_ref(),
        ])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --native on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, stderr, output.status.success())
}

fn run_vm_full(fixture: &str) -> (String, String, bool) {
    let path = workspace_root().join("tests").join("parity").join(fixture);
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux on {fixture}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .replace("\r\n", "\n")
        .trim()
        .to_string();
    (stdout, stderr, output.status.success())
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
        "backend_repr_option_named_field_access.flx",
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

#[test]
fn invalid_index_target_fails_consistently_on_vm_and_native() {
    let fixture = "index_none_runtime_error.flx";
    let (_vm_stdout, vm_stderr, vm_success) = run_vm_full(fixture);
    let (_native_stdout, native_stderr, native_success) = run_native_full(fixture);

    assert!(!vm_success, "VM unexpectedly succeeded:\n{vm_stderr}");
    assert!(
        !native_success,
        "native backend unexpectedly succeeded:\n{native_stderr}"
    );
    assert!(
        vm_stderr.contains("index operator not supported: None"),
        "expected VM index error, got:\n{vm_stderr}"
    );
    assert!(
        native_stderr.contains("index operator not supported: None"),
        "expected native index error, got:\n{native_stderr}"
    );
}
