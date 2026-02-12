use flux::{
    ast::desugar,
    syntax::{expression::Expression, lexer::Lexer, parser::Parser},
};

fn parse_and_desugar(input: &str) -> Expression {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "Parse errors: {:?}", parser.errors);

    let desugared = desugar(program);
    assert_eq!(desugared.statements.len(), 1, "Expected single statement");

    match &desugared.statements[0] {
        flux::syntax::statement::Statement::Expression { expression, .. } => expression.clone(),
        _ => panic!("Expected expression statement"),
    }
}

#[test]
fn desugars_double_negation() {
    let result = parse_and_desugar("!!true;");

    // Should desugar to just: true
    match result {
        Expression::Boolean { value, .. } => assert_eq!(value, true),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn desugars_triple_negation() {
    let result = parse_and_desugar("!!!false;");

    // !!!false → !false (after removing double negation)
    match result {
        Expression::Prefix { operator, right, .. } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Boolean { value, .. } => assert_eq!(*value, false),
                _ => panic!("Expected !false"),
            }
        }
        _ => panic!("Expected Prefix(!), got {:?}", result),
    }
}

#[test]
fn desugars_negated_equality() {
    let result = parse_and_desugar("!(5 == 3);");

    // Should desugar to: 5 != 3
    match result {
        Expression::Infix { left, operator, right, .. } => {
            assert_eq!(operator, "!=");
            match (left.as_ref(), right.as_ref()) {
                (Expression::Integer { value: a, .. }, Expression::Integer { value: b, .. }) => {
                    assert_eq!(*a, 5);
                    assert_eq!(*b, 3);
                }
                _ => panic!("Expected integer operands"),
            }
        }
        _ => panic!("Expected Infix(!=), got {:?}", result),
    }
}

#[test]
fn desugars_negated_inequality() {
    let result = parse_and_desugar("!(x != y);");

    // Should desugar to: x == y
    match result {
        Expression::Infix { left, operator, right, .. } => {
            assert_eq!(operator, "==");
            match (left.as_ref(), right.as_ref()) {
                (Expression::Identifier { .. }, Expression::Identifier { .. }) => {
                    // Just verify it's an infix with == operator
                    // We can't easily check symbol values without interner
                    assert_eq!(operator, "==");
                }
                _ => panic!("Expected identifier operands"),
            }
        }
        _ => panic!("Expected Infix(==), got {:?}", result),
    }
}

#[test]
fn does_not_desugar_single_negation() {
    let result = parse_and_desugar("!true;");

    // Should remain as-is: !true
    match result {
        Expression::Prefix { operator, right, .. } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Boolean { value, .. } => assert_eq!(*value, true),
                _ => panic!("Expected Boolean"),
            }
        }
        _ => panic!("Expected Prefix(!), got {:?}", result),
    }
}

#[test]
fn does_not_desugar_other_negated_comparisons() {
    let result = parse_and_desugar("!(x < y);");

    // Should remain as-is: !(x < y)
    // We don't desugar negated <, >, <=, >= comparisons
    match result {
        Expression::Prefix { operator, right, .. } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Infix { operator, .. } => {
                    assert_eq!(operator, "<");
                }
                _ => panic!("Expected Infix(<)"),
            }
        }
        _ => panic!("Expected Prefix(!), got {:?}", result),
    }
}

#[test]
fn desugars_nested_in_complex_expression() {
    // Just verify that desugaring works inside a larger expression
    // The exact AST structure depends on precedence, so we just check compilation succeeds
    let lexer = Lexer::new("1 + (!(5 == 3)) + 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty());

    // Should desugar without errors
    let desugared = desugar(program);
    assert_eq!(desugared.statements.len(), 1);

    // The program should still be valid after desugaring
    match &desugared.statements[0] {
        flux::syntax::statement::Statement::Expression { .. } => {
            // Success - we have an expression
        }
        _ => panic!("Expected expression statement"),
    }
}

#[test]
fn preserves_variables() {
    let result = parse_and_desugar("!!x;");

    // !!x should desugar to x
    match result {
        Expression::Identifier { .. } => {
            // Success - it's an identifier
        }
        _ => panic!("Expected Identifier, got {:?}", result),
    }
}

#[test]
fn works_with_function_calls() {
    let result = parse_and_desugar("!!(foo());");

    // !!(foo()) should desugar to foo()
    match result {
        Expression::Call { .. } => {
            // Success - it's a call expression
        }
        _ => panic!("Expected Call, got {:?}", result),
    }
}
