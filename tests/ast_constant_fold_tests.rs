use flux::{
    ast::constant_fold,
    syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement},
};

fn parse_and_fold(input: &str) -> Expression {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parse errors: {:?}",
        parser.errors
    );

    let folded = constant_fold(program);
    assert_eq!(folded.statements.len(), 1, "Expected single statement");

    match &folded.statements[0] {
        flux::syntax::statement::Statement::Expression { expression, .. } => expression.clone(),
        _ => panic!("Expected expression statement"),
    }
}

fn parse_and_fold_statement(input: &str) -> Statement {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parse errors: {:?}",
        parser.errors
    );

    let folded = constant_fold(program);
    assert_eq!(folded.statements.len(), 1, "Expected single statement");
    folded.statements[0].clone()
}

#[test]
fn folds_integer_addition() {
    let result = parse_and_fold("2 + 3;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 5),
        _ => panic!("Expected Integer(5), got {:?}", result),
    }
}

#[test]
fn folds_integer_subtraction() {
    let result = parse_and_fold("10 - 4;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 6),
        _ => panic!("Expected Integer(6), got {:?}", result),
    }
}

#[test]
fn folds_integer_multiplication() {
    let result = parse_and_fold("6 * 7;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 42),
        _ => panic!("Expected Integer(42), got {:?}", result),
    }
}

#[test]
fn folds_integer_division() {
    let result = parse_and_fold("20 / 4;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 5),
        _ => panic!("Expected Integer(5), got {:?}", result),
    }
}

#[test]
fn folds_integer_modulo() {
    let result = parse_and_fold("17 % 5;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 2),
        _ => panic!("Expected Integer(2), got {:?}", result),
    }
}

#[test]
fn skips_division_by_zero() {
    let result = parse_and_fold("5 / 0;");
    // Should NOT fold division by zero
    match result {
        Expression::Infix { .. } => {} // Still an infix expression
        _ => panic!("Expected Infix (not folded), got {:?}", result),
    }
}

#[test]
fn skip_modulo_by_zero() {
    let result = parse_and_fold("10 % 0");

    match result {
        Expression::Infix { .. } => {} // Still an infix expression
        _ => panic!("Expected Infix (not folded), got {:?}", result),
    }
}

#[test]
fn fold_nested_integer_arithmetic() {
    // (3 * 4) + 1 → 12 + 1 → 13 (bottom-up)
    match parse_and_fold("3 * 4 + 1;") {
        Expression::Integer { value, .. } => assert_eq!(value, 13),
        other => panic!("expected Integer, got {:?}", other),
    }
}

#[test]
fn folds_float_arithmetic() {
    let result = parse_and_fold("2.5 + 3.5;");
    match result {
        Expression::Float { value, .. } => assert!((value - 6.0).abs() < 0.0001),
        _ => panic!("Expected Float(6.0), got {:?}", result),
    }
}

#[test]
fn folds_string_concatenation() {
    let result = parse_and_fold("\"hello\" + \" world\";");
    match result {
        Expression::String { value, .. } => assert_eq!(value, "hello world"),
        _ => panic!("Expected String(\"hello world\"), got {:?}", result),
    }
}

#[test]
fn folds_boolean_and() {
    let result = parse_and_fold("true && false;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, false),
        _ => panic!("Expected Boolean(false), got {:?}", result),
    }
}

#[test]
fn folds_boolean_or() {
    let result = parse_and_fold("true || false;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, true),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn folds_integer_comparison_equal() {
    let result = parse_and_fold("5 == 5;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, true),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn folds_integer_comparison_not_equal() {
    let result = parse_and_fold("5 != 3;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, true),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn folds_integer_comparison_less_than() {
    let result = parse_and_fold("3 < 5;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, true),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn folds_prefix_negation_integer() {
    let result = parse_and_fold("-42;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, -42),
        _ => panic!("Expected Integer(-42), got {:?}", result),
    }
}

#[test]
fn folds_prefix_negation_float() {
    let result = parse_and_fold("-3.14;");
    match result {
        Expression::Float { value, .. } => assert!((value + 3.14).abs() < 0.0001),
        _ => panic!("Expected Float(-3.14), got {:?}", result),
    }
}

#[test]
fn folds_prefix_not_boolean() {
    let result = parse_and_fold("!true;");
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, false),
        _ => panic!("Expected Boolean(false), got {:?}", result),
    }
}

#[test]
fn folds_nested_arithmetic() {
    let result = parse_and_fold("(2 + 3) * 4;");
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 20),
        _ => panic!("Expected Integer(20), got {:?}", result),
    }
}

#[test]
fn does_not_fold_variables() {
    let result = parse_and_fold("x + 5;");
    // Should NOT fold because x is a variable
    match result {
        Expression::Infix { .. } => {} // Still an infix expression
        _ => panic!("Expected Infix (not folded), got {:?}", result),
    }
}

#[test]
fn folds_complex_expression() {
    let result = parse_and_fold("(10 + 5) * 2 - 30 / 3;");
    // (10 + 5) = 15
    // 15 * 2 = 30
    // 30 / 3 = 10
    // 30 - 10 = 20
    match result {
        Expression::Integer { value, .. } => assert_eq!(value, 20),
        _ => panic!("Expected Integer(20), got {:?}", result),
    }
}

#[test]
fn fold_boolean_and() {
    match parse_and_fold("true && false;") {
        Expression::Boolean { value, .. } => assert!(!value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn fold_boolean_or() {
    match parse_and_fold("false || true;") {
        Expression::Boolean { value, .. } => assert!(value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn fold_integer_less_than() {
    match parse_and_fold("1 < 2;") {
        Expression::Boolean { value, .. } => assert!(value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn fold_integer_equality() {
    match parse_and_fold("3 == 3;") {
        Expression::Boolean { value, .. } => assert!(value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn fold_integer_inequality() {
    match parse_and_fold("3 != 4;") {
        Expression::Boolean { value, .. } => assert!(value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn fold_negate_integer() {
    match parse_and_fold("-42;") {
        Expression::Integer { value, .. } => assert_eq!(value, -42),
        other => panic!("expected Integer, got {:?}", other),
    }
}

#[test]
fn fold_negate_float() {
    match parse_and_fold("-2.5;") {
        Expression::Float { value, .. } => assert!((value - (-2.5)).abs() < f64::EPSILON),
        other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn fold_not_boolean() {
    match parse_and_fold("!true;") {
        Expression::Boolean { value, .. } => assert!(!value),
        other => panic!("expected Boolean, got {:?}", other),
    }
}

#[test]
fn preserves_identifier_plus_literal() {
    // x + 1 cannot be folded
    match parse_and_fold("x + 1;") {
        Expression::Infix { .. } => {}
        other => panic!("expected Infix (preserved), got {:?}", other),
    }
}

#[test]
fn fold_inside_let_binding() {
    let result = parse_and_fold_statement("let x = 1 + 2;");

    match result {
        Statement::Let { value, .. } => match value {
            Expression::Integer { value, .. } => assert_eq!(value, 3),
            other => panic!("expected Integer, got {:?}", other),
        },
        other => panic!("expected Let, got {:?}", other),
    }
}
