/// Unit tests for the HM type inference engine (src/types/).
// ============================================================================
// Helper constructors
// ============================================================================
use std::collections::{HashMap, HashSet};

use flux::ast::type_infer::{InferProgramConfig, infer_program};
use flux::syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement};
use flux::types::infer_effect_row::InferEffectRow;
use flux::types::infer_type::InferType;
use flux::types::scheme::{Scheme, generalize};
use flux::types::type_constructor::TypeConstructor;
use flux::types::type_env::TypeEnv;
use flux::types::type_subst::TypeSubst;
use flux::types::unify::unify;
use flux::types::unify_error::UnifyErrorKind;

fn int() -> InferType {
    InferType::Con(TypeConstructor::Int)
}

fn float() -> InferType {
    InferType::Con(TypeConstructor::Float)
}

fn string() -> InferType {
    InferType::Con(TypeConstructor::String)
}

fn bool_() -> InferType {
    InferType::Con(TypeConstructor::Bool)
}

fn var(v: u32) -> InferType {
    InferType::Var(v)
}

fn list(t: InferType) -> InferType {
    InferType::App(TypeConstructor::List, vec![t])
}

fn option(t: InferType) -> InferType {
    InferType::App(TypeConstructor::Option, vec![t])
}

fn fun(params: Vec<InferType>, ret: InferType) -> InferType {
    InferType::Fun(params, Box::new(ret), InferEffectRow::closed_empty())
}

fn fun_with_effects(
    params: Vec<InferType>,
    ret: InferType,
    effects: Vec<flux::syntax::Identifier>,
) -> InferType {
    InferType::Fun(
        params,
        Box::new(ret),
        InferEffectRow::closed_from_symbols(effects),
    )
}

fn tuple(elems: Vec<InferType>) -> InferType {
    InferType::Tuple(elems)
}

fn infer_program_from_source(
    source: &str,
) -> (
    flux::ast::type_infer::InferProgramResult,
    flux::syntax::program::Program,
) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut effect_op_sigs = HashMap::new();
    fn collect_effect_sigs(
        statements: &[Statement],
        out: &mut HashMap<
            (flux::syntax::Identifier, flux::syntax::Identifier),
            flux::syntax::type_expr::TypeExpr,
        >,
    ) {
        for statement in statements {
            match statement {
                Statement::EffectDecl { name, ops, .. } => {
                    for op in ops {
                        out.insert((*name, op.name), op.type_expr.clone());
                    }
                }
                Statement::Module { body, .. } => collect_effect_sigs(&body.statements, out),
                _ => {}
            }
        }
    }
    collect_effect_sigs(&program.statements, &mut effect_op_sigs);
    let mut interner_for_base = interner.clone();
    let base_symbol = interner_for_base.intern("Flow");
    let result = infer_program(
        &program,
        &interner,
        InferProgramConfig {
            file_path: Some("<test>".into()),

            preloaded_base_schemes: HashMap::new(),
            preloaded_module_member_schemes: HashMap::new(),
            known_flow_names: HashSet::new(),
            flow_module_symbol: base_symbol,
            preloaded_effect_op_signatures: effect_op_sigs,
            class_env: None,
        },
    );
    (result, program)
}

fn first_perform_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Perform { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call {
                function,
                arguments,
                ..
            } => walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr)),
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => pairs
                .iter()
                .find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v))),
            Expression::Index { left, index, .. } => walk_expr(left).or_else(|| walk_expr(index)),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => walk_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => walk_expr(scrutinee).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .as_ref()
                        .and_then(walk_expr)
                        .or_else(|| walk_expr(&arm.body))
                })
            }),
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

fn first_handle_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Handle { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call {
                function,
                arguments,
                ..
            } => walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr)),
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => pairs
                .iter()
                .find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v))),
            Expression::Index { left, index, .. } => walk_expr(left).or_else(|| walk_expr(index)),
            Expression::MemberAccess { object, .. }
            | Expression::TupleFieldAccess { object, .. } => walk_expr(object),
            Expression::Match {
                scrutinee, arms, ..
            } => walk_expr(scrutinee).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.guard
                        .as_ref()
                        .and_then(walk_expr)
                        .or_else(|| walk_expr(&arm.body))
                })
            }),
            Expression::Some { value, .. }
            | Expression::Left { value, .. }
            | Expression::Right { value, .. } => walk_expr(value),
            Expression::Cons { head, tail, .. } => walk_expr(head).or_else(|| walk_expr(tail)),
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

fn first_match_expr(program: &flux::syntax::program::Program) -> Option<&Expression> {
    fn walk_expr(expr: &Expression) -> Option<&Expression> {
        match expr {
            Expression::Match { .. } => Some(expr),
            Expression::Prefix { right, .. } => walk_expr(right),
            Expression::Infix { left, right, .. } => walk_expr(left).or_else(|| walk_expr(right)),
            Expression::If {
                condition,
                consequence,
                alternative,
                ..
            } => walk_expr(condition)
                .or_else(|| walk_block(consequence))
                .or_else(|| alternative.as_ref().and_then(walk_block)),
            Expression::DoBlock { block, .. } => walk_block(block),
            Expression::Function { body, .. } => walk_block(body),
            Expression::Call {
                function,
                arguments,
                ..
            } => walk_expr(function).or_else(|| arguments.iter().find_map(walk_expr)),
            Expression::TupleLiteral { elements, .. }
            | Expression::ListLiteral { elements, .. }
            | Expression::ArrayLiteral { elements, .. } => elements.iter().find_map(walk_expr),
            Expression::Hash { pairs, .. } => pairs
                .iter()
                .find_map(|(k, v)| walk_expr(k).or_else(|| walk_expr(v))),
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

fn has_diagnostic_code(result: &flux::ast::type_infer::InferProgramResult, code: &str) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diag| diag.code() == Some(code))
}

fn has_diagnostic_message_fragment(
    result: &flux::ast::type_infer::InferProgramResult,
    fragment: &str,
) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diag| diag.message().is_some_and(|m| m.contains(fragment)))
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
        result
            .diagnostics
            .iter()
            .map(|d| d.message().unwrap_or(""))
            .collect::<Vec<_>>()
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
        diag.labels()
            .iter()
            .any(|l| l.style == flux::diagnostics::LabelStyle::Primary),
        "expected primary label on contextual if diagnostic"
    );
    assert!(
        diag.labels()
            .iter()
            .any(|l| l.style == flux::diagnostics::LabelStyle::Secondary),
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
            Statement::Let {
                value:
                    Expression::If {
                        alternative: Some(alt),
                        ..
                    },
                ..
            } => match alt.statements.last() {
                Some(Statement::Expression {
                    expression,
                    has_semicolon: false,
                    ..
                }) => expression.span(),
                other => panic!("unexpected else block tail statement shape: {other:?}"),
            },
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
        result
            .diagnostics
            .iter()
            .map(|d| d.message().unwrap_or(""))
            .collect::<Vec<_>>()
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
        diag.labels()
            .iter()
            .any(|l| l.style == flux::diagnostics::LabelStyle::Primary),
        "expected primary label on contextual match diagnostic"
    );
    assert!(
        diag.labels()
            .iter()
            .any(|l| l.style == flux::diagnostics::LabelStyle::Secondary),
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
            d.labels()
                .iter()
                .find(|l| l.style == flux::diagnostics::LabelStyle::Primary)
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
            Statement::Let {
                value:
                    Expression::Call {
                        function: _,
                        arguments: _,
                        ..
                    },
                ..
            } => {
                if let Statement::Let {
                    value: call @ Expression::Call { .. },
                    ..
                } = &body.statements[0]
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
            Statement::Let {
                value:
                    Expression::Call {
                        function: _,
                        arguments: _,
                        ..
                    },
                ..
            } => {
                if let Statement::Let {
                    value: call @ Expression::Call { .. },
                    ..
                } = &body.statements[0]
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
        result
            .diagnostics
            .iter()
            .map(|d| d.message().unwrap_or(""))
            .collect::<Vec<_>>()
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
        result
            .diagnostics
            .iter()
            .map(|d| d.message().unwrap_or(""))
            .collect::<Vec<_>>()
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
        result
            .diagnostics
            .iter()
            .map(|d| d.message().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

// ============================================================================
// Unification tests
// ============================================================================

#[test]
fn unify_same_con() {
    assert!(unify(&int(), &int()).is_ok());
    assert!(unify(&string(), &string()).is_ok());
    assert!(unify(&bool_(), &bool_()).is_ok());
}

#[test]
fn unify_con_mismatch() {
    let err = unify(&int(), &string()).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_var_to_con() {
    let subst = unify(&var(0), &int()).unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_con_to_var() {
    let subst = unify(&string(), &var(1)).unwrap();
    assert_eq!(subst.get(1), Some(&string()));
}

#[test]
fn unify_var_to_var_same() {
    // Var(0) = Var(0) → empty substitution
    let subst = unify(&var(0), &var(0)).unwrap();
    assert!(subst.is_empty());
}

#[test]
fn unify_var_to_var_different() {
    // Var(0) = Var(1) → bind one to the other
    let subst = unify(&var(0), &var(1)).unwrap();
    assert!(subst.get(0).is_some() || subst.get(1).is_some());
}

#[test]
fn unify_list_int_list_int() {
    assert!(unify(&list(int()), &list(int())).is_ok());
}

#[test]
fn unify_list_var_list_int() {
    let subst = unify(&list(var(0)), &list(int())).unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_list_mismatch() {
    let err = unify(&list(int()), &list(string())).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_option_var() {
    let subst = unify(&option(var(0)), &option(float())).unwrap();
    assert_eq!(subst.get(0), Some(&float()));
}

#[test]
fn unify_fun_types() {
    // (Int -> String) = (Int -> String)
    assert!(unify(&fun(vec![int()], string()), &fun(vec![int()], string())).is_ok());
}

#[test]
fn unify_fun_with_var_param() {
    // (Var(0) -> Int) = (String -> Int) → {0 → String}
    let subst = unify(&fun(vec![var(0)], int()), &fun(vec![string()], int())).unwrap();
    assert_eq!(subst.get(0), Some(&string()));
}

#[test]
fn unify_fun_with_var_return() {
    // (Int -> Var(0)) = (Int -> Bool) → {0 → Bool}
    let subst = unify(&fun(vec![int()], var(0)), &fun(vec![int()], bool_())).unwrap();
    assert_eq!(subst.get(0), Some(&bool_()));
}

#[test]
fn unify_fun_arity_mismatch() {
    let err = unify(
        &fun(vec![int()], string()),
        &fun(vec![int(), int()], string()),
    )
    .unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_fun_effect_match_succeeds() {
    let mut interner = flux::syntax::interner::Interner::new();
    let io = interner.intern("IO");
    let time = interner.intern("Time");
    let left = fun_with_effects(vec![int()], int(), vec![io, time]);
    let right = fun_with_effects(vec![int()], int(), vec![time, io]);
    assert!(unify(&left, &right).is_ok());
}

#[test]
fn unify_fun_effect_mismatch_fails() {
    let mut interner = flux::syntax::interner::Interner::new();
    let io = interner.intern("IO");
    let time = interner.intern("Time");
    let left = fun_with_effects(vec![int()], int(), vec![io]);
    let right = fun_with_effects(vec![int()], int(), vec![time]);
    let err = unify(&left, &right).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn infer_perform_returns_declared_op_return_type() {
    let (result, program) = infer_program_from_source(
        r#"
effect Console {
    read: String -> Int
}
fn main() -> Unit with Console {
    let x = perform Console.read("name")
}
"#,
    );
    let perform = first_perform_expr(&program).expect("expected perform expression");
    let ty = result
        .expr_types
        .get(&perform.expr_id())
        .expect("expected inferred type for perform expression");
    assert_eq!(*ty, int());
}

#[test]
fn infer_perform_arg_type_mismatch_reports_e300() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Unit
}
fn main() -> Unit with Console {
    perform Console.print(1)
}
"#,
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code().is_some_and(|code| code == "E300")),
        "expected E300 diagnostics for perform arg mismatch, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.code().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_order_equivalence_for_function_params() {
    let (result, _program) = infer_program_from_source(
        r#"
fn call_swapped_effects(f: ((Int) -> Int with Time, IO), x: Int) -> Int with IO, Time {
    f(x)
}

fn add_one(x: Int) -> Int with IO, Time {
    x + 1
}

fn main() -> Unit {
    let _ = call_swapped_effects(add_one, 1)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no diagnostics for order-equivalent effect rows, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.code().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_subtract_var_signature_stays_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> ()
}
fn run_filtered(f: (() -> Int with |e - Console)) -> Int with |e - Console {
    f()
}
fn io_work() -> Int with IO {
    print("work")
    10
}
fn main() -> Unit {
    let _ = run_filtered(io_work)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected no HM diagnostics for satisfiable row-variable subtraction signature, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.code().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_subset_callback_requirement_emits_e300() {
    let (result, _program) = infer_program_from_source(
        r#"
fn run_needs_io_time(f: (() -> Int with IO, Time)) -> Int with IO, Time {
    f()
}
fn only_io() -> Int with IO {
    1
}
fn main() -> Unit {
    let _ = run_needs_io_time(only_io)
}
"#,
    );
    assert!(
        has_diagnostic_code(&result, "E300"),
        "expected HM effect-row mismatch diagnostics, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.code().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn infer_effect_row_absent_ordering_linked_vars_stays_hm_clean() {
    let (result, _program) = infer_program_from_source(
        r#"
fn needs_linked_absence(
    first: (() -> Int with |e - IO),
    second: (() -> Int with |e)
) -> Int with |e - IO {
    first() + second()
}
fn row_var_worker() -> Int with |e {
    1
}
fn time_worker() -> Int with Time {
    now_ms()
    2
}
fn main() -> Unit with Time {
    let _ = needs_linked_absence(row_var_worker, time_worker)
}
"#,
    );
    assert!(
        result.diagnostics.is_empty(),
        "expected HM to remain diagnostics-clean for linked row-var absence flow; compiler enforces call-site row constraints, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.code().unwrap_or(""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn infer_handle_result_matches_handled_expression_type() {
    let (result, program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Int
}
fn main() -> Unit with Console {
    let x = (perform Console.print("x")) handle Console {
        print(resume, _msg) -> 1
    }
}
"#,
    );
    let handle = first_handle_expr(&program).expect("expected handle expression");
    let ty = result
        .expr_types
        .get(&handle.expr_id())
        .expect("expected inferred type for handle expression");
    assert_eq!(*ty, int());
}

#[test]
fn infer_handle_arms_bind_declared_param_types() {
    let (result, _program) = infer_program_from_source(
        r#"
effect Console {
    print: String -> Int
}
fn main() -> Unit with Console {
    let _ = (perform Console.print("x")) handle Console {
        print(resume, msg) -> msg + 1
    }
}
"#,
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code().is_some_and(|code| code == "E300")),
        "expected E300 diagnostics for handler param type usage mismatch"
    );
}

#[test]
fn unify_tuple_match() {
    let subst = unify(
        &tuple(vec![var(0), string()]),
        &tuple(vec![int(), string()]),
    )
    .unwrap();
    assert_eq!(subst.get(0), Some(&int()));
}

#[test]
fn unify_tuple_length_mismatch() {
    let err = unify(&tuple(vec![int(), string()]), &tuple(vec![int()])).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::Mismatch);
}

#[test]
fn unify_var_with_concrete_and_composite_types() {
    assert!(unify(&var(42), &int()).is_ok());
    assert!(unify(&int(), &var(42)).is_ok());
    assert!(unify(&var(7), &list(string())).is_ok());
}

#[test]
fn unify_occurs_check() {
    // Var(0) = List<Var(0)> → infinite type
    let err = unify(&var(0), &list(var(0))).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
}

#[test]
fn unify_occurs_check_nested() {
    // Var(0) = Option<Var(0)> → infinite type
    let err = unify(&var(0), &option(var(0))).unwrap_err();
    assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
}

// ============================================================================
// Substitution tests
// ============================================================================

#[test]
fn subst_apply_concrete() {
    let subst = TypeSubst::empty();
    assert_eq!(int().apply_type_subst(&subst), int());
}

#[test]
fn subst_apply_var() {
    let mut subst = TypeSubst::empty();
    subst.insert(0, int());
    assert_eq!(var(0).apply_type_subst(&subst), int());
    assert_eq!(var(1).apply_type_subst(&subst), var(1)); // unbound
}

#[test]
fn subst_apply_nested() {
    let mut subst = TypeSubst::empty();
    subst.insert(0, string());
    assert_eq!(list(var(0)).apply_type_subst(&subst), list(string()));
    assert_eq!(
        fun(vec![var(0)], var(0)).apply_type_subst(&subst),
        fun(vec![string()], string())
    );
}

#[test]
fn subst_compose_sequential() {
    // s1 = {0 → Int}, s2 = {1 → Var(0)}
    // s1 ∘ s2 applied to Var(1) should give Int
    let mut s1 = TypeSubst::empty();
    s1.insert(0, int());
    let mut s2 = TypeSubst::empty();
    s2.insert(1, var(0));
    let composed = s1.compose(&s2);
    let result = var(1).apply_type_subst(&composed);
    assert_eq!(result, int());
}

// ============================================================================
// Scheme tests
// ============================================================================

#[test]
fn scheme_mono_no_forall() {
    let s = Scheme::mono(int());
    assert!(s.forall.is_empty());
    assert_eq!(s.infer_type, int());
}

#[test]
fn scheme_instantiate_mono() {
    let s = Scheme::mono(int());
    let mut counter = 0u32;
    let (ty, mapping, _) = s.instantiate(&mut counter);
    assert_eq!(ty, int());
    assert!(mapping.is_empty());
    assert_eq!(counter, 0); // no fresh vars allocated
}

#[test]
fn scheme_instantiate_poly() {
    // ∀0. 0 → 0  (identity function scheme)
    let s = Scheme {
        forall: vec![0],
        constraints: vec![],
        infer_type: fun(vec![var(0)], var(0)),
    };
    let mut counter = 10u32;
    let (ty, mapping, _) = s.instantiate(&mut counter);
    // Fresh var 10 should replace var 0
    assert_eq!(counter, 11);
    assert_eq!(*mapping.get(&0).unwrap(), 10u32);
    assert_eq!(ty, fun(vec![var(10)], var(10)));
}

#[test]
fn scheme_instantiate_two_vars() {
    // ∀0 1. (0, 1) → 0  (const scheme)
    let s = Scheme {
        forall: vec![0, 1],
        constraints: vec![],
        infer_type: fun(vec![var(0), var(1)], var(0)),
    };
    let mut counter = 5u32;
    let (ty, mapping, _) = s.instantiate(&mut counter);
    assert_eq!(counter, 7); // allocated vars 5 and 6
    let v0 = *mapping.get(&0).unwrap();
    let v1 = *mapping.get(&1).unwrap();
    assert_eq!(ty, fun(vec![var(v0), var(v1)], var(v0)));
}

// ============================================================================
// Generalize tests
// ============================================================================

#[test]
fn generalize_no_free_vars() {
    use std::collections::HashSet;
    // int() has no free vars → scheme has no forall
    let scheme = generalize(&int(), &HashSet::new());
    assert!(scheme.forall.is_empty());
}

#[test]
fn generalize_free_var_not_in_env() {
    use std::collections::HashSet;
    // Var(0) not in env → gets generalized
    let scheme = generalize(&var(0), &HashSet::new());
    assert!(scheme.forall.contains(&0));
    assert_eq!(scheme.infer_type, var(0));
}

#[test]
fn generalize_free_var_in_env() {
    use std::collections::HashSet;
    // Var(0) IS in the env's free vars → not generalized (would be escaping)
    let env_free = HashSet::from([0u32]);
    let scheme = generalize(&var(0), &env_free);
    assert!(scheme.forall.is_empty()); // NOT quantified
}

#[test]
fn generalize_fun_partial() {
    use std::collections::HashSet;
    // (Var(0) -> Var(1)) where Var(0) is in env (fixed) but Var(1) is free
    let env_free = HashSet::from([0u32]);
    let scheme = generalize(&fun(vec![var(0)], var(1)), &env_free);
    assert!(!scheme.forall.contains(&0)); // env var, not quantified
    assert!(scheme.forall.contains(&1)); // free var, quantified
}

// ============================================================================
// TypeEnv bridge tests
// ============================================================================

#[test]
fn type_env_fresh() {
    let mut env = TypeEnv::new();
    let v0 = env.alloc_type_var_id();
    let v1 = env.alloc_type_var_id();
    assert_ne!(v0, v1);
    assert_eq!(v0 + 1, v1);
}

#[test]
fn type_env_bind_lookup() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    // We need a Symbol. Since Symbol::new is crate-private, use the interner.
    let mut interner = Interner::new();
    let x = interner.intern("x");
    env.bind(x, Scheme::mono(int()));
    assert!(env.lookup(x).is_some());
    assert_eq!(env.lookup(x).unwrap().infer_type, int());
}

#[test]
fn type_env_scope() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    let mut interner = Interner::new();
    let x = interner.intern("x");
    env.bind(x, Scheme::mono(int()));
    env.enter_scope();
    env.bind(x, Scheme::mono(string())); // shadow in inner scope
    assert_eq!(env.lookup(x).unwrap().infer_type, string());
    env.leave_scope();
    assert_eq!(env.lookup(x).unwrap().infer_type, int()); // outer restored
}

#[test]
fn type_env_free_vars() {
    use flux::syntax::interner::Interner;
    let mut env = TypeEnv::new();
    let mut interner = Interner::new();
    let x = interner.intern("x");
    // Monomorphic type with a free var
    env.bind(x, Scheme::mono(var(42)));
    let fvs = env.free_vars();
    assert!(fvs.contains(&42));
}

#[test]
fn type_env_try_to_runtime_primitives() {
    use flux::runtime::runtime_type::RuntimeType;
    assert_eq!(
        TypeEnv::try_to_runtime(&int(), &TypeSubst::empty()).unwrap(),
        RuntimeType::Int
    );
    assert_eq!(
        TypeEnv::try_to_runtime(&float(), &TypeSubst::empty()).unwrap(),
        RuntimeType::Float
    );
    assert_eq!(
        TypeEnv::try_to_runtime(&string(), &TypeSubst::empty()).unwrap(),
        RuntimeType::String
    );
    assert_eq!(
        TypeEnv::try_to_runtime(&bool_(), &TypeSubst::empty()).unwrap(),
        RuntimeType::Bool
    );
    assert_eq!(
        TypeEnv::try_to_runtime(&var(0), &TypeSubst::empty())
            .expect_err("unresolved vars should not lower as a concrete runtime type")
            .issue(),
        &flux::types::type_env::RuntimeTypeLoweringIssue::UnresolvedTypeVariable
    );
}

#[test]
fn type_env_try_to_runtime_option() {
    use flux::runtime::runtime_type::RuntimeType;
    let rt = TypeEnv::try_to_runtime(&option(int()), &TypeSubst::empty()).unwrap();
    assert_eq!(rt, RuntimeType::Option(Box::new(RuntimeType::Int)));
}

#[test]
fn type_env_try_to_runtime_resolves_var() {
    use flux::runtime::runtime_type::RuntimeType;
    let mut subst = TypeSubst::empty();
    subst.insert(0, int());
    // var(0) with substitution {0 → Int} → Int
    let rt = TypeEnv::try_to_runtime(&var(0), &subst).unwrap();
    assert_eq!(rt, RuntimeType::Int);
}

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
            Statement::Let {
                value: Expression::Call { arguments, .. },
                ..
            } => arguments[1].span(),
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
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
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
            known_flow_names,
            flow_module_symbol: base,
            preloaded_effect_op_signatures: HashMap::new(),
            class_env: None,
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
