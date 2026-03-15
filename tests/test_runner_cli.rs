use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path(name: &str) -> PathBuf {
    workspace_root().join("tests").join("flux").join(name)
}

fn example_path(rel: &str) -> PathBuf {
    workspace_root().join("examples").join(rel)
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

#[test]
fn test_mode_primops_fixture_passes_on_vm() {
    let file = fixture_path("primops_all.flx");
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
        text.contains("8 tests: 8 passed, 0 failed"),
        "unexpected summary, output:\n{}",
        text
    );
}

#[test]
fn all_errors_flag_reveals_downstream_diagnostics_in_run_mode() {
    let file = example_path("type_system/failing/210_stage_all_errors_flag.flx");

    let default_output = run_flux(&["--no-cache", file.to_str().unwrap()]);
    let default_text = combined_output(&default_output);
    assert!(
        !default_output.status.success(),
        "expected fixture to fail in default mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E300]"),
        "expected type diagnostic to remain visible in a different module, output:\n{}",
        default_text
    );
    assert!(
        !default_text.contains("Downstream Errors Suppressed"),
        "did not expect a cross-module suppression note in default mode, output:\n{}",
        default_text
    );

    let all_output = run_flux(&["--no-cache", "--all-errors", file.to_str().unwrap()]);
    let all_text = combined_output(&all_output);
    assert!(
        !all_output.status.success(),
        "expected fixture to fail with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        all_text.contains("error[E300]"),
        "expected downstream type diagnostic visible with --all-errors, output:\n{}",
        all_text
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_runtime_error_json_matches_text_metadata() {
    let file = example_path("runtime_errors/indirect_call_wrong_arity.flx");

    let text_output = run_flux(&["--no-cache", file.to_str().unwrap(), "--jit"]);
    let text = combined_output(&text_output);
    assert!(
        !text_output.status.success(),
        "expected JIT text run to fail, output:\n{}",
        text
    );
    assert!(
        text.contains("error[E1000]: wrong number of arguments: want=2, got=1"),
        "expected structured text diagnostic, output:\n{}",
        text
    );

    let json_output = run_flux(&[
        "--no-cache",
        file.to_str().unwrap(),
        "--jit",
        "--format",
        "json",
    ]);
    let json_text = combined_output(&json_output);
    assert!(
        !json_output.status.success(),
        "expected JIT json run to fail, output:\n{}",
        json_text
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&json_text).expect("expected valid JSON diagnostics output");
    let first = parsed
        .as_array()
        .and_then(|arr| arr.first())
        .expect("expected at least one runtime diagnostic");

    assert_eq!(first.get("code").and_then(|v| v.as_str()), Some("E1000"));
    assert_eq!(first.get("phase").and_then(|v| v.as_str()), Some("runtime"));
    assert_eq!(
        first.get("category").and_then(|v| v.as_str()),
        Some("runtime_execution")
    );
    assert_eq!(
        first.get("title").and_then(|v| v.as_str()),
        Some("wrong number of arguments: want=2, got=1")
    );
    assert_eq!(
        first.get("file").and_then(|v| v.as_str()),
        Some("examples/runtime_errors/indirect_call_wrong_arity.flx")
    );
}

#[test]
fn test_mode_parse_errors_exit_early_even_with_all_errors() {
    let file = fixture_path("parse_error.flx");

    let normal = run_flux(&["--test", file.to_str().unwrap()]);
    let normal_text = combined_output(&normal);
    assert!(
        !normal.status.success(),
        "expected parse failure in test mode, output:\n{}",
        normal_text
    );
    assert!(
        normal_text.contains("error[E071]") || normal_text.contains("error[E076]"),
        "expected parse diagnostics in test mode, output:\n{}",
        normal_text
    );

    let all = run_flux(&["--test", "--all-errors", file.to_str().unwrap()]);
    let all_text = combined_output(&all);
    assert!(
        !all.status.success(),
        "expected parse failure in test mode with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        all_text.contains("error[E071]") || all_text.contains("error[E076]"),
        "expected parse diagnostics in test mode with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        !all_text.contains("Downstream Errors Suppressed"),
        "test mode exits immediately on parse errors, so stage-filter suppression notes should not appear:\n{}",
        all_text
    );
}

#[test]
fn all_errors_flag_reveals_effect_diagnostics_after_type_errors() {
    let file = example_path("compiler_errors/adversarial/stage_all_errors/Main.flx");

    let default_output = run_flux(&["--no-cache", file.to_str().unwrap()]);
    let default_text = combined_output(&default_output);
    assert!(
        !default_output.status.success(),
        "expected adversarial compiler fixture to fail in default mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E300]: Annotation Type Mismatch"),
        "expected visible type diagnostic in default mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E400]: Missing Ambient Effect"),
        "expected effect diagnostic to remain visible in a different module, output:\n{}",
        default_text
    );
    assert!(
        !default_text.contains("Downstream Errors Suppressed"),
        "did not expect a cross-module suppression note in default mode, output:\n{}",
        default_text
    );

    let all_output = run_flux(&["--no-cache", "--all-errors", file.to_str().unwrap()]);
    let all_text = combined_output(&all_output);
    assert!(
        !all_output.status.success(),
        "expected adversarial compiler fixture to fail with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        all_text.contains("error[E300]: Annotation Type Mismatch"),
        "expected type diagnostic visible with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        all_text.contains("error[E400]: Missing Ambient Effect"),
        "expected effect diagnostic visible with --all-errors, output:\n{}",
        all_text
    );
}

#[cfg(feature = "jit")]
#[test]
fn test_mode_primops_fixture_passes_on_jit() {
    let file = fixture_path("primops_all.flx");
    let output = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
        "--jit",
    ]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected success, output:\n{}",
        text
    );
    assert!(
        text.contains("8 tests: 8 passed, 0 failed"),
        "unexpected summary, output:\n{}",
        text
    );
}
