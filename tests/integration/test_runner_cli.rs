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
    assert!(
        text.contains("9 tests: 9 passed, 0 failed"),
        "unexpected summary, output:\n{}",
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
    assert!(
        text.contains("5 tests: 5 passed, 0 failed"),
        "unexpected summary, output:\n{}",
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
    let file = example_path("guide/arithmetic.flx");
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
        text.contains("handle Console") && text.contains("__primop_print"),
        "expected readable Core dump to include the default Console handler, output:\n{}",
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
    let file = example_path("guide/arithmetic.flx");
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
    let file = example_path("guide/arithmetic.flx");
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
        text.contains("letrec id : forall a.") && text.contains("letrec choose : forall a."),
        "expected dump-core debug to preserve explicit quantified polymorphism for local defs, output:\n{}",
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

#[cfg(feature = "llvm")]
#[test]
fn test_native_large_list_fold_is_stack_safe() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }

    let file = write_temp_flux_file(
        "native_large_list_fold",
        r#"
fn main() with IO {
    let xs = range(1, 100001)
    let total = fold(xs, 0, \(acc, x) -> acc + x)
    print(total)
}
"#,
    );

    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native large list fold to succeed, output:\n{}",
        text
    );
    assert!(
        text.contains("5000050000"),
        "expected native large list fold result, output:\n{}",
        text
    );
}

#[cfg(feature = "llvm")]
#[test]
fn test_native_aoc_2025_day2_haskell_style_no_longer_crashes() {
    if !cli_supports_flag("--native") {
        eprintln!("skipping native CLI test: binary does not advertise --native");
        return;
    }

    let file = example_path("aoc/2025/aoc_day2_haskell_style.flx");
    let output = run_flux(&[file.to_str().unwrap(), "--native", "--no-cache"]);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "expected native success for aoc_day2_haskell_style, output:\n{}",
        text
    );
    assert!(
        text.contains("54234399924\n70187097315") || text.contains("54234399924\r\n70187097315"),
        "expected native day2 output, output:\n{}",
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
    let file = example_path("guide/using_modules.flx");
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
fn all_errors_flag_does_not_cross_entry_parse_failure_in_run_mode() {
    let file = example_path("type_system/failing/210_stage_all_errors_flag.flx");

    let default_output = run_flux(&["--no-cache", "--strict", file.to_str().unwrap()]);
    let default_text = combined_output(&default_output);
    assert!(
        !default_output.status.success(),
        "expected fixture to fail in default mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E071]"),
        "expected entry parse diagnostic in default mode, output:\n{}",
        default_text
    );
    assert!(
        !default_text.contains("error[E300]"),
        "entry parse failure should stop before imported type diagnostics, output:\n{}",
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
        all_text.contains("error[E071]"),
        "expected entry parse diagnostic with --all-errors, output:\n{}",
        all_text
    );
    assert!(
        !all_text.contains("error[E300]"),
        "--all-errors should not typecheck imports after an entry parse failure, output:\n{}",
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

    let default_output = run_flux(&["--no-cache", "--strict", file.to_str().unwrap()]);
    let default_text = combined_output(&default_output);
    assert!(
        !default_output.status.success(),
        "expected adversarial compiler fixture to fail in strict mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E300]: Annotation Type Mismatch"),
        "expected visible type diagnostic in strict mode, output:\n{}",
        default_text
    );
    assert!(
        default_text.contains("error[E418]: Strict Effect Annotation Required"),
        "expected effect diagnostic to remain visible in a different module, output:\n{}",
        default_text
    );
    assert!(
        !default_text.contains("Downstream Errors Suppressed"),
        "did not expect a cross-module suppression note in default mode, output:\n{}",
        default_text
    );

    let all_output = run_flux(&[
        "--no-cache",
        "--strict",
        "--all-errors",
        file.to_str().unwrap(),
    ]);
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
        all_text.contains("error[E418]: Strict Effect Annotation Required"),
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
        "E300",
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
    assert!(
        !output.status.success(),
        "expected a type error to fail, output:\n{text}"
    );
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

// ── `flux repl` — interactive session (proposal 0175, Phase 1) ───────────────
//
// The REPL prints evaluation results to stdout and prompts/errors to stderr, so
// stdout carries only the values an expression line produced.

/// Drive a scripted `flux repl` session: feed `script` on stdin and capture the
/// process output. `script` should end each line with `\n` and finish with
/// `:quit` (or rely on EOF).
fn run_flux_repl(script: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_flux"))
        .arg("repl")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `flux repl`");
    child
        .stdin
        .take()
        .expect("repl stdin")
        .write_all(script.as_bytes())
        .expect("write repl script");
    child.wait_with_output().expect("wait for `flux repl`")
}

/// The non-empty stdout lines of a REPL session (the printed results).
fn repl_results(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn repl_evaluates_and_remembers_bindings() {
    // A later line sees an earlier `let`; declarations themselves print nothing.
    let output = run_flux_repl("1 + 2\nlet x = 10\nx + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["3", "11"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_binds_it_to_the_last_result() {
    let output = run_flux_repl("21 * 2\nit + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42", "43"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_rolls_back_a_failed_line() {
    // `nope` is undefined: that line errors and is discarded, so the session
    // stays intact and `x` is still usable on the next line.
    let output = run_flux_repl("let x = 5\nnope + 1\nx + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["6"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !combined_output(&output).contains("panicked"),
        "a bad line must report a diagnostic, not panic, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_reset_clears_the_session() {
    // After `:reset`, the earlier `x` is gone, so `x + 1` errors and prints
    // nothing to stdout.
    let output = run_flux_repl("let x = 5\n:reset\nx + 1\n:quit\n");
    assert!(
        repl_results(&output).is_empty(),
        "stdout should be empty after reset, got:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        combined_output(&output).contains('x'),
        "expected an undefined-name error mentioning `x`, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_type_reports_inferred_types() {
    // `:type` prints `<expr> : <type>`, sees session bindings, and resolves `it`
    // to the last result — interleaved with ordinary evaluation.
    let output = run_flux_repl("let xs = [1, 2, 3]\n:type xs\n21 * 2\n:type it\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["xs : List<Int>", "42", "it : Int"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_type_does_not_evaluate_the_expression() {
    // `:type` of a printing expression reports its type without running it: the
    // type line appears, but the side effect never does.
    let output = run_flux_repl(":type println(\"side effect\")\n:quit\n");
    let results = repl_results(&output);
    assert_eq!(
        results,
        ["println(\"side effect\") : Unit"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        !results.iter().any(|line| line == "side effect"),
        ":type must not execute the expression, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_type_reports_errors_without_evaluating() {
    // A `:type` on an undefined name prints no type line and is reported (not a
    // panic); the session stays intact so a later expression still evaluates.
    let output = run_flux_repl(":type nope\n1 + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !combined_output(&output).contains("panicked"),
        "a bad `:type` must report a diagnostic, not panic, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_runs_effects_once_without_replaying() {
    // Phase 2 (proposal 0176): each line runs only its own delta on the live VM,
    // so an effectful expression's output appears exactly once. The Phase 1
    // engine re-ran the whole session every line and would have replayed it.
    let output = run_flux_repl("println(\"effect\")\n1 + 1\n:quit\n");
    let effect_lines = repl_results(&output)
        .into_iter()
        .filter(|line| line.contains("effect"))
        .count();
    assert_eq!(
        effect_lines,
        1,
        "the effect must run exactly once, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        repl_results(&output).iter().any(|line| line == "2"),
        "the later pure line still evaluates, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_runs_effectful_declaration_once() {
    // A top-level declaration whose initializer is effectful (e.g.
    // `let _ = println("effect")`) used to fail with E413 / E414. The REPL now
    // re-runs it inside a synthesized `main`, so the effect fires exactly once
    // and a later pure line still evaluates (proposal 0176).
    let output = run_flux_repl("let _ = println(\"effect\")\n1 + 1\n:quit\n");
    let effect_lines = repl_results(&output)
        .into_iter()
        .filter(|line| line.contains("effect"))
        .count();
    assert_eq!(
        effect_lines,
        1,
        "the declaration's effect must run exactly once, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        repl_results(&output).iter().any(|line| line == "2"),
        "the later pure line still evaluates, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_rebinds_an_existing_name() {
    // Phase 2 keeps one persistent compiler for the session, so redefining a
    // name shadows the old binding (on a fresh global slot) instead of erroring
    // as a duplicate — `x` reads `1`, then `99` after the rebind.
    let output = run_flux_repl("let x = 1\nx\nlet x = 99\nx\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["1", "99"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_evaluates_top_level_control_flow() {
    // Top-level `if` / `match` emit jumps with absolute targets into the session
    // buffer; the engine runs the full buffer from the line's offset so those
    // targets resolve (a delta run at offset 0 would mis-jump). A non-constant
    // condition keeps the optimizer from folding the branch away.
    let output = run_flux_repl(
        "let c = 1\nif c > 0 { 111 } else { 222 }\nmatch c { 1 -> \"one\", _ -> \"other\" }\n:quit\n",
    );
    assert_eq!(
        repl_results(&output),
        ["111", "\"one\""],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_type_works_after_rebinding() {
    // Rebinding must not leave a duplicate definition in the `:type` record:
    // its fresh re-inference compile would otherwise reject the second `let x`.
    let output = run_flux_repl("let x = 5\nlet x = 99\n:type x + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["x + 1 : Int"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_continues_multiline_forms() {
    // A form left open by a `{` continues onto further physical lines until the
    // delimiters balance, then compiles as one declaration. This exercises the
    // multi-line continuation shared by the interactive and piped input paths.
    let output = run_flux_repl("fn double(n: Int) -> Int {\n  n * 2\n}\ndouble(21)\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42"],
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Write `source` to a uniquely named temp `.flx` file and return its path.
fn repl_temp_flx(tag: &str, source: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("flux_repl_{tag}_{nanos}.flx"));
    std::fs::write(&path, source).expect("write temp flx");
    path
}

#[test]
fn repl_load_brings_file_definitions_into_scope() {
    // `:load` compiles the whole file in one delta, so its definitions are usable
    // on later lines (here `triple` applied to the loaded `base`).
    let file = repl_temp_flx("load", "fn triple(n: Int) -> Int { n * 3 }\nlet base = 7\n");
    let output = run_flux_repl(&format!(
        ":load \"{}\"\ntriple(base)\n:quit\n",
        file.display()
    ));
    let _ = std::fs::remove_file(&file);
    assert_eq!(
        repl_results(&output),
        ["21"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_reload_reapplies_the_loaded_file() {
    // After `:load`, `:reload` re-bootstraps and re-applies the same file, so its
    // definitions remain callable.
    let file = repl_temp_flx("reload", "fn answer() -> Int { 42 }\n");
    let output = run_flux_repl(&format!(
        ":load \"{}\"\n:reload\nanswer()\n:quit\n",
        file.display()
    ));
    let _ = std::fs::remove_file(&file);
    assert_eq!(
        repl_results(&output),
        ["42"],
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_reload_without_load_reports_nothing() {
    let output = run_flux_repl(":reload\n:quit\n");
    assert!(
        combined_output(&output).contains("Nothing to reload"),
        "expected a 'nothing to reload' message, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_load_missing_file_keeps_session() {
    // A failed `:load` reports the error and leaves the session usable: the
    // following `1 + 1` still evaluates.
    let output = run_flux_repl(":load definitely_missing_repl_file.flx\n1 + 1\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["2"],
        "stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        combined_output(&output).contains("cannot read"),
        "expected a 'cannot read' error, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_list_shows_session_declarations() {
    // `:list` echoes the committed session declarations (to stderr). A pure
    // `let` and a `fn` should both appear verbatim.
    let output = run_flux_repl("let a = 1\nfn f(n) { n + 1 }\n:list\n:quit\n");
    let combined = combined_output(&output);
    assert!(
        combined.contains("let a = 1"),
        "`:list` should show the `let`, output:\n{combined}",
    );
    assert!(
        combined.contains("fn f(n) { n + 1 }"),
        "`:list` should show the `fn`, output:\n{combined}",
    );
}

#[test]
fn repl_help_lists_commands() {
    // `:help` and its `:?` alias both print the command list (to stderr).
    for command in [":help", ":?"] {
        let output = run_flux_repl(&format!("{command}\n:quit\n"));
        let combined = combined_output(&output);
        for expected in [":type", ":load", ":reset", ":list", ":quit"] {
            assert!(
                combined.contains(expected),
                "`{command}` should mention `{expected}`, output:\n{combined}",
            );
        }
    }
}

#[test]
fn repl_reset_after_load_clears_loaded_definitions() {
    // `:reset` forgets everything brought in by `:load`: `triple` resolves before
    // the reset (one `21`) and is gone after it (the second call errors, so no
    // second `21`).
    let file = repl_temp_flx("reset_after_load", "fn triple(n: Int) -> Int { n * 3 }\n");
    let output = run_flux_repl(&format!(
        ":load \"{}\"\ntriple(7)\n:reset\ntriple(7)\n:quit\n",
        file.display()
    ));
    let _ = std::fs::remove_file(&file);
    let twenty_ones = repl_results(&output)
        .into_iter()
        .filter(|line| line == "21")
        .count();
    assert_eq!(
        twenty_ones,
        1,
        "`triple` should resolve before `:reset` and be gone after, output:\n{}",
        combined_output(&output),
    );
    assert!(
        combined_output(&output).contains("Session reset."),
        "`:reset` should confirm, output:\n{}",
        combined_output(&output),
    );
}

#[test]
fn repl_effectful_expression_does_not_update_it() {
    // A documented v1 limitation: an effectful expression runs but its result is
    // not captured by `it`. So after `21 * 2` (it = 42), running `println("hi")`
    // leaves `it` at 42, which the final line prints.
    let output = run_flux_repl("21 * 2\nprintln(\"hi\")\nit\n:quit\n");
    assert_eq!(
        repl_results(&output),
        ["42", "\"hi\"", "42"],
        "effectful expression must not rebind `it`, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[test]
fn repl_uses_effect_declared_on_an_earlier_line() {
    // Proposal 0176: a user `effect` declared on one line is usable on later
    // lines (it persists in the session's effect registry instead of resetting
    // to the prelude-only set each compile, which used to report E407).
    let output = run_flux_repl(concat!(
        "effect Audit { log: String -> Int }\n",
        "fn audited() -> Int with Audit { perform Audit.log(\"started\") + 1 }\n",
        "audited() handle Audit { log(resume, message) -> resume(len(message)) }\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["8"],
        "cross-line effect should resolve and run, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_uses_class_and_instance_declared_on_earlier_lines() {
    // Proposal 0176: a user `class` and its `instance`s declared on earlier
    // lines stay in scope, so a later line can dispatch the method across them
    // (previously the class env was rebuilt per compile and the method was
    // reported undefined, E004).
    let output = run_flux_repl(concat!(
        "class Sizeable<a> { fn size(x: a) -> Int }\n",
        "instance Sizeable<Int> { fn size(x) { 1 } }\n",
        "instance Sizeable<String> { fn size(x) { len(x) } }\n",
        "size(42) + size(\"hello\")\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["6"],
        "cross-line class method should dispatch, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_uses_named_field_data_declared_on_an_earlier_line() {
    // Proposal 0176: a `data` type with named fields declared on one line is
    // fully usable on later lines — construction, dot access, and functional
    // spread-update. The session accumulates the `data` declaration so later
    // lines' inference + named-field desugar see its field metadata; without
    // it, construction reported E082/E430 and spread reported E464. (Unblocked
    // by the top-level-frame codegen fix, which makes the spread's desugared
    // `match` compile correctly at REPL top level.)
    let output = run_flux_repl(concat!(
        "data Person { Person { name: String, age: Int } }\n",
        "let alice = Person { name: \"Alice\", age: 30 }\n",
        "alice.age\n",
        "let bob = { ...alice, name: \"Bob\", age: alice.age + 1 }\n",
        "bob.name\n",
        "bob.age\n",
        ":quit\n",
    ));
    assert_eq!(
        repl_results(&output),
        ["30", "\"Bob\"", "31"],
        "cross-line named-field construct / access / spread should work, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn repl_rejects_unknown_field_on_cross_line_named_data() {
    // The accumulated `data` metadata is scoped, not a blanket pass: an unknown
    // field on an earlier-line record still errors (E463) rather than silently
    // resolving — the line is rolled back and produces no value.
    let output = run_flux_repl(concat!(
        "data Point { Point { x: Int, y: Int } }\n",
        "let p = Point { x: 1, y: 2 }\n",
        "p.z\n",
        ":quit\n",
    ));
    assert!(
        repl_results(&output).is_empty(),
        "unknown field must not yield a value, stdout:\n{}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("E463"),
        "expected E463 for unknown field, stderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn run_compiles_top_level_match_and_destructure() {
    // The cached/parallel compile path (module linker) used to fail with
    // "missing global mapping" when a top-level `match` or tuple `let (a, b)`
    // allocated a transient slot in the global namespace. Run a file end to end
    // through `flux <file>` (which exercises that path) and check it executes.
    let file = repl_temp_flx(
        "toplevel_frame",
        "let x = match Some(7) { Some(n) -> n, _ -> 0 }\n\
         let (a, b) = (10, 20)\n\
         fn main() with IO { println(x + a + b) }\n",
    );
    let path = file.to_string_lossy().into_owned();
    let output = run_flux(&[path.as_str()]);
    let _ = std::fs::remove_file(&file);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line.trim() == "37"),
        "expected 37 from top-level match + destructure, stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
}
