use super::*;

// ---------------------------------------------------------------------------
// Example manifest cases (previously in ci/examples_manifest.tsv)
// ---------------------------------------------------------------------------

/// Helper: run an example file with optional roots, expect success.
fn assert_example_ok(path: &str, roots: &[&str]) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> =
        roots.iter().map(|r| workspace_root().join(r).to_string_lossy().into_owned()).collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux(&args);
    let text = combined_output(&output);
    assert!(output.status.success(), "expected success for {path}, output:\n{text}");
}

/// Helper: run an example file with optional roots in strict mode, expect success.
fn assert_example_ok_strict(path: &str, roots: &[&str]) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> =
        roots.iter().map(|r| workspace_root().join(r).to_string_lossy().into_owned()).collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux_strict(&args);
    let text = combined_output(&output);
    assert!(output.status.success(), "expected success (strict) for {path}, output:\n{text}");
}

/// Helper: run an example, expect failure with a specific error code.
fn assert_example_error(path: &str, roots: &[&str], expected_code: &str) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> =
        roots.iter().map(|r| workspace_root().join(r).to_string_lossy().into_owned()).collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux(&args);
    let text = combined_output(&output);
    assert!(!output.status.success(), "expected failure for {path}, output:\n{text}");
    assert!(text.contains(expected_code), "expected {expected_code} for {path}, output:\n{text}");
}

/// Helper: run an example in strict mode, expect failure with a specific error code.
fn assert_example_error_strict(path: &str, roots: &[&str], expected_code: &str) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> =
        roots.iter().map(|r| workspace_root().join(r).to_string_lossy().into_owned()).collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux_strict(&args);
    let text = combined_output(&output);
    assert!(!output.status.success(), "expected failure (strict) for {path}, output:\n{text}");
    assert!(
        text.contains(expected_code),
        "expected {expected_code} (strict) for {path}, output:\n{text}"
    );
}

// --- Smoke tests ---

#[test]
fn example_aoc_day05_part1() {
    assert_example_ok("examples/aoc/2024/day05_part1_test.flx", &["lib", "examples/aoc/2024"]);
}

#[test]
fn example_contextual_boundary_ok_99() {
    assert_example_ok(
        "examples/type_system/99_contextual_boundary_effect_module_ok.flx",
        &["examples/type_system"],
    );
}

#[test]
fn example_contextual_boundary_e425_189() {
    assert_example_error_strict(
        "examples/type_system/failing/189_contextual_boundary_unresolved_strict_e425.flx",
        &["examples/type_system"],
        "E425",
    );
}

#[test]
fn example_perform_arg_e425_192() {
    assert_example_error_strict(
        "examples/type_system/failing/192_perform_arg_unresolved_strict_e425.flx",
        &[],
        "E425",
    );
}

#[test]
fn example_contextual_boundary_e300_190() {
    assert_example_error(
        "examples/type_system/failing/190_contextual_boundary_arg_runtime_e300.flx",
        &["examples/type_system"],
        "E300",
    );
}

#[test]
fn example_contextual_effect_e400_191() {
    assert_example_error(
        "examples/type_system/failing/191_contextual_effect_missing_module_call_e400.flx",
        &["examples/type_system"],
        "E400",
    );
}

#[test]
fn example_flow_wrapper() {
    assert_example_ok("tests/flux/flow_wrapper.flx", &["lib"]);
}

// --- Parser error tests ---

#[test]
fn example_parser_perform_missing_dot_173() {
    assert_example_error("examples/type_system/failing/173_perform_missing_dot.flx", &[], "E034");
}

#[test]
fn example_parser_handle_missing_lbrace_174() {
    assert_example_error("examples/type_system/failing/174_handle_missing_lbrace.flx", &[], "E034");
}

#[test]
fn example_parser_handle_missing_arrow_175() {
    assert_example_error(
        "examples/type_system/failing/175_handle_arm_missing_arrow.flx",
        &[],
        "E034",
    );
}

#[test]
fn example_parser_module_missing_brace_177() {
    assert_example_error(
        "examples/type_system/failing/177_module_missing_open_brace.flx",
        &[],
        "E034",
    );
}

// --- Type system tests ---

#[test]
fn example_strict_public_checked_60() {
    assert_example_ok_strict(
        "examples/type_system/60_strict_module_public_checked.flx",
        &["examples/type_system"],
    );
}

#[test]
fn example_strict_private_allowed_61() {
    assert_example_ok_strict(
        "examples/type_system/61_strict_module_private_unannotated_allowed.flx",
        &["examples/type_system"],
    );
}

#[test]
fn example_effect_row_order_ok_100() {
    assert_example_ok("examples/type_system/100_effect_row_order_equivalence_ok.flx", &[]);
}

#[test]
fn example_effect_row_multi_missing_e400_194() {
    assert_example_error(
        "examples/type_system/failing/194_effect_row_multi_missing_deterministic_e400.flx",
        &[],
        "E400",
    );
}

#[test]
fn example_effect_row_subtract_ok_101() {
    assert_example_ok("examples/type_system/101_effect_row_subtract_concrete_ok.flx", &[]);
}

#[test]
fn example_effect_row_subtract_var_ok_102() {
    assert_example_ok("examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx", &[]);
}

#[test]
fn example_effect_row_multivar_ok_103() {
    assert_example_ok("examples/type_system/103_effect_row_multivar_disambiguated_ok.flx", &[]);
}

#[test]
fn example_effect_row_invalid_subtract_e300_195() {
    assert_example_error(
        "examples/type_system/failing/195_effect_row_invalid_subtract_e300.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_effect_row_unresolved_single_e419_196() {
    assert_example_error(
        "examples/type_system/failing/196_effect_row_subtract_unresolved_single_e419.flx",
        &[],
        "E419",
    );
}

#[test]
fn example_effect_row_unresolved_multi_e304_197() {
    // HM now reports E304 (Invalid Effect Row) directly at the annotation
    // site when a single `with` clause mixes distinct row variables, which
    // supersedes the downstream E420 this fixture historically produced.
    assert_example_error(
        "examples/type_system/failing/197_effect_row_subtract_unresolved_multi_e304.flx",
        &[],
        "E304",
    );
}

#[test]
fn example_effect_row_subset_unsatisfied_e300_198() {
    assert_example_error(
        "examples/type_system/failing/198_effect_row_subset_unsatisfied_e300.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_effect_row_subset_sorted_e300_199() {
    assert_example_error(
        "examples/type_system/failing/199_effect_row_subset_ordered_missing_e300.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_effect_row_absent_ordering_ok_104() {
    assert_example_ok("examples/type_system/104_effect_row_absent_ordering_linked_ok.flx", &[]);
}

#[test]
fn example_base_hof_effect_row_ok_105() {
    assert_example_ok("examples/type_system/105_base_hof_callback_effect_row_ok.flx", &[]);
}

#[test]
fn example_effect_row_absent_ordering_violation_e421_200() {
    assert_example_error(
        "examples/type_system/failing/200_effect_row_absent_ordering_linked_violation_e421.flx",
        &[],
        "E421",
    );
}

#[test]
fn example_base_hof_effect_missing_e400_201() {
    assert_example_error(
        "examples/type_system/failing/201_base_hof_callback_effect_missing_e400.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_runtime_boundary_list_e300_187() {
    assert_example_error(
        "examples/type_system/failing/187_runtime_list_boundary_e300.flx",
        &["examples/type_system"],
        "E300",
    );
}

// --- Full program tests ---

#[test]
fn example_real_program_domain() {
    assert_example_ok(
        "examples/type_system/67_real_program_domain_module_test.flx",
        &["lib", "examples/type_system"],
    );
}

#[test]
fn example_real_program_effects() {
    assert_example_ok(
        "examples/type_system/68_real_program_effects_module_test.flx",
        &["lib", "examples/type_system"],
    );
}

#[test]
fn example_real_program_public_api() {
    assert_example_ok(
        "examples/type_system/69_real_program_public_api_test.flx",
        &["lib", "examples/type_system"],
    );
}

#[test]
fn example_real_program_primops() {
    assert_example_ok(
        "examples/type_system/70_real_program_primops_module_test.flx",
        &["lib", "examples/type_system"],
    );
}

#[test]
fn example_real_program_base_interop() {
    assert_example_ok(
        "examples/type_system/71_real_program_base_interop_module_test.flx",
        &["lib", "examples/type_system"],
    );
}

// ── `flux eval "<expr>"` — evaluate an expression and print its value ─────────
// The result lands on stdout; the VM banner and any diagnostics go to stderr, so
// value assertions read stdout alone. These run with cwd = the crate root, which
// has `lib/Flow`, so the prelude (`map`, …) resolves the same way the editor's
// subprocess sees it.

fn eval_stdout(expr: &str) -> String {
    let output = run_flux(&["eval", expr]);
    assert!(
        output.status.success(),
        "expected `flux eval {expr:?}` to succeed, output:\n{}",
        combined_output(&output)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn eval_arithmetic_prints_value() {
    assert_eq!(eval_stdout("2 + 2"), "4");
}

#[test]
fn eval_builtin_call_prints_value() {
    assert_eq!(eval_stdout("len([1, 2, 3])"), "3");
}

#[test]
fn eval_list_literal_prints_value() {
    assert_eq!(eval_stdout("[1, 2, 3]"), "[1, 2, 3]");
}

#[test]
fn eval_prelude_call_prints_value() {
    // `map` comes from the Flow prelude, exercising cwd-based prelude resolution.
    assert_eq!(eval_stdout("map([1, 2, 3], \\x -> x * 2)"), "[2, 4, 6]");
}

#[test]
fn eval_type_error_reports_diagnostic_without_panicking() {
    let output = run_flux(&["eval", "1 + \"x\""]);
    let text = combined_output(&output);
    assert!(!output.status.success(), "expected a type error to fail, output:\n{text}");
    assert!(
        String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        "a failed eval must not print a value to stdout, output:\n{text}"
    );
    assert!(
        !text.contains("panicked"),
        "eval must report a diagnostic, not a Rust panic, output:\n{text}"
    );
}

#[test]
fn eval_parse_error_exits_nonzero() {
    let output = run_flux(&["eval", "2 +"]);
    assert!(
        !output.status.success(),
        "expected a parse error to fail, output:\n{}",
        combined_output(&output)
    );
}
