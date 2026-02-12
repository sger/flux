use flux::{
    ast::constant_fold,
    syntax::{expression::Expression, lexer::Lexer, parser::Parser},
};

fn parse_and_fold(input: &str) -> Expression {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "Parse errors: {:?}", parser.errors);

    let folded = constant_fold(program);
    assert_eq!(folded.statements.len(), 1, "Expected single statement");

    match &folded.statements[0] {
        flux::syntax::statement::Statement::Expression { expression, .. } => expression.clone(),
        _ => panic!("Expected expression statement"),
    }
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
