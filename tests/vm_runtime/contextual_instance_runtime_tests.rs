// This suite launches both the VM and `flux --native`; the latter requires
// the LLVM feature even though the VM path is also exercised here.
#![cfg(feature = "llvm")]

#[path = "../support/flux_runner.rs"]
mod flux_runner;
use flux_runner::{run_native, run_vm};

const FIXTURE: &str = "contextual_instance_eq_list.flx";

#[test]
fn contextual_instance_list_fixture_native_outputs_expected_results() {
    let (stdout, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{stdout}");

    let lines: Vec<&str> = stdout.lines().filter(|line| !line.starts_with('[')).collect();
    assert_eq!(lines, vec!["true", "false"]);
}

#[test]
fn contextual_instance_list_fixture_vm_native_parity() {
    let vm_out = run_vm(FIXTURE);
    let (native_out, success) = run_native(FIXTURE);
    assert!(success, "native backend failed:\n{native_out}");

    let vm_lines: Vec<&str> = vm_out.lines().collect();
    let native_lines: Vec<&str> =
        native_out.lines().filter(|line| !line.starts_with('[')).collect();
    assert_eq!(
        vm_lines, native_lines,
        "VM and native output differ for contextual instance fixture"
    );
}
