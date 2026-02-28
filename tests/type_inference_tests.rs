/// Unit tests for the HM type inference engine (src/types/).
// ============================================================================
// Helper constructors
// ============================================================================
use std::collections::HashMap;

use flux::ast::type_infer::infer_program;
use flux::syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement};
use flux::types::infer_type::InferType;
use flux::types::scheme::{Scheme, generalize};
use flux::types::type_constructor::TypeConstructor;
use flux::types::type_env::TypeEnv;
use flux::types::type_subst::TypeSubst;
use flux::types::unify_error::{UnifyErrorKind, unify};

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

fn any() -> InferType {
    InferType::Con(TypeConstructor::Any)
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
    InferType::Fun(params, Box::new(ret), vec![])
}

fn fun_with_effects(
    params: Vec<InferType>,
    ret: InferType,
    effects: Vec<flux::syntax::Identifier>,
) -> InferType {
    InferType::Fun(params, Box::new(ret), effects)
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
    let result = infer_program(
        &program,
        &interner,
        Some("<test>".to_string()),
        HashMap::new(),
        effect_op_sigs,
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
fn infer_adt_constructor_call_generic_mismatch_silent_in_hm() {
    // HM uses `unify_propagate` (silent) for let annotation mismatches —
    // the compiler's boundary checker is the authoritative reporter.
    // Verify HM does NOT emit E300 for this constraint.
    let source = r#"
type Result<T, E> = Ok(T) | Err(E)
let x: Result<Int, String> = Ok("oops")
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_code(&result, "E300"),
        "HM should not emit E300 for let annotation mismatches (compiler handles it), got: {:#?}",
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
            "The branches of this `if` expression produce different types."
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
                    m.contains("The branches of this `if` expression produce different types.")
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
            "The branches of this `if` expression produce different types."
        ),
        "did not expect contextual if-branch mismatch diagnostic when one branch is Any, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_if_with_nested_any_branch_type_does_not_emit_contextual_e300() {
    let source = r#"
fn concrete_fn(x: Int) -> Int { x }
fn any_param_fn(x: Any) -> Int { 0 }
fn main() -> Unit {
    let _f = if true { concrete_fn } else { any_param_fn }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(
            &result,
            "The branches of this `if` expression produce different types."
        ),
        "did not expect contextual if-branch mismatch diagnostic when one branch contains nested Any, got: {:#?}",
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
            "The arms of this `match` expression produce different types."
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
                    m.contains("The arms of this `match` expression produce different types.")
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
                    m.contains("The arms of this `match` expression produce different types.")
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
            "The arms of this `match` expression produce different types."
        ),
        "did not expect contextual match-arm mismatch diagnostic when one arm is Any, got: {:#?}",
        result.diagnostics
    );
}

#[test]
fn infer_match_with_nested_any_arm_type_does_not_emit_contextual_e300() {
    let source = r#"
fn concrete_fn(x: Int) -> Int { x }
fn any_param_fn(x: Any) -> Int { 0 }
fn main() -> Unit {
    let _f = match true {
        true -> concrete_fn,
        false -> any_param_fn,
    }
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(
            &result,
            "The arms of this `match` expression produce different types."
        ),
        "did not expect contextual match-arm mismatch diagnostic when one arm contains nested Any, got: {:#?}",
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
        has_diagnostic_message_fragment(
            &result,
            "Function parameter 1 type does not match: expected `Int`, found `String`."
        ),
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
            "Function return types do not match: expected `Int`, found `String`."
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
        has_diagnostic_message_fragment(&result, "Function arity does not match."),
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
    let key = perform as *const Expression as usize;
    let node_id = result
        .expr_ptr_to_id
        .get(&key)
        .expect("expected expr node id for perform");
    let ty = result
        .expr_types
        .get(node_id)
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
    let key = handle as *const Expression as usize;
    let node_id = result
        .expr_ptr_to_id
        .get(&key)
        .expect("expected expr node id for handle");
    let ty = result
        .expr_types
        .get(node_id)
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
fn unify_any_with_int() {
    // Any is compatible with everything (gradual typing)
    assert!(unify(&any(), &int()).is_ok());
    assert!(unify(&int(), &any()).is_ok());
    assert!(unify(&any(), &list(string())).is_ok());
    assert!(unify(&any(), &var(42)).is_ok());
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
    let (ty, mapping) = s.instantiate(&mut counter);
    assert_eq!(ty, int());
    assert!(mapping.is_empty());
    assert_eq!(counter, 0); // no fresh vars allocated
}

#[test]
fn scheme_instantiate_poly() {
    // ∀0. 0 → 0  (identity function scheme)
    let s = Scheme {
        forall: vec![0],
        infer_type: fun(vec![var(0)], var(0)),
    };
    let mut counter = 10u32;
    let (ty, mapping) = s.instantiate(&mut counter);
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
        infer_type: fun(vec![var(0), var(1)], var(0)),
    };
    let mut counter = 5u32;
    let (ty, mapping) = s.instantiate(&mut counter);
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
    let v0 = env.fresh();
    let v1 = env.fresh();
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
fn type_env_to_runtime_primitives() {
    use flux::runtime::runtime_type::RuntimeType;
    assert_eq!(
        TypeEnv::to_runtime(&int(), &TypeSubst::empty()),
        RuntimeType::Int
    );
    assert_eq!(
        TypeEnv::to_runtime(&float(), &TypeSubst::empty()),
        RuntimeType::Float
    );
    assert_eq!(
        TypeEnv::to_runtime(&string(), &TypeSubst::empty()),
        RuntimeType::String
    );
    assert_eq!(
        TypeEnv::to_runtime(&bool_(), &TypeSubst::empty()),
        RuntimeType::Bool
    );
    assert_eq!(
        TypeEnv::to_runtime(&any(), &TypeSubst::empty()),
        RuntimeType::Any
    );
    // Unresolved var → Any
    assert_eq!(
        TypeEnv::to_runtime(&var(0), &TypeSubst::empty()),
        RuntimeType::Any
    );
}

#[test]
fn type_env_to_runtime_option() {
    use flux::runtime::runtime_type::RuntimeType;
    let rt = TypeEnv::to_runtime(&option(int()), &TypeSubst::empty());
    assert_eq!(rt, RuntimeType::Option(Box::new(RuntimeType::Int)));
}

#[test]
fn type_env_to_runtime_resolves_var() {
    use flux::runtime::runtime_type::RuntimeType;
    let mut subst = TypeSubst::empty();
    subst.insert(0, int());
    // var(0) with substitution {0 → Int} → Int
    let rt = TypeEnv::to_runtime(&var(0), &subst);
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
        has_diagnostic_message_fragment(&result, "The 1st argument to `greet` has the wrong type."),
        "expected named call-arg contextual message, got: {:#?}",
        result.diagnostics
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
        has_diagnostic_message_fragment(&result, "The 1st argument to this function has the wrong type."),
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
fn infer_call_with_nested_any_param_does_not_emit_contextual_callarg_e300() {
    let source = r#"
fn accepts_any_param_fn(f: (Any) -> Int) -> Int { f(0) }
fn concrete_fn(x: Int) -> Int { x }
fn main() -> Unit {
    let _x = accepts_any_param_fn(concrete_fn)
}
"#;
    let (result, _) = infer_program_from_source(source);
    assert!(
        !has_diagnostic_message_fragment(&result, "argument to `accepts_any_param_fn` has the wrong type."),
        "did not expect contextual call-arg mismatch when expected type contains Any, got: {:#?}",
        result.diagnostics
    );
}
