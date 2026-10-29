use super::*;

#[test]
fn strict_public_function_typed_contract_is_supported() {
    compile_ok_strict(
        r#"
fn is_positive(n: Int) -> Bool { n > 0 }
public fn apply(f: (Int) -> Bool, x: Int) -> Bool {
    f(x)
}
fn main() -> Unit {
    let _ = apply(is_positive, 1)
}
"#,
    );
}

#[test]
fn strict_unresolved_generic_boundary_reports_error() {
    let code = compile_err_strict(
        r#"
public fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    id(1)
}
"#,
    );
    assert_eq!(code, "E425");
}

#[test]
fn hm_polymorphic_reuse_across_concrete_types_ok() {
    compile_ok_in(
        "test.flx",
        r#"
fn id<T>(x: T) -> T { x }
fn main() -> Unit {
    let n = id(1)
    let s = id("ok")
}
"#,
    );
}

#[test]
fn hm_typed_mixed_numeric_add_is_compile_mismatch() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let x: Int = 1 + 2.5
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn hm_pattern_binding_from_some_is_constrained() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let x: String = match Some(1) {
        Some(v) -> v,
        None -> 0,
    }
}
"#,
    );
    assert_eq!(code, "E300");
}

#[test]
fn hm_if_concrete_branch_mismatch_reports_contextual_message() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#,
    );
    assert_eq!(code, "E300");

    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#,
    );
    assert!(
        rendered.contains("The branches of this `if` expression do not agree on a type."),
        "expected contextual if mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_if_unresolved_branch_does_not_report_contextual_e300() {
    let unresolved_rendered = compile_rendered_or_empty(
        r#"
fn main() -> Unit {
    let _x = if true { 42 } else { mystery_value }
}
"#,
    );
    assert!(
        !unresolved_rendered
            .contains("The branches of this `if` expression do not agree on a type."),
        "did not expect contextual if mismatch text for unresolved branch, got:\n{}",
        unresolved_rendered
    );
}

#[test]
fn hm_fixture_134_if_concrete_branch_mismatch_reports_contextual_message() {
    let source =
        include_str!("../../../examples/type_system/failing/134_if_concrete_branch_mismatch.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("The branches of this `if` expression do not agree on a type."),
        "expected contextual if mismatch text for fixture 134, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_135_if_unresolved_branch_suppresses_contextual_message() {
    let source =
        include_str!("../../../examples/type_system/failing/135_if_any_branch_suppressed.flx");
    let rendered = compile_err_rendered(source);
    assert!(
        !rendered.contains("The branches of this `if` expression do not agree on a type."),
        "did not expect contextual if mismatch text for fixture 135, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("E056"),
        "expected fixture 135 to fail due to follow-up known call arity error, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_136_tuple_projection_uses_precise_hm_type() {
    let source = include_str!(
        "../../../examples/type_system/failing/136_tuple_projection_precise_mismatch.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 136, got:\n{}", rendered);
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("Int")
            && rendered.contains("String"),
        "expected typed-let mismatch details with Int/String for fixture 136, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("error[E425]"),
        "known tuple projection should not be unresolved in fixture 136, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_137_tuple_projection_unresolved_behavior_is_stable() {
    let source = include_str!(
        "../../../examples/type_system/failing/137_tuple_projection_unresolved_path_unchanged.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E004]"),
        "expected unresolved-name error for fixture 137, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("does not match its type annotation"),
        "did not expect contextual typed-let mismatch noise for unresolved fixture 137, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_138_match_scrutinee_constraint_propagates() {
    let source = include_str!(
        "../../../examples/type_system/failing/138_match_scrutinee_constraint_propagates.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 138, got:\n{}", rendered);
    assert!(
        rendered.contains("does not match its type annotation")
            && rendered.contains("String")
            && rendered.contains("Int"),
        "expected constrained downstream typed-let mismatch details for fixture 138, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_139_match_scrutinee_constraint_mixed_family_no_propagation() {
    let source = include_str!(
        "../../../examples/type_system/failing/139_match_scrutinee_constraint_no_propagation_mixed_family.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(
        rendered.contains("error[E056]"),
        "expected independent follow-up E056 for fixture 139, got:\n{}",
        rendered
    );
    assert!(
        !rendered.contains("The arms of this `match` expression do not agree on a type."),
        "did not expect contextual match-arm mismatch noise for fixture 139, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_142_bool_missing_true_emits_e015() {
    let source =
        include_str!("../../../examples/type_system/failing/142_match_bool_missing_true.flx");
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E015]"), "expected E015 for fixture 142, got:\n{}", rendered);
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 142, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_143_bool_missing_false_emits_e015() {
    let source =
        include_str!("../../../examples/type_system/failing/143_match_bool_missing_false.flx");
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E015]"), "expected E015 for fixture 143, got:\n{}", rendered);
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 143, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_144_guarded_wildcard_only_targeted_message() {
    let source = include_str!(
        "../../../examples/type_system/failing/144_guarded_wildcard_only_non_exhaustive_targeted.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E015]"), "expected E015 for fixture 144, got:\n{}", rendered);
    assert!(
        rendered.contains("not exhaustive"),
        "expected non-exhaustive message for fixture 144, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_151_array_literal_concrete_conflict_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/151_array_literal_concrete_conflict_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 151, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect strict unresolved E425 when concrete array mismatch already exists in fixture 151, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_152_array_literal_callarg_conflict_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/152_array_literal_callarg_conflict_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 152, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E425]"),
        "did not expect strict unresolved E425 when concrete array arg mismatch already exists in fixture 152, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_153_match_branch_conflict_prefers_e300() {
    let source = include_str!(
        "../../../examples/type_system/failing/153_match_branch_conflict_prefers_e300.flx"
    );
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E300]"), "expected E300 for fixture 153, got:\n{}", rendered);
}

#[test]
fn hm_fixture_154_unresolved_projection_reports_undefined_name_first() {
    let source =
        include_str!("../../../examples/type_system/failing/154_unresolved_projection_e004.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E004]"), "expected E004 for fixture 154, got:\n{}", rendered);
}

#[test]
fn hm_fixture_155_unresolved_member_access_reports_undefined_name_first() {
    let source =
        include_str!("../../../examples/type_system/failing/155_unresolved_member_access_e004.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E004]"), "expected E004 for fixture 155, got:\n{}", rendered);
}

#[test]
fn hm_fixture_156_unresolved_call_arg_reports_undefined_name_first() {
    let source =
        include_str!("../../../examples/type_system/failing/156_unresolved_call_arg_e004.flx");
    let rendered = compile_err_strict_rendered(source);
    assert!(rendered.contains("error[E004]"), "expected E004 for fixture 156, got:\n{}", rendered);
    assert!(
        !rendered.contains("error[E300]"),
        "did not expect concrete E300 for unresolved call-arg fixture 156, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_146_constructor_pattern_arity_some_too_many() {
    let source = include_str!(
        "../../../examples/type_system/failing/146_constructor_pattern_arity_some_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E085]"), "expected E085 for fixture 146, got:\n{}", rendered);
}

#[test]
fn hm_fixture_147_constructor_pattern_arity_none_too_many() {
    let source = include_str!(
        "../../../examples/type_system/failing/147_constructor_pattern_arity_none_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E085]"), "expected E085 for fixture 147, got:\n{}", rendered);
}

#[test]
fn hm_fixture_148_constructor_pattern_arity_left_too_many() {
    let source = include_str!(
        "../../../examples/type_system/failing/148_constructor_pattern_arity_left_too_many.flx"
    );
    let rendered = compile_err_rendered(source);
    assert!(rendered.contains("error[E085]"), "expected E085 for fixture 148, got:\n{}", rendered);
}

#[test]
fn hm_fixture_149_cross_module_constructor_access_strict() {
    let source = r#"
module M { type MaybeInt = SomeInt(Int) | NoneInt }
fn main() { M.SomeInt(1); }
"#;
    let rendered = compile_err_strict_rendered(source);
    assert!(
        rendered.contains("error[E086]"),
        "expected E086 for fixture 149 in strict mode, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_150_cross_module_constructor_access_nonstrict_warning() {
    let source = r#"
module M { type MaybeInt = SomeInt(Int) | NoneInt }
fn main() { M.SomeInt(1); }
"#;
    let warnings = compile_ok_with_warnings_in("examples/test.flx", source, false);
    assert!(
        warnings.iter().any(|d| d.code() == Some("W201")),
        "expected W201 warning for fixture 150, got: {:?}",
        warnings.iter().map(|d| (d.code(), d.title().to_string())).collect::<Vec<_>>()
    );
}

#[test]
fn hm_match_concrete_arm_mismatch_reports_contextual_message() {
    let code = compile_err(
        r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
    }
}
"#,
    );
    assert_eq!(code, "E300");

    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
    }
}
"#,
    );
    assert!(
        rendered.contains("The arms of this `match` expression do not agree on a type."),
        "expected contextual match-arm mismatch text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_call_arg_named_fn_reports_contextual_message_and_definition_site() {
    let rendered = compile_err_rendered(
        r#"
fn greet(name: String) -> String { name }
fn main() -> Unit {
    let _x = greet(42)
}
"#,
    );
    assert!(
        rendered.contains("wrong type in the 1st argument to `greet`"),
        "expected named call-arg mismatch message, got:\n{}",
        rendered
    );
}

/// The line a fixture marks with its `//~` annotation, so these tests follow
/// the fixture when lines above the error move.
fn annotated_line(source: &str) -> usize {
    source.lines().position(|line| line.contains("//~ E")).expect("a `//~` annotation") + 1
}

#[test]
fn hm_fixture_131_call_arg_primary_label_is_argument_subspan() {
    let source =
        include_str!("../../../examples/type_system/failing/131_call_arg_span_precision.flx");
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message()
                    .is_some_and(|m| m.contains("wrong type in the 2nd argument to `pair`"))
        })
        .expect("expected contextual call-arg E300 diagnostic for fixture 131");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 131 diagnostic");
    assert_eq!(primary.span.start.line, annotated_line(source));
    assert_eq!(primary.span.start.column, 21);
}

#[test]
fn hm_call_arg_unresolved_paths_do_not_report_contextual_message() {
    let unresolved_rendered = compile_rendered_or_empty(
        r#"
fn main() -> Unit {
    let _x = unknown_fn(42)
}
"#,
    );
    assert!(
        !unresolved_rendered.contains("argument to `"),
        "did not expect contextual call-arg mismatch text for unresolved callee, got:\n{}",
        unresolved_rendered
    );
}

#[test]
fn hm_let_annotation_mismatch_reports_dual_labels() {
    let rendered = compile_err_rendered(
        r#"
fn main() -> Unit {
    let x: Int = "hello"
}
"#,
    );
    assert!(
        rendered.contains("The value of `x` does not match its type annotation."),
        "expected contextual let annotation mismatch message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Change `x` to a `Int` value or update the annotation."),
        "expected actionable let-annotation help text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_132_let_annotation_primary_label_is_initializer_subspan() {
    let source = include_str!(
        "../../../examples/type_system/failing/132_let_initializer_span_precision.flx"
    );
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The value of `x` does not match its type annotation.")
                })
        })
        .expect("expected contextual let-annotation E300 diagnostic for fixture 132");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 132 diagnostic");
    assert_eq!(primary.span.start.line, annotated_line(source));
    assert_eq!(primary.span.start.column, 17);
}

#[test]
fn hm_function_return_annotation_mismatch_reports_dual_labels() {
    let rendered = compile_err_rendered(
        r#"
fn add() -> Int {
    "oops"
}
fn main() -> Unit { add() }
"#,
    );
    assert!(
        rendered.contains("The body of `add` does not match its declared return type."),
        "expected contextual return-annotation mismatch message, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("Return a `Int` value from `add` or change its annotation."),
        "expected actionable return-annotation help text, got:\n{}",
        rendered
    );
}

#[test]
fn hm_fixture_133_if_primary_label_is_else_value_subspan() {
    let source = include_str!(
        "../../../examples/type_system/failing/133_if_branch_value_span_precision.flx"
    );
    let diagnostics = compile_err_diagnostics(source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The branches of this `if` expression do not agree on a type.")
                })
        })
        .expect("expected contextual if-branch E300 diagnostic for fixture 133");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .expect("expected primary label on fixture 133 diagnostic");
    let secondary = diag
        .labels()
        .iter()
        .find(|l| l.style == LabelStyle::Secondary)
        .expect("expected secondary label on fixture 133 diagnostic");
    assert_eq!(primary.span.start.line, annotated_line(source));
    assert_eq!(primary.span.start.column, 35);
    assert_eq!(secondary.span.start.line, annotated_line(source));
    assert_eq!(secondary.span.start.column, 23);
}
