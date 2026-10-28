use super::*;

#[test]
fn test_mode_no_tests_returns_success() {
    let file = fixture_path("no_tests.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected success status, output:\n{}", text);
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

    assert!(!output.status.success(), "expected failure, output:\n{}", text);
    assert!(text.contains("PASS  test_ok"), "expected passing test to run, output:\n{}", text);
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

    assert!(output.status.success(), "expected success, output:\n{}", text);
    assert!(text.contains("[Tests]"), "expected Tests group header, output:\n{}", text);
    assert!(text.contains("[top-level]"), "expected top-level group header, output:\n{}", text);
    assert!(text.contains("PASS  test_inside"), "expected module test pass, output:\n{}", text);
    assert!(text.contains("PASS  test_top"), "expected top-level test pass, output:\n{}", text);
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

    assert!(output.status.success(), "expected success, output:\n{}", text);
    assert!(
        text.contains("PASS  test_flow_wrappers"),
        "expected wrapper test pass, output:\n{}",
        text
    );
}

#[test]
fn test_mode_flow_list_module_fixture_passes() {
    let file = fixture_path("Flow/List_test.flx");
    let output = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected success, output:\n{}", text);
    assert!(
        text.contains("PASS  test_core_hofs"),
        "expected core HOFs test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_accessors_and_predicates"),
        "expected accessors test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_sort_and_reverse"),
        "expected sort/reverse test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_numeric_reductions"),
        "expected numeric reductions test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_slicing_helpers"),
        "expected slicing helper test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_zip_and_group_helpers"),
        "expected zip/group helper test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_list_specific_shape"),
        "expected list shape test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_set_like_operations"),
        "expected set-like operations test pass, output:\n{}",
        text
    );
    assert!(text.contains("9 tests: 9 passed, 0 failed"), "unexpected summary, output:\n{}", text);
}

#[test]
fn test_mode_flow_list_module_fixture_passes_in_strict_mode() {
    let file = fixture_path("Flow/List_test.flx");
    let output = run_flux(&[
        "--strict",
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected strict Flow/List fixture success, output:\n{}",
        text
    );
    assert!(text.contains("9 tests: 9 passed, 0 failed"), "unexpected summary, output:\n{}", text);
}

#[test]
fn test_mode_flow_array_module_fixture_passes() {
    let file = fixture_path("Flow/Array_test.flx");
    let output = run_flux(&[
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected success, output:\n{}", text);
    assert!(
        text.contains("PASS  test_array_core_hofs"),
        "expected core HOFs test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_array_accessors_and_predicates"),
        "expected accessors test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_array_ordering_and_slices"),
        "expected ordering/slices test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_array_specific_shape"),
        "expected array shape test pass, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_array_zip_and_updates"),
        "expected zip/update test pass, output:\n{}",
        text
    );
    assert!(text.contains("5 tests: 5 passed, 0 failed"), "unexpected summary, output:\n{}", text);
}

#[test]
fn test_mode_flow_array_module_fixture_passes_in_strict_mode() {
    let file = fixture_path("Flow/Array_test.flx");
    let output = run_flux(&[
        "--strict",
        "--test",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected strict Flow/Array fixture success, output:\n{}",
        text
    );
    assert!(text.contains("5 tests: 5 passed, 0 failed"), "unexpected summary, output:\n{}", text);
}

#[cfg(feature = "llvm")]
#[test]
fn test_mode_flow_list_module_fixture_passes_on_native_llvm() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = fixture_path("Flow/List_test.flx");
    let output = run_flux(&[
        "--test",
        "--native",
        file.to_str().unwrap(),
        "--root",
        workspace_root().join("lib").to_str().unwrap(),
    ]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected native success, output:\n{}", text);
    assert!(
        text.contains("PASS  test_core_hofs"),
        "expected core HOFs test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_accessors_and_predicates"),
        "expected accessors test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_sort_and_reverse"),
        "expected sort/reverse test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_numeric_reductions"),
        "expected numeric reductions test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_slicing_helpers"),
        "expected slicing helper test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_zip_and_group_helpers"),
        "expected zip/group helper test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_list_specific_shape"),
        "expected list shape test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_set_like_operations"),
        "expected set-like operations test pass on native backend, output:\n{}",
        text
    );
    assert!(
        text.contains("9 tests: 9 passed, 0 failed"),
        "unexpected native summary, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_mode_native_handles_files_that_already_define_main() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }

    let (_files, file) =
        write_temp_flux_file("native_test_main", "fn main() { 0 }\nfn test_ok() { 0 }\n");
    let output = run_flux(&["--test", "--native", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for file with user main, output:\n{}",
        text
    );
    assert!(text.contains("PASS  test_ok"), "expected native test pass, output:\n{}", text);
    assert!(
        !text.contains("Duplicate Main Function"),
        "unexpected duplicate main error, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_mode_native_rejects_additional_main_references_in_harness_rewrite() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }

    let (_files, file) =
        write_temp_flux_file("native_test_main_ref", "fn main() { main() }\nfn test_ok() { 0 }\n");
    let output = run_flux(&["--test", "--native", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        !output.status.success(),
        "expected native failure for unsupported main references, output:\n{}",
        text
    );
    assert!(
        text.contains("does not support additional `main` references"),
        "expected targeted harness error, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_sort_by_string_len_repro_prints_sorted_strings() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("guide/sort_by_string_len.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for sort_by_string_len repro, output:\n{}",
        text
    );
    assert!(
        text.contains("\"[\"a\", \"bb\", \"ccc\"]\""),
        "expected sorted string output on native backend, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_list_map_filter_example_preserves_list_zip_output() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("guide/list_map_filter.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for list_map_filter example, output:\n{}",
        text
    );
    assert!(
        text.contains("[(1, 1), (2, 2), (3, 3)]"),
        "expected list zip output on native backend, output:\n{}",
        text
    );
}

#[test]
fn test_vm_higher_order_builtins_example_sorts_arrays() {
    let file = example_path("guide/higher_order_builtins.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected VM success for higher_order_builtins example, output:\n{}",
        text
    );
    assert!(
        text.contains("[|\"fig\", \"kiwi\", \"apple\", \"banana\"|]"),
        "expected string sort_by output on VM backend, output:\n{}",
        text
    );
    assert!(
        text.contains("[|5, 4, 3, 2, 1|]"),
        "expected descending numeric sort_by output on VM backend, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_dump_lir_llvm_reuse_path_writes_raw_cons_headers() {
    if !cli_supports_flag("--dump-lir-llvm") {
        eprintln!("skipping LLVM dump CLI test: binary does not advertise --dump-lir-llvm");
        return;
    }
    let file = example_path("guide/sort_by_string_len.flx");
    let output = run_flux(&["--dump-lir-llvm", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected --dump-lir-llvm success, output:\n{}", text);

    let merge_by_key = extract_function_ir(&text, "flux_Flow_List_merge_by_key");
    assert!(
        merge_by_key.contains("call fastcc i1 @flux_rc_is_unique(i64 %v0)"),
        "expected merge_by_key to branch on uniqueness for left cons reuse:\n{}",
        merge_by_key
    );
    assert!(
        merge_by_key.contains("call fastcc i1 @flux_rc_is_unique(i64 %v1)"),
        "expected merge_by_key to branch on uniqueness for right cons reuse:\n{}",
        merge_by_key
    );
    // The reuse path stores ctor_tag=4 and field_count=2 as raw i32s.
    // Check that at least two pairs exist (one for left, one for right
    // reuse) without relying on exact temp variable names.
    let ctor_tag_stores = merge_by_key.matches("store i32 4, ptr %t").count();
    let field_count_stores = merge_by_key.matches("store i32 2, ptr %t").count();
    assert!(
        ctor_tag_stores >= 2,
        "expected at least 2 raw ctor_tag=4 stores (left+right reuse), found {ctor_tag_stores}:\n{}",
        merge_by_key
    );
    assert!(
        field_count_stores >= 2,
        "expected at least 2 raw field_count=2 stores (left+right reuse), found {field_count_stores}:\n{}",
        merge_by_key
    );
    assert!(
        !merge_by_key.contains("@flux_drop_reuse"),
        "expected merge_by_key to avoid the drop_reuse path that masked fresh allocations:\n{}",
        merge_by_key
    );
}

#[test]
fn test_mode_test_filter_runs_subset() {
    let file = fixture_path("all_pass.flx");
    let output = run_flux(&["--test", file.to_str().unwrap(), "--test-filter", "test_a"]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected success status, output:\n{}", text);
    assert!(text.contains("PASS  test_a"), "expected filtered test to run, output:\n{}", text);
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
    let output = run_flux(&["--test", file.to_str().unwrap(), "--test-filter", "does_not_exist"]);
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

    assert!(output.status.success(), "expected success, output:\n{}", text);
    assert!(text.contains("8 tests: 8 passed, 0 failed"), "unexpected summary, output:\n{}", text);
}

#[test]
fn test_mode_base_assertions_all_pass() {
    let file = fixture_path("base_assertions.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected all base assertion tests to pass, output:\n{}",
        text
    );

    // Verify key test groups ran
    assert!(
        text.contains("PASS  test_assert_eq_integers"),
        "expected assert_eq integer test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_eq_maps"),
        "expected assert_eq map test (HAMT comparison), output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_throws_one_arg"),
        "expected 1-arg assert_throws, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_throws_two_args"),
        "expected 2-arg assert_throws, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_msg_passes"),
        "expected assert_msg test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_msg_custom_message"),
        "expected assert_msg custom message test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_try_catch_ok"),
        "expected try_catch ok test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_try_catch_error"),
        "expected try_catch error test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_try_catch_does_not_propagate"),
        "expected try_catch isolation test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_str_contains_found"),
        "expected str_contains found test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_gt_passes"),
        "expected assert_gt passes test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_lt_passes"),
        "expected assert_lt passes test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_assert_len_array"),
        "expected assert_len array test, output:\n{}",
        text
    );

    // Verify total count
    assert!(
        text.contains("40 tests: 40 passed, 0 failed"),
        "expected 40 tests all passing, output:\n{}",
        text
    );
}

/// The assertion's own output, not an unhandled Console effect: `assert_eq`
/// prints before it panics, and the test declares no effects (KI-139).
fn assert_reports_assert_eq_failure(output: &Output) {
    let text = combined_output(output);
    assert!(!output.status.success(), "expected failure, output:\n{text}");
    assert!(!text.contains("unhandled effect"), "expected no unhandled effect, output:\n{text}");
    assert!(
        text.contains("ASSERTION FAILED: assert_eq"),
        "expected the assertion, output:\n{text}"
    );
    assert!(text.contains("assert_eq failed"), "expected the panic, output:\n{text}");
}

#[test]
fn test_mode_failing_assert_eq_reports_its_values_ki139() {
    let file = fixture_path("assert_eq_failure_message.flx");
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    assert_reports_assert_eq_failure(&output);
    assert!(combined_output(&output).contains("PASS  test_assert_eq_passes"));
}

#[test]
fn failing_assert_eq_in_main_reports_its_values_ki139() {
    let file = fixture_path("assert_eq_failure_in_main.flx");
    assert_reports_assert_eq_failure(&run_flux(&[file.to_str().unwrap()]));
}

#[cfg(feature = "llvm")]
#[test]
fn test_mode_failing_assert_eq_reports_its_values_on_native_llvm_ki139() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = fixture_path("assert_eq_failure_message.flx");
    let output = run_flux(&["--test", "--native", file.to_str().unwrap()]);
    assert_reports_assert_eq_failure(&output);
}
