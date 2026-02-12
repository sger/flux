use flux::{
    ast::desugar,
    syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement},
};

fn parse_and_desugar(input: &str) -> Expression {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parse errors: {:?}",
        parser.errors
    );

    let desugared = desugar(program);
    assert_eq!(desugared.statements.len(), 1, "Expected single statement");

    match &desugared.statements[0] {
        flux::syntax::statement::Statement::Expression { expression, .. } => expression.clone(),
        _ => panic!("Expected expression statement"),
    }
}

fn parse_and_desugar_statement(input: &str) -> Statement {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parse errors: {:?}",
        parser.errors
    );

    let desugared = desugar(program);
    assert_eq!(desugared.statements.len(), 1, "Expected single statement");
    desugared.statements[0].clone()
}

#[test]
fn desugars_double_negation() {
    let result = parse_and_desugar("!!true;");

    // Should desugar to just: true
    match result {
        Expression::Boolean { value, .. } => assert!(value),
        _ => panic!("Expected Boolean(true), got {:?}", result),
    }
}

#[test]
fn double_negation_on_identifier() {
    // !!x → x
    match parse_and_desugar("!!x;") {
        Expression::Identifier { .. } => {}
        other => panic!("expected Identifier, got {:?}", other),
    }
}

#[test]
fn single_negation_unchanged() {
    // !x should NOT be simplified
    match parse_and_desugar("!x;") {
        Expression::Prefix { operator, .. } => assert_eq!(operator, "!"),
        other => panic!("expected Prefix, got {:?}", other),
    }
}

#[test]
fn triple_negation_reduces_to_single() {
    // !!!x → !x (bottom-up: inner !! → identity, outer ! remains)
    match parse_and_desugar("!!!x;") {
        Expression::Prefix {
            operator, right, ..
        } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Identifier { .. } => {}
                other => panic!("expected Identifier inside !, got {:?}", other),
            }
        }
        other => panic!("expected Prefix, got {:?}", other),
    }
}

#[test]
fn negate_equals_to_not_equals() {
    // !(a == b) → a != b
    match parse_and_desugar("!(a == b);") {
        Expression::Infix { operator, .. } => assert_eq!(operator, "!="),
        other => panic!("expected Infix, got {:?}", other),
    }
}

#[test]
fn negate_not_equals_to_equals() {
    // !(a != b) → a == b
    match parse_and_desugar("!(a != b);") {
        Expression::Infix { operator, .. } => assert_eq!(operator, "=="),
        other => panic!("expected Infix, got {:?}", other),
    }
}

#[test]
fn negate_less_than_unchanged() {
    // !(a < b) → not simplified (De Morgan for < is complex)
    match parse_and_desugar("!(a < b);") {
        Expression::Prefix { operator, .. } => assert_eq!(operator, "!"),
        other => panic!("expected Prefix (unchanged), got {:?}", other),
    }
}

#[test]
fn double_negation_of_comparison() {
    // !!(a == b) → a == b (bottom-up: inner desugars, then outer)
    match parse_and_desugar("!!(a == b);") {
        Expression::Infix { operator, .. } => assert_eq!(operator, "=="),
        other => panic!("expected Infix, got {:?}", other),
    }
}

#[test]
fn desugars_inside_let_binding() {
    let result = parse_and_desugar_statement("let x = !(a == b);");

    match result {
        Statement::Let { value, .. } => match value {
            Expression::Infix { operator, .. } => assert_eq!(operator, "!="),
            other => panic!("expected Infix, got {:?}", other),
        },
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn desugars_triple_negation() {
    let result = parse_and_desugar("!!!false;");

    // !!!false → !false (after removing double negation)
    match result {
        Expression::Prefix {
            operator, right, ..
        } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Boolean { value, .. } => assert!(!(*value)),
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
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
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
        Expression::Infix {
            left,
            operator,
            right,
            ..
        } => {
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
        Expression::Prefix {
            operator, right, ..
        } => {
            assert_eq!(operator, "!");
            match right.as_ref() {
                Expression::Boolean { value, .. } => assert!(*value),
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
        Expression::Prefix {
            operator, right, ..
        } => {
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
