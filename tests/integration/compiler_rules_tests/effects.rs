use super::*;

#[test]
fn perform_arg_type_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with IO {
    let _x = (perform Console.print(1)) handle Console {
        print(_resume, _msg) -> None
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn perform_wrong_arity_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with IO {
    (perform Console.print()) handle Console {
        print(resume, _msg) -> resume(())
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn handle_arm_param_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    1 handle Console {
        print(resume) -> resume(())
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn handle_arm_result_mismatch_fails_compile_e300() {
    let code = compile_err(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    1 handle Console {
        print(resume, _msg) -> "oops"
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn valid_handle_with_resume_and_correct_types_compiles() {
    compile_ok_in(
        "test.flx",
        r#"
effect Console {
    print: String -> Int
}
fn run() -> Int with Console {
    perform Console.print("x")
}
fn main() -> Unit with IO {
    let _ = run() handle Console {
        print(resume, _msg) -> resume(1)
    }
}
"#,
    );
}

#[test]
fn polymorphic_effect_op_multiple_instantiations_compile() {
    compile_ok_in(
        "test.flx",
        r#"
effect Id {
    bounce: a -> a
}
fn run() -> Int with Id {
    let x = perform Id.bounce(1)
    let _y = perform Id.bounce("hi")
    x
}
fn main() -> Unit with Id {
    let _ = run() handle Id {
        bounce(resume, v) -> resume(v)
    }
}
"#,
    );
}

#[test]
#[ignore = "fixture uses base functions (print/to_string) not in standalone compiler"]
fn effect_row_order_equivalence_fixture_compiles() {
    let source =
        include_str!("../../../examples/type_system/100_effect_row_order_equivalence_ok.flx");
    compile_ok_in("examples/type_system/100_effect_row_order_equivalence_ok.flx", source);
}

#[test]
fn effect_row_multi_missing_reports_deterministic_first_effect() {
    let source = include_str!(
        "../../../examples/type_system/failing/194_effect_row_multi_missing_deterministic_e400.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E400]"),
        "expected E400 for deterministic multi-missing fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("requires effect `Clock`"),
        "expected deterministic first missing effect `Clock`, got:\n{}",
        rendered
    );
}

#[test]
#[ignore = "fixture uses base functions (print/to_string) not in standalone compiler"]
fn effect_row_subtract_concrete_fixture_compiles() {
    let source =
        include_str!("../../../examples/type_system/101_effect_row_subtract_concrete_ok.flx");
    compile_ok_in("examples/type_system/101_effect_row_subtract_concrete_ok.flx", source);
}

#[test]
#[ignore = "fixture uses base functions (print/to_string) not in standalone compiler"]
fn effect_row_subtract_var_satisfied_fixture_compiles() {
    let source =
        include_str!("../../../examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx");
    compile_ok_in("examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx", source);
}

#[test]
fn effect_row_multivar_disambiguated_fixture_compiles() {
    let source =
        include_str!("../../../examples/type_system/103_effect_row_multivar_disambiguated_ok.flx");
    compile_ok_in("examples/type_system/103_effect_row_multivar_disambiguated_ok.flx", source);
}

#[test]
fn effect_row_absent_ordering_linked_ok_fixture_compiles() {
    let source =
        include_str!("../../../examples/type_system/104_effect_row_absent_ordering_linked_ok.flx");
    compile_ok_in("examples/type_system/104_effect_row_absent_ordering_linked_ok.flx", source);
}

// The CLI hides this E421 behind an earlier E300 (KI-140). This checks that
// the type checker still finds it.
#[test]
fn effect_row_invalid_subtract_reports_e421() {
    let source = include_str!(
        "../../../examples/type_system/failing/195_effect_row_invalid_subtract_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E421]"),
        "expected E421 for invalid subtraction fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Effect Requirement Mismatch"),
        "expected E421 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("I cannot subtract effect `Console`"),
        "expected concrete subtract message for `Console`, got:\n{}",
        rendered
    );
}

#[test]
fn effect_row_unresolved_single_subtract_reports_e419() {
    let source = include_str!(
        "../../../examples/type_system/failing/196_effect_row_subtract_unresolved_single_e419.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E419]"),
        "expected E419 for unresolved single subtraction fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Unresolved Effect Row"),
        "expected E419 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("I cannot resolve the effect variable `e`"),
        "expected unresolved variable `e` message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("constraint source: effect-row checking for"),
        "expected call-site provenance in E419, got:\n{}",
        rendered
    );
}

#[test]
fn effect_row_unresolved_multi_subtract_reports_e420() {
    let source = include_str!(
        "../../../examples/type_system/failing/197_effect_row_subtract_unresolved_multi_e304.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E420]"),
        "expected E420 for unresolved multi subtraction fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Unresolved Effect Row"),
        "expected E420 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("e, t"),
        "expected sorted unresolved variable list (`e, t`), got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("constraint source: effect-row checking for"),
        "expected call-site provenance in E420, got:\n{}",
        rendered
    );
}

#[test]
fn effect_row_subset_unsatisfied_reports_e422() {
    let source = include_str!(
        "../../../examples/type_system/failing/198_effect_row_subset_unsatisfied_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E422]"),
        "expected E422 for unsatisfied subset fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Effect Requirement Mismatch"),
        "expected E422 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("missing required effects: Clock"),
        "expected missing `Clock` subset message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("constraint source: effect-row checking for"),
        "expected call-site provenance in E422, got:\n{}",
        rendered
    );
}

#[test]
fn effect_row_subset_missing_list_is_sorted() {
    let source = include_str!(
        "../../../examples/type_system/failing/199_effect_row_subset_ordered_missing_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E422]"),
        "expected E422 for ordered missing-list fixture, got:\n{}",
        rendered
    );
    // `IO` and `Time` are effect aliases, so the row solver reports their
    // atoms rather than the alias names (KI-094). The list is still sorted,
    // which is what this test is about.
    assert!(
        rendered.contains("Clock, Console, Env, FileSystem, Process, Stdin"),
        "expected sorted missing effects list (the atoms of `IO, Time`), got:\n{}",
        rendered
    );
}

#[test]
fn effect_row_absent_ordering_linked_violation_reports_e421() {
    let source = include_str!(
        "../../../examples/type_system/failing/200_effect_row_absent_ordering_linked_violation_e421.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E421]"),
        "expected E421 for deferred-absent ordering fixture, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Effect Requirement Mismatch"),
        "expected E421 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("I cannot subtract effect `Console`"),
        "expected deferred absent violation message for `Console`, got:\n{}",
        rendered
    );
}

#[test]
fn member_access_non_module_path_currently_compiles_without_strict_mode() {
    compile_ok_in(
        "examples/test.flx",
        r#"
fn main() -> Unit {
    let h = { "a": 1 }
    let x: Int = h.a
}
"#,
    );
}

#[test]
fn strict_unresolved_generic_boundary_has_stable_diagnostic_shape() {
    let rendered = compile_err_strict_rendered(
        r#"
public fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    id(1)
}
"#,
    );
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 in rendered diagnostics:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Unresolved Boundary Type"),
        "expected title in rendered diagnostics:\n{}",
        rendered
    );
    assert!(
        rendered.contains("unresolved expression type in function return expression"),
        "expected unresolved-context message in rendered diagnostics:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_perform_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with IO {
    perform I.print("x")
}
"#,
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in perform diagnostic, got:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_function_annotation_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with I {
}
"#,
    );
    assert!(
        rendered.contains("error[E407]"),
        "expected E407 in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("error[E407]: Unknown Effect"),
        "expected E407 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in function-annotation diagnostic, got:\n{}",
        rendered
    );
}

#[test]
fn unknown_effect_in_handle_has_did_you_mean_hint() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit with IO {
    None handle I {
        print(resume, _msg) -> resume(())
    }
}
"#,
    );
    assert!(
        rendered.contains("did you mean `IO`?"),
        "expected effect suggestion hint in handle diagnostic, got:\n{}",
        rendered
    );
}
