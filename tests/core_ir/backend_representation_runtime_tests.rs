//! Integration tests for backend representation-family parity.

// Gated on `llvm`, not `native`: `llvm = ["native"]` but not the reverse, so
// `--features native` alone compiles these and then fails at runtime with
// "native backend features require `llvm`".
#![cfg(feature = "llvm")]

use crate::support::flux_runner;
use flux_runner::{flux_file, run_native, run_vm};

fn normalized_lines(output: &str) -> Vec<&str> {
    output.lines().filter(|line| !line.starts_with('[')).collect()
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
        assert!(success, "native backend failed for {fixture}:\n{native_out}");
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
    let rel = format!("tests/parity/{fixture}");
    let vm = flux_file(&rel).run();
    let native = flux_file(&rel).native().run();
    let (vm_stderr, vm_success) = (vm.stderr.trim(), vm.success);
    let (native_stderr, native_success) = (native.stderr.trim(), native.success);

    assert!(!vm_success, "VM unexpectedly succeeded:\n{vm_stderr}");
    assert!(!native_success, "native backend unexpectedly succeeded:\n{native_stderr}");
    assert!(
        vm_stderr.contains("index operator not supported: None"),
        "expected VM index error, got:\n{vm_stderr}"
    );
    assert!(
        native_stderr.contains("index operator not supported: None"),
        "expected native index error, got:\n{native_stderr}"
    );
}
