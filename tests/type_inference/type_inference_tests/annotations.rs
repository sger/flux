use super::*;

#[test]
fn infer_call_arg_named_fn_emits_contextual_e300() {
    let source = r#"
fn greet(name: String) -> String { name }
fn main() -> Unit {
    let _x = greet(42)
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(&result, "wrong type in the 1st argument to `greet`"),
        "expected named call-arg contextual message, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_call_arg_contextual_primary_label_uses_argument_span() {
    let source = r#"
fn pair(a: Int, b: Int) -> Int { a + b }
fn main() -> Unit {
    let _x = pair(1, "oops")
}
"#;
    let (result, program) = infer_program_from_source(source);
    let expected_arg_span = match &program.statements[1] {
        Statement::Function { body, .. } => match &body.statements[0] {
            Statement::Let { value: Expression::Call { arguments, .. }, .. } => arguments[1].span(),
            other => panic!("unexpected function body statement shape: {other:?}"),
        },
        other => panic!("unexpected program statement shape: {other:?}"),
    };

    let diag = result
        .diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message()
                    .is_some_and(|m| m.contains("wrong type in the 2nd argument to `pair`"))
        })
        .expect("expected contextual call-arg E300 diagnostic");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == flux::diagnostics::LabelStyle::Primary)
        .expect("expected primary label on call-arg diagnostic");
    assert_eq!(
        primary.span, expected_arg_span,
        "expected primary label span to match mismatching argument expression span"
    );
    assert!(
        diag.hints().iter().any(|hint| hint
            .text
            .contains("actual argument type is inferred from this expression")),
        "expected origin note explaining conflicting type source, got: {:?}",
        diag.hints()
    );
}

#[test]
fn infer_call_arg_anonymous_fn_emits_contextual_e300_without_name() {
    let source = r#"
fn greet(name: String) -> String { name }
fn main() -> Unit {
    let _x = (if true { greet } else { greet })(42)
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(&result, "wrong type in the 1st argument to this function"),
        "expected anonymous call-arg contextual message, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_call_with_unresolved_callee_does_not_emit_contextual_callarg_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = unknown_fn(42)
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(&result, "argument to `"),
        "did not expect contextual call-arg mismatch for unresolved callee, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn base_missing_hm_metadata_emits_e426() {
    let source = r#"
fn id(x: Int) -> Int { x }
fn main() -> Unit {
    let _ = map([|1, 2, 3|], id)
}
"#;
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut interner_for_base = interner.clone();
    let map = interner_for_base.intern("map");
    let base = interner_for_base.intern("Flow");
    let known_flow_names = HashSet::from([map]);
    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: Some("<test>".into()),

            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            task_module_bindings: HashSet::new(),
            task_spawn_exposed: false,
            known_flow_names,
            flow_module_symbol: base,
            preloaded_effect_op_signatures: HashMap::new(),
            effect_row_aliases: HashMap::new(),
            class_env: None,
            preloaded_adt_data: Vec::new(),
        },
    );
    assert!(
        has_diagnostic_code(&result, "E426"),
        "expected E426 diagnostics, got: {:#?}",
        result.diagnostics
    );
}

// ── Hash literal homogeneity (collections.rs:infer_hash_literal_expression) ──

#[test]
fn infer_hash_literal_homogeneous_values_ok() {
    let source = r#"
let m = {"a": 1, "b": 2}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 for homogeneous hash literal: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_hash_literal_heterogeneous_values_emits_e300() {
    // Second pair's value is Float while the first established Int;
    // unification of subsequent value types must flag the mismatch.
    let source = r#"
let m = {"a": 1, "b": 2.5}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for heterogeneous hash values, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_hash_literal_heterogeneous_keys_emits_e300() {
    let source = r#"
let m = {"a": 1, 2: 2}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for heterogeneous hash keys, got: {:#?}",
        result.diagnostics
    );
}

// ── Let-binding annotation enforcement (statement.rs:check_let_annotation) ──

#[test]
fn infer_let_annotation_array_element_mismatch_emits_e300() {
    let source = r#"
let xs: Array<Int> = [|0.1, 0.2|]
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for Array<Int> annotation vs Array<Float> value, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_list_element_mismatch_emits_e300() {
    let source = r#"
let xs: List<Int> = [1.5, 2.5]
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for List<Int> annotation vs List<Float> value, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_primitive_mismatch_emits_e300() {
    let source = r#"
let x: Int = "hello"
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for Int annotation vs String value, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_matching_type_ok() {
    let source = r#"
let x: Int = 42
let xs: List<Int> = [1, 2, 3]
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 for matching annotations: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_empty_list_stays_polymorphic() {
    // An empty list literal has type List<'a>; the annotation should
    // instantiate 'a without raising a mismatch.
    let source = r#"
let xs: List<Int> = []
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 for empty list annotated List<Int>: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_match_expression_mismatch_emits_e300() {
    // Regression: prior to HM-authoritative annotation checking, a match
    // whose arms unified to Int was silently accepted as String.
    let source = r#"
fn main() -> Unit {
    let x: String = match Some(1) {
        Some(v) -> v,
        None -> 0,
    }
    x
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for String annotation vs Int match value, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_naming_the_signatures_own_type_parameter_is_accepted() {
    // KI-058: the annotation's `a` and the signature's `a` must be the same
    // rigid variable. With an empty type-parameter map the annotation
    // converted to a nominal type *named* `a` instead, so this reported E300
    // against itself — and the rendered message showed `a` on both sides.
    let source = r#"
fn annotated<a>(x: a) -> a {
    let y: a = x
    y
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "annotating a let with the signature's own type parameter is correct, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_still_reports_a_genuine_mismatch_under_a_type_parameter() {
    // The counterpart: carrying the signature's parameters must not make
    // every annotation vacuous. `Int` and the rigid `a` are still distinct.
    let source = r#"
fn annotated<a>(x: a) -> a {
    let y: Int = x
    x
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300") || has_diagnostic_code(&result, "E305"),
        "expected a mismatch for Int annotation vs rigid `a`, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_let_annotation_type_parameter_does_not_leak_into_a_sibling_function() {
    // The parameters are held on a stack, so `a` is in scope only inside the
    // function that declares it. In `plain`, `a` is an ordinary unknown type
    // name, not `generic`'s rigid variable.
    let source = r#"
fn generic<a>(x: a) -> a {
    let y: a = x
    y
}

fn plain(n: Int) -> Int {
    let z: a = n
    n
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !result.diagnostics.is_empty(),
        "`a` is not in scope in `plain`, so its annotation should not resolve: {:#?}",
        result.diagnostics
    );
}

// ── Return annotation enforcement (function.rs:check_return_annotation) ──

#[test]
fn infer_return_annotation_primitive_mismatch_emits_e300() {
    let source = r#"
fn f() -> Int { "hello" }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for Int return vs String body, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_return_annotation_collection_mismatch_emits_e300() {
    let source = r#"
fn f() -> List<Int> { [1.5, 2.5] }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for List<Int> return vs List<Float> body, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_return_annotation_matching_ok() {
    let source = r#"
fn f() -> Int { 42 }
fn g() -> List<Int> { [1, 2, 3] }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 for matching return annotations: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_return_annotation_polymorphic_body_skipped() {
    // When the body type contains free variables, skip the E300 check so
    // that generic/forward-reference cases don't fire false positives;
    // downstream compiler boundary checks remain the fallback.
    let source = r#"
fn ident<a>(x: a) -> a { x }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 for polymorphic identity function: {:#?}",
        result.diagnostics
    );
}

// ── Parameter annotation lowering (function.rs:infer_and_bind_parameter_types) ──
// Note: `convert_type_expr_rec` treats unknown named types as ADT stubs rather
// than returning `None`, so E303 currently only fires for cases the parser
// can't produce (e.g. multiple distinct row variables on the same row). The
// happy path — well-formed annotations — must not emit E303.

#[test]
fn infer_param_annotation_well_formed_no_e303() {
    let source = r#"
fn f(x: Int, y: List<Int>) -> Int { x }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E303"),
        "unexpected E303 for well-formed parameter annotations: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_param_annotation_unknown_name_treated_as_adt_no_e303() {
    // Unknown bare type names (e.g. `Widget`) are lowered as ADT stubs by
    // `convert_type_expr_rec`, so they don't trip E303. They surface later
    // as unification failures or unresolved-reference errors instead.
    let source = r#"
fn f(x: Widget) -> Int { 0 }
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E303"),
        "unknown type names should not trigger E303 (they become ADT stubs), got: {:#?}",
        result.diagnostics
    );
}

// ── Effect row annotation (effects.rs:infer_effect_row) ──

#[test]
fn infer_effect_row_multiple_row_vars_emits_e304() {
    // Mixing two distinct row variables in one `with` clause is
    // semantically ambiguous — HM now reports E304 at the annotation site
    // instead of silently defaulting to a closed empty row.
    let source = r#"
fn unresolved_many(x: Int) -> Int with |e - IO, |t - Time {
    x
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E304"),
        "expected E304 for distinct row variables in the same row, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_effect_row_single_row_var_ok() {
    let source = r#"
fn f(x: Int) -> Int with |e - IO {
    x
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E304"),
        "unexpected E304 for single-row-var annotation: {:#?}",
        result.diagnostics
    );
}
