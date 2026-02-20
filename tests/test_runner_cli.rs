use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path(name: &str) -> PathBuf {
    workspace_root()
        .join("tests")
        .join("testdata")
        .join("test_runner")
        .join(name)
}

fn run_flux(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args))
}

fn combined_output(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[test]
fn test_mode_no_tests_returns_success() {
    let file = fixture_path("no_tests.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success status, output:\n{}",
        text
    );
    assert!(
        text.contains("No test functions found"),
        "expected no-tests message, output:\n{}",
        text
    );
}

#[test]
fn test_mode_non_zero_arity_fails_but_continues() {
    let file = fixture_path("arity_failure.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        !output.status.success(),
        "expected failure, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_ok"),
        "expected passing test to run, output:\n{}",
        text
    );
    assert!(
        text.contains("FAIL  test_needs_arg"),
        "expected arity test failure, output:\n{}",
        text
    );
    assert!(
        text.contains("wrong number of arguments"),
        "expected arity error details, output:\n{}",
        text
    );
}

#[test]
fn test_mode_discovers_tests_module_and_top_level() {
    let file = fixture_path("module_discovery.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success, output:\n{}",
        text
    );
    assert!(
        text.contains("[Tests]"),
        "expected Tests group header, output:\n{}",
        text
    );
    assert!(
        text.contains("[top-level]"),
        "expected top-level group header, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_inside"),
        "expected module test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_top"),
        "expected top-level test pass, output:\n{}",
        text
    );
}

#[test]
fn test_mode_flow_test_wrappers_work() {
    let file = fixture_path("flow_wrapper.flx");
    let output = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_flow_wrappers"),
        "expected wrapper test pass, output:\n{}",
        text
    );
}

#[test]
fn test_mode_test_filter_runs_subset() {
    let file = fixture_path("all_pass.flx");
    let output = run_flux(&["--test", file.to_str().unwrap(), "--test-filter", "test_a"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success status, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_a"),
        "expected filtered test to run, output:\n{}",
        text
    );
    assert!(
        !text.contains("PASS  test_b"),
        "expected non-matching test to be excluded, output:\n{}",
        text
    );
    assert!(
        text.contains("1 tests: 1 passed, 0 failed"),
        "expected subset summary, output:\n{}",
        text
    );
}

#[test]
fn test_mode_test_filter_no_match_reports_empty() {
    let file = fixture_path("all_pass.flx");
    let output = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--test-filter",
        "does_not_exist",
    ]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success when no tests match filter, output:\n{}",
        text
    );
    assert!(
        text.contains("No test functions found matching filter"),
        "expected no-match message, output:\n{}",
        text
    );
}

#[cfg(feature = "jit")]
#[test]
fn test_mode_jit_matches_vm_summary() {
    let file = fixture_path("all_pass.flx");
    let vm = run_flux(&["--test", file.to_str().unwrap()]);
    let vm_text = combined_output(&vm);
    assert!(vm.status.success(), "vm run failed:\n{}", vm_text);

    let jit = run_flux(&["--test", file.to_str().unwrap(), "--jit"]);
    let jit_text = combined_output(&jit);
    assert!(jit.status.success(), "jit run failed:\n{}", jit_text);

    assert!(
        vm_text.contains("2 tests: 2 passed, 0 failed"),
        "unexpected vm summary:\n{}",
        vm_text
    );
    assert!(
        jit_text.contains("2 tests: 2 passed, 0 failed"),
        "unexpected jit summary:\n{}",
        jit_text
    );

    let jit_filtered = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--test-filter",
        "test_a",
        "--jit",
    ]);
    let jit_filtered_text = combined_output(&jit_filtered);
    assert!(
        jit_filtered.status.success(),
        "jit filtered run failed:\n{}",
        jit_filtered_text
    );
    assert!(
        jit_filtered_text.contains("1 tests: 1 passed, 0 failed"),
        "unexpected jit filtered summary:\n{}",
        jit_filtered_text
    );
}
