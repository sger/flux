use super::*;

// ---------------------------------------------------------------------------
// Syntax test fixtures
// ---------------------------------------------------------------------------

fn run_syntax_fixture(name: &str, expected_count: u32) {
    let file = fixture_path(name);
    let output = run_flux(&["--test", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected all tests to pass in {}, output:\n{}", name, text);

    let summary = format!("{} tests: {} passed, 0 failed", expected_count, expected_count);
    assert!(text.contains(&summary), "expected '{}' in {}, output:\n{}", summary, name, text);
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

    assert!(output.status.success(), "expected dump-core success, output:\n{}", text);
    assert!(text.contains("def main ="), "expected readable Core dump header, output:\n{}", text);
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
    assert!(!text.contains("\n3\n"), "dump-core should not execute the program, output:\n{}", text);
}

#[test]
fn dump_core_debug_preserves_raw_identity_details() {
    let file = example_path("guide/arithmetic.flx");
    let output = run_flux(&["--dump-core=debug", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected dump-core debug success, output:\n{}", text);
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

    assert!(output.status.success(), "expected dump-cfg success, output:\n{}", text);
    assert!(
        text.contains("fn #") || text.contains("fn <anon>"),
        "expected CFG function header, output:\n{}",
        text
    );
    assert!(text.contains("Return"), "expected CFG terminator, output:\n{}", text);
    assert!(!text.contains("\n3\n"), "dump-cfg should not execute the program, output:\n{}", text);
}

#[test]
fn dump_core_debug_excludes_aether_stats() {
    let file = example_path("aether/verify_aether.flx");
    let output = run_flux(&["--dump-core=debug", file.to_str().unwrap()]);
    let text = combined_output(&output);

    assert!(output.status.success(), "expected dump-core debug success, output:\n{}", text);
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

    assert!(output.status.success(), "expected dump-core debug success, output:\n{}", text);
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

    let (_files, file) = write_temp_flux_file(
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

    let all_output = run_flux(&["--no-cache", "--strict", "--all-errors", file.to_str().unwrap()]);
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
