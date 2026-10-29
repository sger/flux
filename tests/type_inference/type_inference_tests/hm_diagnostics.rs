use super::*;

fn first_match_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Match { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If { condition, consequence, alternative, .. } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call { function, arguments, .. } => {
                walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr))
            }
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => {
                pairs.iter().find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v)))
            }
            Expression::Index { left, index, .. } => walk_expr(left).or_else(|| walk_expr(index)),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => walk_expr(object),
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => walk_expr(value),
            Expression::Cons { head, tail, .. } => walk_expr(head).or_else(|| walk_expr(tail)),
            Expression::Handle { expr, arms, .. } => {
                walk_expr(expr).or_else(|| arms.iter().find_map(|arm| walk_expr(&arm.body)))
            }
            _ => None,
        }
    }

    fn walk_stmt(statement: &Statement) -> Option<&Expression> {
        match statement {
            Statement::Let { value, .. }
            | Statement::LetDestructure { value, .. }
            | Statement::Assign { value, .. } => walk_expr(value),
            Statement::Return { value, .. } => value.as_ref().and_then(walk_expr),
            Statement::Expression { expression, .. } => walk_expr(expression),
            Statement::Function { body, .. } | Statement::Module { body, .. } => walk_block(body),
            _ => None,
        }
    }

    fn walk_block(block: &flux::syntax::block::Block) -> Option<&Expression> {
        block.statements.iter().find_map(walk_stmt)
    }

    program.statements.iter().find_map(walk_stmt)
}

#[test]
fn infer_adt_constructor_call_generic_ok() {
    let source = r#"
type Result<T, E> = Ok(T) | Err(E)
let x: Result<Int, String> = Ok(1)
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "unexpected E300 diagnostics: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_adt_constructor_call_generic_mismatch_emits_e300_in_hm() {
    // HM is now authoritative for typed-let annotation mismatches and emits
    // E300 directly; the bytecode compiler's boundary check is a fallback.
    let source = r#"
type Result<T, E> = Ok(T) | Err(E)
let x: Result<Int, String> = Ok("oops")
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "HM should emit E300 for let annotation mismatches, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_adt_constructor_pattern_binding_propagates_field_type() {
    let source = r#"
type Result<T, E> = Ok(T) | Err(E)
fn unwrap_plus(r: Result<Int, String>) -> Int {
    match r {
        Ok(v) -> v + "x",
        Err(_) -> 0,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_if_concrete_branch_mismatch_emits_contextual_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(
            &result,
            "The branches of this `if` expression do not agree on a type."
        ),
        "expected contextual if-branch mismatch message, got: {:#?}",
        result.diagnostics.iter().map(|d| d.message().unwrap_or("")).collect::<Vec<_>>()
    );

    let diag = result
        .diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The branches of this `if` expression do not agree on a type.")
                })
        })
        .expect("expected contextual if E300 diagnostic");
    assert!(
        diag.labels().iter().any(|l| l.style == flux::diagnostics::LabelStyle::Primary),
        "expected primary label on contextual if diagnostic"
    );
    assert!(
        diag.labels().iter().any(|l| l.style == flux::diagnostics::LabelStyle::Secondary),
        "expected secondary label on contextual if diagnostic"
    );
}

#[test]
fn infer_if_contextual_primary_label_uses_else_value_expression_span() {
    let source = r#"
fn main() -> Unit {
    let _x = if true { 42 } else { "nope" }
}
"#;
    let (result, program) = infer_program_from_source(source);
    let expected_else_value_span = match &program.statements[0] {
        Statement::Function { body, .. } => match &body.statements[0] {
            Statement::Let { value: Expression::If { alternative: Some(alt), .. }, .. } => {
                match alt.statements.last() {
                    Some(Statement::Expression { expression, has_semicolon: false, .. }) => {
                        expression.span()
                    }
                    other => panic!("unexpected else block tail statement shape: {other:?}"),
                }
            }
            other => panic!("unexpected function body statement shape: {other:?}"),
        },
        other => panic!("unexpected program statement shape: {other:?}"),
    };

    let diag = result
        .diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The branches of this `if` expression do not agree on a type.")
                })
        })
        .expect("expected contextual if E300 diagnostic");
    let primary = diag
        .labels()
        .iter()
        .find(|l| l.style == flux::diagnostics::LabelStyle::Primary)
        .expect("expected primary label on contextual if diagnostic");
    assert_eq!(
        primary.span, expected_else_value_span,
        "expected primary label span to match else value expression span"
    );
}

#[test]
fn infer_if_with_any_branch_does_not_emit_contextual_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = if true { 42 } else { mystery_value }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(
            &result,
            "The branches of this `if` expression do not agree on a type."
        ),
        "did not expect contextual if-branch mismatch diagnostic when one branch is unresolved, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_concrete_arm_mismatch_emits_contextual_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(
            &result,
            "The arms of this `match` expression do not agree on a type."
        ),
        "expected contextual match-arm mismatch message, got: {:#?}",
        result.diagnostics.iter().map(|d| d.message().unwrap_or("")).collect::<Vec<_>>()
    );

    let diag = result
        .diagnostics
        .iter()
        .find(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The arms of this `match` expression do not agree on a type.")
                })
        })
        .expect("expected contextual match E300 diagnostic");
    assert!(
        diag.labels().iter().any(|l| l.style == flux::diagnostics::LabelStyle::Primary),
        "expected primary label on contextual match diagnostic"
    );
    assert!(
        diag.labels().iter().any(|l| l.style == flux::diagnostics::LabelStyle::Secondary),
        "expected secondary label on contextual match diagnostic"
    );
}

#[test]
fn infer_match_multiple_mismatching_arms_emits_multiple_contextual_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> "no",
        _ -> false,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    let contextual: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| {
            d.code() == Some("E300")
                && d.message().is_some_and(|m| {
                    m.contains("The arms of this `match` expression do not agree on a type.")
                })
        })
        .collect();
    assert_eq!(
        contextual.len(),
        2,
        "expected 2 contextual match-arm E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    let msgs: Vec<_> = contextual
        .iter()
        .filter_map(|d| {
            d.labels().iter().find(|l| l.style == flux::diagnostics::LabelStyle::Primary)
        })
        .map(|l| l.text.clone())
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("arm 2")),
        "expected primary label mentioning arm 2, got: {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("arm 3")),
        "expected primary label mentioning arm 3, got: {:?}",
        msgs
    );
}

#[test]
fn infer_match_any_or_unresolved_arm_does_not_emit_contextual_e300() {
    let source = r#"
fn main() -> Unit {
    let _x = match true {
        true -> 1,
        false -> mystery_value,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(
            &result,
            "The arms of this `match` expression do not agree on a type."
        ),
        "did not expect contextual match-arm mismatch diagnostic when one arm is unresolved, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_array_literal_with_concrete_heterogeneous_elements_emits_e300() {
    let source = r#"
fn main() -> Unit {
    let _xs = [|1, "x"|]
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for concrete heterogeneous array literal, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_array_literal_with_equivalent_generic_map_elements_stays_concrete() {
    let source = r##"
import Flow.Map as Map

fn main() -> Unit {
    let _xs = [|Map.set({}, "x", 1), Map.set(Map.set({}, "y", 2), "z", 3)|]
}
"##;
    let (result, program) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E430"),
        "did not expect E430 for equivalent generic map elements, got: {:#?}",
        result.diagnostics
    );

    let let_value = program
        .statements
        .iter()
        .find_map(|statement| match statement {
            Statement::Function { body, .. } => {
                body.statements.iter().find_map(|statement| match statement {
                    Statement::Let { value, .. } => Some(value),
                    _ => None,
                })
            }
            _ => None,
        })
        .expect("expected let-bound array literal");
    let ty = result
        .expr_types
        .get(&let_value.expr_id())
        .expect("expected inferred type for array literal");
    match ty {
        InferType::App(TypeConstructor::Array, elems) if elems.len() == 1 => match &elems[0] {
            InferType::Var(var) => assert!(
                !result.fallback_vars.contains(var),
                "array element type should stay flexible, not fallback-poisoned: {:#?}",
                ty
            ),
            other => panic!("expected flexible array element type, got {other:#?}"),
        },
        other => panic!("expected Array<_> type for array literal, got {other:#?}"),
    }
}

#[test]
fn infer_tuple_destructure_from_concrete_non_tuple_emits_e300() {
    let source = r#"
fn main() -> Unit {
    let (a, b) = 1
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for tuple destructure from concrete non-tuple source, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_with_unresolved_first_arm_and_concrete_conflict_still_emits_e300() {
    let source = r#"
fn main() -> Unit {
    let x = mystery_value
    let _y = match 1 {
        0 -> x,
        1 -> 10,
        _ -> "oops",
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for concrete match-arm conflict even when first arm is unresolved, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(
            &result,
            "The arms of this `match` expression do not agree on a type."
        ),
        "expected contextual match-arm mismatch message, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_family_consistent_arms_propagate_scrutinee_constraint() {
    let source = r#"
fn main() -> Unit {
    let x = mystery_value
    let y: String = match x {
        Some(n) -> n,
        None -> 0,
    }
}
"#;
    let (result, program) = infer_program_from_source(source);
    let match_expr = first_match_expr(&program).expect("expected match expression");
    let ty = result
        .expr_types
        .get(&match_expr.expr_id())
        .expect("expected inferred type for match expression");
    assert_eq!(
        *ty,
        int(),
        "expected family-consistent match to resolve scrutinee-driven result type to Int, got: {ty:?}"
    );
}

#[test]
fn infer_match_wildcard_only_does_not_propagate_scrutinee_constraint() {
    let source = r#"
fn main() -> Unit {
    let x = mystery_value
    let _y = match x {
        _ -> mystery_value,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for wildcard-only non-constraining match, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_mixed_families_do_not_propagate_scrutinee_constraint() {
    let source = r#"
fn main() -> Unit {
    let x = mystery_value
    let _y = match x {
        Some(n) -> n,
        Left(l) -> l,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for mixed-family match propagation guard, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_mixed_adt_constructors_do_not_propagate_scrutinee_constraint() {
    let source = r#"
type A = A1(Int)
type B = B1(Int)

fn main() -> Unit {
    let x = mystery_value
    let _y = match x {
        A1(v) -> v,
        B1(w) -> w,
    }
}
    "#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected concrete mismatch diagnostics for mixed-ADT-family unresolved match, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_concrete_option_with_wildcard_preserves_scrutinee_type() {
    let source = r#"
data Contact {
    Contact { name: String }
}

fn main() -> Unit {
    let value: Option<Contact> = Some(Contact { name: "Alice" })
    let _name = match value {
        Some(c) -> c.name,
        None -> "missing",
        _ -> "missing",
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    // The only diagnostic allowed is the coverage checker's
    // "Redundant Match Arm" warning for the trailing `_ -> ...`
    // arm (covered by `Some(_)` + `None`). No errors should appear.
    let has_error =
        result.diagnostics.iter().any(|d| d.severity() == flux::diagnostics::Severity::Error);
    assert!(
        !has_error,
        "expected no error diagnostics for concrete Option match, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_unannotated_self_recursive_function_refines_return_type_on_second_pass() {
    let source = r#"
fn countdown(n) {
    if n == 0 {
        0
    } else {
        countdown(n - 1)
    }
}

fn main() -> Unit {
    let value: String = countdown(2)
}
"#;
    let (result, program) = infer_program_from_source(source);
    let call_expr = match &program.statements[1] {
        Statement::Function { body, .. } => match &body.statements[0] {
            Statement::Let { value: Expression::Call { .. }, .. } => {
                if let Statement::Let { value: call @ Expression::Call { .. }, .. } =
                    &body.statements[0]
                {
                    call
                } else {
                    unreachable!("shape checked above")
                }
            }
            other => panic!("unexpected main statement shape: {other:?}"),
        },
        other => panic!("unexpected program statement shape: {other:?}"),
    };
    let inferred_call_ty = result
        .expr_types
        .get(&call_expr.expr_id())
        .cloned()
        .expect("expected inferred type for recursive call in main");
    assert_eq!(
        inferred_call_ty,
        int(),
        "expected refined recursive call type to be Int, got: {inferred_call_ty:?}"
    );
    // Annotation `String` conflicts with refined `Int`; HM now reports E300.
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for annotation mismatch, got: {:#?}",
        result.diagnostics
    );
}

/// The refinement pass binds the return type, so the first pass's return
/// variable has to be bound with it. Otherwise everything the first pass typed
/// with it keeps a variable: `fib(n - 1) + fib(n - 2)` stayed `?r`, and its
/// `Add<?r>` was never solved although `fib` returns `Int`.
#[test]
fn self_recursive_refinement_resolves_what_the_first_pass_typed_with_the_return() {
    let source = r#"
fn fib(n) {
    if (n <= 1) {
        return n;
    }
    return fib(n - 1) + fib(n - 2);
}
"#;
    let (result, program) = infer_program_from_source(source);
    let Statement::Function { body, .. } = &program.statements[0] else {
        panic!("expected `fib`");
    };
    let Some(Statement::Return { value: Some(sum @ Expression::Infix { .. }), .. }) =
        body.statements.last()
    else {
        panic!("expected `return fib(n - 1) + fib(n - 2)`, got {:?}", body.statements.last());
    };
    assert_eq!(result.expr_types.get(&sum.expr_id()), Some(&int()), "the sum's type");
}

/// What follows a block's own `return` never runs, so it is not the block's
/// value: `return 42; print("x");` returns `Int`, not `Unit`.
#[test]
fn statements_after_a_return_are_not_the_blocks_value() {
    let source = r#"
fn answer() {
    return 42;
    print("unreachable");
}

fn main() -> Unit {
    let n: Int = answer()
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "expected no E300, got: {:#?}",
        result.diagnostics
    );
}

/// An early `return` is checked against the annotation like the final value.
#[test]
fn an_early_return_must_match_the_return_annotation() {
    let source = r#"
fn pick(flag: Bool) -> Int {
    if (flag) {
        return "no";
    }
    return 1;
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for `return \"no\"`, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_non_recursive_function_does_not_trigger_second_pass_behavior() {
    let source = r#"
fn id(x) { x }
fn main() -> Unit {
    let value: Int = id(1)
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        result.diagnostics.is_empty(),
        "expected no HM diagnostics for non-recursive baseline function, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_unannotated_self_recursive_concrete_chain_keeps_refined_int_type() {
    let source = r#"
fn sum_down(n) {
    if n == 0 {
        0
    } else {
        1 + sum_down(n - 1)
    }
}

fn main() -> Unit {
    let value: String = sum_down(2)
}
"#;
    let (result, program) = infer_program_from_source(source);
    let call_expr = match &program.statements[1] {
        Statement::Function { body, .. } => match &body.statements[0] {
            Statement::Let { value: Expression::Call { .. }, .. } => {
                if let Statement::Let { value: call @ Expression::Call { .. }, .. } =
                    &body.statements[0]
                {
                    call
                } else {
                    unreachable!("shape checked above")
                }
            }
            other => panic!("unexpected main statement shape: {other:?}"),
        },
        other => panic!("unexpected program statement shape: {other:?}"),
    };
    let inferred_call_ty = result
        .expr_types
        .get(&call_expr.expr_id())
        .cloned()
        .expect("expected inferred type for recursive call in main");
    assert_eq!(
        inferred_call_ty,
        int(),
        "expected refined recursive call type to stay Int, got: {inferred_call_ty:?}"
    );
    // Annotation `String` conflicts with refined `Int`; HM now reports E300.
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 for annotation mismatch, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_fun_param_mismatch_emits_param_specific_e300() {
    let source = r#"
fn takes_int(x: Int) -> Int { x }
fn takes_string(x: String) -> Int { 0 }
fn main() -> Unit {
    let _f = if true {
        takes_int
    } else {
        takes_string
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(&result, "Parameter 1 has the wrong type."),
        "expected function param mismatch message, got: {:#?}",
        result.diagnostics.iter().map(|d| d.message().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_fun_return_mismatch_emits_return_specific_e300() {
    let source = r#"
fn ret_int() -> Int { 1 }
fn ret_string() -> String { "x" }
fn main() -> Unit {
    let _f = if true {
        ret_int
    } else {
        ret_string
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(
            &result,
            "The body of this function does not match its return type."
        ),
        "expected function return mismatch message, got: {:#?}",
        result.diagnostics.iter().map(|d| d.message().unwrap_or("")).collect::<Vec<_>>()
    );
}

#[test]
fn infer_fun_arity_mismatch_emits_arity_specific_e300() {
    let source = r#"
fn one_arg(x: Int) -> Int { x }
fn two_args(x: Int, y: Int) -> Int { x + y }
fn main() -> Unit {
    let _f = if true {
        one_arg
    } else {
        two_args
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected E300 diagnostics, got: {:#?}",
        result.diagnostics
    );
    assert!(
        has_diagnostic_message_fragment(&result, "too many arguments"),
        "expected function arity mismatch message, got: {:#?}",
        result.diagnostics.iter().map(|d| d.message().unwrap_or("")).collect::<Vec<_>>()
    );
}
