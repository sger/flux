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

fn run_flux_strict(args: &[&str]) -> Output {
    let mut full_args = vec!["--strict"];
    full_args.extend_from_slice(args);
    Command::new(env!("CARGO_BIN_EXE_flux"))
        .args(&full_args)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux with args {:?}: {e}", args))
}

#[cfg(feature = "llvm")]
fn cli_supports_flag(flag: &str) -> bool {
    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run flux --help: {e}"));
    combined_output(&output).contains(flag)
}

fn combined_output(output: &Output) -> String {
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

#[cfg(feature = "llvm")]
fn write_temp_flux_file(prefix: &str, source: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "flux_{prefix}_{}_{}.flx",
        std::process::id(),
        unique
    ));
    std::fs::write(&path, source)
        .unwrap_or_else(|err| panic!("failed to write temp fixture {}: {err}", path.display()));
    path
}

#[allow(dead_code)]
fn extract_function_ir<'a>(llvm: &'a str, name: &str) -> &'a str {
    // Try both internal (single-module) and non-internal (per-module) linkage.
    let needle_internal = format!("define internal fastcc i64 @{name}");
    let needle_public = format!("define fastcc i64 @{name}");
    let start = llvm
        .find(&needle_internal)
        .or_else(|| llvm.find(&needle_public))
        .unwrap_or_else(|| panic!("expected function `{name}` in LLVM output:\n{llvm}"));
    let rest = &llvm[start..];
    if let Some(next_define) = rest[1..].find("\ndefine ") {
        &rest[..next_define + 1]
    } else {
        rest
    }
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
fn test_mode_flow_list_module_fixture_passes() {
    let file = fixture_path("Flow/List_test.flx");
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
    assert!(
        text.contains("9 tests: 9 passed, 0 failed"),
        "unexpected summary, output:\n{}",
        text
    );
}

#[test]
fn test_mode_flow_list_module_fixture_reports_strict_stdlib_diagnostics() {
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
        !output.status.success(),
        "expected strict failure surfacing stdlib diagnostics, output:\n{}",
        text
    );
    assert!(
        text.contains("tests/flux/Flow/List_test.flx")
            || text.contains("lib/Flow/List.flx")
            || text.contains("lib/Flow/Assert.flx"),
        "expected strict Flow-related module path in output, output:\n{}",
        text
    );
    assert!(
        text.contains("error[E417]")
            || text.contains("error[E430]")
            || text.contains("error[E425]"),
        "expected strict typing diagnostics from stdlib, output:\n{}",
        text
    );
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

    assert!(
        output.status.success(),
        "expected success, output:\n{}",
        text
    );
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
    assert!(
        text.contains("5 tests: 5 passed, 0 failed"),
        "unexpected summary, output:\n{}",
        text
    );
}

#[test]
fn test_mode_flow_array_module_fixture_reports_strict_stdlib_diagnostics() {
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
        !output.status.success(),
        "expected strict failure surfacing stdlib diagnostics, output:\n{}",
        text
    );
    assert!(
        text.contains("tests/flux/Flow/Array_test.flx")
            || text.contains("lib/Flow/Array.flx")
            || text.contains("lib/Flow/Assert.flx"),
        "expected strict Flow-related module path in output, output:\n{}",
        text
    );
    assert!(
        text.contains("error[E417]")
            || text.contains("error[E430]")
            || text.contains("error[E425]"),
        "expected strict typing diagnostics from stdlib, output:\n{}",
        text
    );
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

    assert!(
        output.status.success(),
        "expected native success, output:\n{}",
        text
    );
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

    let file = write_temp_flux_file("native_test_main", "fn main() { 0 }\nfn test_ok() { 0 }\n");
    let output = run_flux(&["--test", "--native", file.to_str().unwrap()]);
    let text = combined_output(&output);
    let _ = std::fs::remove_file(&file);

    assert!(
        output.status.success(),
        "expected native success for file with user main, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_ok"),
        "expected native test pass, output:\n{}",
        text
    );
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

    let file = write_temp_flux_file(
        "native_test_main_ref",
        "fn main() { main() }\nfn test_ok() { 0 }\n",
    );
    let output = run_flux(&["--test", "--native", file.to_str().unwrap()]);
    let text = combined_output(&output);
    let _ = std::fs::remove_file(&file);

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
    let file = example_path("repros/sort_by_string_len.flx");
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
    let file = example_path("advanced/list_map_filter.flx");
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
    let file = example_path("basics/higher_order_builtins.flx");
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
    let file = example_path("repros/sort_by_string_len.flx");
    let output = run_flux(&["--dump-lir-llvm", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected --dump-lir-llvm success, output:\n{}",
        text
    );

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
        text.contains("PASS  test_try_ok"),
        "expected try ok test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_try_error"),
        "expected try error test, output:\n{}",
        text
    );
    assert!(
        text.contains("PASS  test_try_does_not_propagate"),
        "expected try isolation test, output:\n{}",
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

// ---------------------------------------------------------------------------
// Syntax test fixtures
// ---------------------------------------------------------------------------

fn run_syntax_fixture(name: &str, expected_count: u32) {
    let file = fixture_path(name);
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected all tests to pass in {}, output:\n{}",
        name,
        text
    );

    let summary = format!(
        "{} tests: {} passed, 0 failed",
        expected_count, expected_count
    );
    assert!(
        text.contains(&summary),
        "expected '{}' in {}, output:\n{}",
        summary,
        name,
        text
    );
}

#[test]
fn test_syntax_let_and_operators() {
    run_syntax_fixture("syntax_let_and_operators.flx", 24);
}

#[test]
fn test_syntax_functions_and_lambdas() {
    run_syntax_fixture("syntax_functions_and_lambdas.flx", 16);
}

#[test]
fn test_syntax_control_flow() {
    run_syntax_fixture("syntax_control_flow.flx", 17);
}

#[test]
fn test_syntax_collections() {
    run_syntax_fixture("syntax_collections.flx", 30);
}

#[test]
fn test_syntax_pattern_matching() {
    run_syntax_fixture("syntax_pattern_matching.flx", 13);
}

#[test]
fn test_syntax_pipes_and_hof() {
    run_syntax_fixture("syntax_pipes_and_hof.flx", 30);
}

#[test]
fn test_syntax_strings() {
    run_syntax_fixture("syntax_strings.flx", 23);
}

#[test]
fn test_syntax_adt() {
    run_syntax_fixture("syntax_adt.flx", 10);
}

#[test]
fn test_syntax_comprehensions() {
    run_syntax_fixture("syntax_comprehensions.flx", 11);
}

#[test]
fn dump_core_prints_core_ir_and_exits_before_execution() {
    let file = example_path("basics/arithmetic.flx");
    let output = run_flux(&["--dump-core", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected dump-core success, output:\n{}",
        text
    );
    assert!(
        text.contains("def main ="),
        "expected readable Core dump header, output:\n{}",
        text
    );
    assert!(
        text.contains("Print(3)"),
        "expected optimized readable Core dump for arithmetic example, output:\n{}",
        text
    );
    assert!(
        text.contains("%t1"),
        "expected normalized temp names in readable dump, output:\n{}",
        text
    );
    assert!(
        !text.contains("#200000"),
        "readable dump should not contain raw synthetic names, output:\n{}",
        text
    );
    assert!(
        !text.contains("print#?"),
        "readable dump should hide external markers, output:\n{}",
        text
    );
    assert!(
        !text.contains("\n3\n"),
        "dump-core should not execute the program, output:\n{}",
        text
    );
}

#[test]
fn dump_core_debug_preserves_raw_identity_details() {
    let file = example_path("basics/arithmetic.flx");
    let output = run_flux(&["--dump-core=debug", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected dump-core debug success, output:\n{}",
        text
    );
    assert!(
        text.contains("letrec main"),
        "expected raw main thunk shape in debug dump, output:\n{}",
        text
    );
    assert!(
        text.contains("Print("),
        "expected Print() primop after promotion in debug dump, output:\n{}",
        text
    );
    assert!(
        text.contains("[synthetic]"),
        "expected synthetic binder annotation in debug dump, output:\n{}",
        text
    );
    assert!(
        !text.contains("def main ="),
        "debug dump should not normalize main thunk shape, output:\n{}",
        text
    );
    assert!(
        !text.contains("\n3\n"),
        "dump-core debug should not execute the program, output:\n{}",
        text
    );
}

#[test]
fn dump_cfg_prints_cfg_ir_and_exits_before_execution() {
    let file = example_path("basics/arithmetic.flx");
    let output = run_flux(&["--dump-cfg", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected dump-cfg success, output:\n{}",
        text
    );
    assert!(
        text.contains("fn #") || text.contains("fn <anon>"),
        "expected CFG function header, output:\n{}",
        text
    );
    assert!(
        text.contains("Return"),
        "expected CFG terminator, output:\n{}",
        text
    );
    assert!(
        !text.contains("\n3\n"),
        "dump-cfg should not execute the program, output:\n{}",
        text
    );
}

#[test]
fn dump_core_debug_excludes_aether_stats() {
    let file = example_path("aether/verify_aether.flx");
    let output = run_flux(&["--dump-core=debug", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected dump-core debug success, output:\n{}",
        text
    );
    assert!(
        !text.contains("── Aether stats ──"),
        "dump-core debug should stay semantic-only, output:\n{}",
        text
    );
    assert!(
        !text.contains("DropSpecs: "),
        "dump-core debug should not include Aether stats, output:\n{}",
        text
    );
}

#[test]
fn dump_core_debug_shows_explicit_polymorphism_without_dynamic() {
    let file = example_path("aether/polymorphic_core_types.flx");
    let output = run_flux(&["--dump-core=debug", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected dump-core debug success, output:\n{}",
        text
    );
    assert!(
        text.contains("forall "),
        "expected dump-core debug to render explicit polymorphism, output:\n{}",
        text
    );
    assert!(
        text.contains("letrec id : t") && text.contains("letrec choose : t"),
        "expected dump-core debug to preserve explicit type-variable residue for local polymorphic defs, output:\n{}",
        text
    );
    assert!(
        !text.contains("Dynamic"),
        "dump-core debug should not regress to semantic Dynamic placeholders, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_aether_queue_workload_matches_vm_totals() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("aether/queue_workload.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for queue_workload example, output:\n{}",
        text
    );
    assert!(
        text.contains("\"rotated total = 3502500\""),
        "expected native rotated total to match VM, output:\n{}",
        text
    );
    assert!(
        text.contains("\"drained total = 2001000\""),
        "expected native drained total to match VM, output:\n{}",
        text
    );
}

#[test]
fn test_aether_bench_reuse_enabled_prints_head_value() {
    let file = example_path("aether/bench_reuse_enabled.flx");
    let output = run_flux(&["--no-cache", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected VM success for bench_reuse_enabled example, output:\n{}",
        text
    );
    assert!(
        text.contains("\"enabled: result head = 101\""),
        "expected mapped head output on VM backend, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_aether_bench_reuse_enabled_prints_head_value() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("aether/bench_reuse_enabled.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for bench_reuse_enabled example, output:\n{}",
        text
    );
    assert!(
        text.contains("\"enabled: result head = 101\""),
        "expected mapped head output on native backend, output:\n{}",
        text
    );
}

#[test]
fn test_aether_bench_reuse_blocked_prints_head_value() {
    let file = example_path("aether/bench_reuse_blocked.flx");
    let output = run_flux(&["--no-cache", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected VM success for bench_reuse_blocked example, output:\n{}",
        text
    );
    assert!(
        text.contains("\"blocked: result head = 101\""),
        "expected mapped head output on VM backend, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_aether_bench_reuse_blocked_prints_head_value() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("aether/bench_reuse_blocked.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for bench_reuse_blocked example, output:\n{}",
        text
    );
    assert!(
        text.contains("\"blocked: result head = 101\""),
        "expected mapped head output on native backend, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_day06_multimodule_adt_program_links_and_runs() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("aoc/2024/day06.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for day06 example, output:\n{}",
        text
    );
    assert!(
        text.contains("Part A:"),
        "expected Part A output from native day06 example, output:\n{}",
        text
    );
    assert!(
        text.contains("Part B:"),
        "expected Part B output from native day06 example, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_using_modules_program_links_without_user_adts() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }
    let file = example_path("advanced/using_modules.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for using_modules example, output:\n{}",
        text
    );
    assert!(
        !text.contains("undefined symbol: flux_user_ctor_name"),
        "expected runtime stub to satisfy ctor-name symbol, output:\n{}",
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

// ---------------------------------------------------------------------------
// Example manifest cases (previously in ci/examples_manifest.tsv)
// ---------------------------------------------------------------------------

/// Helper: run an example file with optional roots, expect success.
fn assert_example_ok(path: &str, roots: &[&str]) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> = roots
        .iter()
        .map(|r| workspace_root().join(r).to_string_lossy().into_owned())
        .collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux(&args);
    let text = combined_output(&output);
    assert!(
        output.status.success(),
        "expected success for {path}, output:\n{text}"
    );
}

/// Helper: run an example file with optional roots in strict mode, expect success.
fn assert_example_ok_strict(path: &str, roots: &[&str]) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> = roots
        .iter()
        .map(|r| workspace_root().join(r).to_string_lossy().into_owned())
        .collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux_strict(&args);
    let text = combined_output(&output);
    assert!(
        output.status.success(),
        "expected success (strict) for {path}, output:\n{text}"
    );
}

/// Helper: run an example, expect failure with a specific error code.
fn assert_example_error(path: &str, roots: &[&str], expected_code: &str) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> = roots
        .iter()
        .map(|r| workspace_root().join(r).to_string_lossy().into_owned())
        .collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux(&args);
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "expected failure for {path}, output:\n{text}"
    );
    assert!(
        text.contains(expected_code),
        "expected {expected_code} for {path}, output:\n{text}"
    );
}

/// Helper: run an example in strict mode, expect failure with a specific error code.
fn assert_example_error_strict(path: &str, roots: &[&str], expected_code: &str) {
    let file = workspace_root().join(path);
    let mut args: Vec<&str> = vec!["--no-cache"];
    let root_paths: Vec<String> = roots
        .iter()
        .map(|r| workspace_root().join(r).to_string_lossy().into_owned())
        .collect();
    for r in &root_paths {
        args.push("--root");
        args.push(r);
    }
    args.push(file.to_str().unwrap());
    let output = run_flux_strict(&args);
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "expected failure (strict) for {path}, output:\n{text}"
    );
    assert!(
        text.contains(expected_code),
        "expected {expected_code} (strict) for {path}, output:\n{text}"
    );
}

// --- Smoke tests ---

#[test]
fn example_aoc_day05_part1() {
    assert_example_ok(
        "examples/aoc/2024/day05_part1_test.flx",
        &["lib", "examples/aoc/2024"],
    );
}

#[test]
fn example_contextual_boundary_ok_161() {
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
        "examples/type_system/failing/190_contextual_boundary_arg_runtime_e1004.flx",
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
    assert_example_error(
        "examples/type_system/failing/173_perform_missing_dot.flx",
        &[],
        "E034",
    );
}

#[test]
fn example_parser_handle_missing_lbrace_174() {
    assert_example_error(
        "examples/type_system/failing/174_handle_missing_lbrace.flx",
        &[],
        "E034",
    );
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
fn example_effect_row_order_ok_162() {
    assert_example_ok(
        "examples/type_system/100_effect_row_order_equivalence_ok.flx",
        &[],
    );
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
fn example_effect_row_subtract_ok_163() {
    assert_example_ok(
        "examples/type_system/101_effect_row_subtract_concrete_ok.flx",
        &[],
    );
}

#[test]
fn example_effect_row_subtract_var_ok_164() {
    assert_example_ok(
        "examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx",
        &[],
    );
}

#[test]
fn example_effect_row_multivar_ok_165() {
    assert_example_ok(
        "examples/type_system/103_effect_row_multivar_disambiguated_ok.flx",
        &[],
    );
}

#[test]
fn example_effect_row_invalid_subtract_e300_195() {
    assert_example_error(
        "examples/type_system/failing/195_effect_row_invalid_subtract_e421.flx",
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
fn example_effect_row_unresolved_multi_e420_197() {
    // HM now reports E304 (Invalid Effect Row) directly at the annotation
    // site when a single `with` clause mixes distinct row variables, which
    // supersedes the downstream E420 this fixture historically produced.
    assert_example_error(
        "examples/type_system/failing/197_effect_row_subtract_unresolved_multi_e420.flx",
        &[],
        "E304",
    );
}

#[test]
fn example_effect_row_subset_unsatisfied_e300_198() {
    assert_example_error(
        "examples/type_system/failing/198_effect_row_subset_unsatisfied_e422.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_effect_row_subset_sorted_e300_199() {
    assert_example_error(
        "examples/type_system/failing/199_effect_row_subset_ordered_missing_e422.flx",
        &[],
        "E300",
    );
}

#[test]
fn example_effect_row_absent_ordering_ok_166() {
    assert_example_ok(
        "examples/type_system/104_effect_row_absent_ordering_linked_ok.flx",
        &[],
    );
}

#[test]
fn example_base_hof_effect_row_ok_167() {
    assert_example_ok(
        "examples/type_system/105_base_hof_callback_effect_row_ok.flx",
        &[],
    );
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
        "E400",
    );
}

#[test]
fn example_runtime_boundary_list_e300_187() {
    assert_example_error(
        "examples/type_system/failing/187_runtime_list_boundary_e1004.flx",
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
