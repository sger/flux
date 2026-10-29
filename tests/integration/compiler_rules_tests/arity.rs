use super::*;

#[test]
fn known_call_too_many_args_emits_e056() {
    let code = compile_err(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2, 3)
}
"#,
    );
    assert_eq!(code, "E056");

    let rendered = compile_err_rendered(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2, 3)
}
"#,
    );
    assert!(
        rendered.contains("error[E056]: Wrong Number Of Arguments"),
        "expected E056 title in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Remove 1 extra argument(s), for example: `add(arg1, arg2)`."),
        "expected actionable too-many-args hint in rendered diagnostics, got:\n{}",
        rendered
    );
}

#[test]
fn known_call_too_few_args_emits_e056() {
    let code = compile_err(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1)
}
"#,
    );
    assert_eq!(code, "E056");

    let rendered = compile_err_rendered(
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1)
}
"#,
    );
    assert!(
        rendered.contains("takes 2 arguments, but 1 were provided"),
        "expected contextual arity text in rendered diagnostics, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Add 1 missing argument(s), for example: `add(arg1, arg2)`."),
        "expected actionable too-few-args hint in rendered diagnostics, got:\n{}",
        rendered
    );
}

#[test]
fn known_call_correct_arity_no_e056() {
    compile_ok_in(
        "<unknown>",
        r#"
fn add(a: Int, b: Int) -> Int {
    a + b
}
fn main() -> Unit {
    let _x = add(1, 2)
}
"#,
    );
}

#[test]
fn pass2_multi_error_continuation_reports_independent_errors_in_order() {
    let source =
        include_str!("../../../examples/type_system/failing/99_multi_error_continuation.flx");
    let rendered = compile_err_rendered(source);

    let e002_idx = rendered
        .find("error[E002]")
        .expect("expected E002 immutability error in rendered diagnostics");
    let e300_idx = rendered
        .find("error[E300]")
        .expect("expected E300 type-unification error in rendered diagnostics");

    assert!(
        e002_idx < e300_idx,
        "expected deterministic source-order diagnostics (E002 before E300), got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_161_tuple_destructure_concrete_mismatch_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/161_tuple_destructure_concrete_mismatch_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 161, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 for concrete destructure mismatch fixture 161, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_162_tuple_destructure_unresolved_strict_e425() {
    let source = include_str!(
        "../../../examples/type_system/failing/162_tuple_destructure_unresolved_e004.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for unresolved destructure fixture 162, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_163_match_concrete_disagreement_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/163_match_concrete_disagreement_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 163, got:\n{}", rendered);
}

#[test]
fn hm_fixture_164_match_unresolved_arm_stays_suppressed() {
    let source = include_str!(
        "../../../examples/type_system/failing/164_match_unresolved_arm_stays_suppressed.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        !rendered.contains("The arms of this `match` expression do not agree on a type."),
        "did not expect contextual match-arm mismatch for unresolved-arm fixture 164, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_165_self_recursive_precision_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/165_self_recursive_precision_prefers_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 165, got:\n{}", rendered);
}

#[test]
fn hm_fixture_166_self_recursive_guard_stable_unresolved() {
    let source = include_str!(
        "../../../examples/type_system/failing/166_self_recursive_guard_stable_unresolved.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E004]"),
        "expected unresolved symbol baseline for fixture 166, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("Return Type Mismatch"),
        "did not expect recursion hardening to introduce unrelated return-mismatch noise for fixture 166, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_167_tuple_destructure_ordered_concrete_conflict_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/167_tuple_destructure_ordered_concrete_conflict_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 167, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 for concrete tuple-destructure conflict fixture 167, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_168_tuple_destructure_unresolved_guard_strict_e425() {
    let source = include_str!(
        "../../../examples/type_system/failing/168_tuple_destructure_unresolved_guard_e004.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E425]"),
        "expected E425 for unresolved tuple-destructure fixture 168, got:\n{}",
        rendered
    );
}

#[test]
fn unresolved_name_fixtures_report_e004_even_in_strict_modes() {
    let unresolved_name_fixtures = [
        include_str!("../../../examples/type_system/failing/154_unresolved_projection_e004.flx"),
        include_str!("../../../examples/type_system/failing/155_unresolved_member_access_e004.flx"),
        include_str!("../../../examples/type_system/failing/156_unresolved_call_arg_e004.flx"),
    ];

    for source in unresolved_name_fixtures {
        let strict_rendered = compile_err_strict_rendered(source);
        assert!(
            strict_rendered.contains("error[E004]"),
            "expected strict E004 for unresolved-name fixture, got:\n{}",
            strict_rendered
        );

        let non_strict_rendered = compile_err_rendered(source);
        assert!(
            non_strict_rendered.contains("error[E004]"),
            "expected non-strict unresolved baseline E004 for unresolved-name fixture, got:\n{}",
            non_strict_rendered
        );
    }
}

#[test]
fn strict_only_hm_boundary_fixtures_still_report_e425_in_strict_modes() {
    let strict_boundary_fixtures = [
        include_str!(
            "../../../examples/type_system/failing/162_tuple_destructure_unresolved_e004.flx"
        ),
        include_str!(
            "../../../examples/type_system/failing/168_tuple_destructure_unresolved_guard_e004.flx"
        ),
    ];

    for source in strict_boundary_fixtures {
        let strict_rendered = compile_err_strict_rendered(source);
        assert!(
            strict_rendered.contains("error[E425]"),
            "expected strict E425 for HM boundary fixture, got:\n{}",
            strict_rendered
        );

        let non_strict_rendered = compile_err_rendered(source);
        assert!(
            non_strict_rendered.contains("error[E004]"),
            "expected non-strict unresolved baseline E004 for HM boundary fixture, got:\n{}",
            non_strict_rendered
        );
    }
}

#[test]
fn hm_fixture_169_match_disagreement_first_arm_unresolved_still_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/169_match_disagreement_first_arm_unresolved_still_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 169, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect E425 to mask concrete match disagreement in fixture 169, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_170_match_disagreement_all_concrete_ordering_invariant_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/170_match_disagreement_all_concrete_ordering_invariant_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 170, got:\n{}", rendered);
}

#[test]
fn hm_fixture_171_self_recursive_refinement_concrete_chain_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/171_self_recursive_refinement_concrete_chain_e300.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 171, got:\n{}", rendered);
}

#[test]
fn hm_fixture_172_self_recursive_unresolved_guard_no_false_positive() {
    let source = include_str!(
        "../../../examples/type_system/failing/172_self_recursive_unresolved_guard_no_false_positive.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E004]"),
        "expected unresolved symbol baseline for fixture 172, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("Return Type Mismatch"),
        "did not expect recursion hardening to introduce unrelated return-mismatch noise for fixture 172, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_param_mismatch_reports_param_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn takes_int(x: Int) -> Int { x }
fn takes_string(x: String) -> Int { 0 }
fn main() -> Unit {
    let _f = if true {
        takes_int
    } else {
        takes_string
    }
}
"#,
    );
    assert!(
        rendered.contains("Parameter 1 has the wrong type."),
        "expected function parameter mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_return_mismatch_reports_return_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn ret_int() -> Int { 1 }
fn ret_string() -> String { "x" }
fn main() -> Unit {
    let _f = if true {
        ret_int
    } else {
        ret_string
    }
}
"#,
    );
    assert!(
        rendered.contains("The body of this function does not match its return type."),
        "expected function return mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fun_arity_mismatch_reports_arity_specific_message() {
    let rendered = compile_err_rendered(
        r#"
fn one_arg(x: Int) -> Int { x }
fn two_args(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit {
    let _f = if true {
        one_arg
    } else {
        two_args
    }
}
"#,
    );
    assert!(
        rendered.contains("too many arguments"),
        "expected function arity mismatch text, got:\n{}",
        rendered
    );
}
