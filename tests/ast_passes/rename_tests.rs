use std::collections::HashMap;

use flux::ast::rename::{rename, rename_expr};
use flux::syntax::{
    expression::Expression, interner::Interner, lexer::Lexer, parser::Parser, program::Program,
    statement::Statement,
};

fn parse(input: &str) -> (Program, Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

#[test]
fn rename_single_identifier() {
    let (program, mut interner) = parse("let a = 1;");
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let map = HashMap::from([(sym_a, sym_b)]);
    let rewritten = rename(program, map);

    match &rewritten.statements[0] {
        Statement::Let { name, .. } => assert_eq!(*name, sym_b),
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn rename_multiple_identifiers() {
    let (program, mut interner) = parse("let x = a + b;");
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");
    let sym_c = interner.intern("c");
    let sym_d = interner.intern("d");

    let map = HashMap::from([(sym_a, sym_c), (sym_b, sym_d)]);
    let rewritten = rename(program, map);

    match &rewritten.statements[0] {
        Statement::Let { value, .. } => match value {
            Expression::Infix { left, right, .. } => {
                match left.as_ref() {
                    Expression::Identifier { name, .. } => assert_eq!(*name, sym_c),
                    other => panic!("expected Identifier, got {:?}", other),
                }
                match right.as_ref() {
                    Expression::Identifier { name, .. } => assert_eq!(*name, sym_d),
                    other => panic!("expected Identifier, got {:?}", other),
                }
            }
            other => panic!("expected Infix, got {:?}", other),
        },
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn unmapped_identifiers_preserved() {
    let (program, mut interner) = parse("let x = a + y;");
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");
    let sym_x = interner.intern("x");
    let sym_y = interner.intern("y");

    let map = HashMap::from([(sym_a, sym_b)]);
    let rewritten = rename(program, map);

    match &rewritten.statements[0] {
        Statement::Let { name, value, .. } => {
            // x is not in the map, stays x
            assert_eq!(*name, sym_x);
            match value {
                Expression::Infix { left, right, .. } => {
                    match left.as_ref() {
                        Expression::Identifier { name, .. } => assert_eq!(*name, sym_b),
                        other => panic!("expected Identifier, got {:?}", other),
                    }
                    // y is not in the map, stays y
                    match right.as_ref() {
                        Expression::Identifier { name, .. } => assert_eq!(*name, sym_y),
                        other => panic!("expected Identifier, got {:?}", other),
                    }
                }
                other => panic!("expected Infix, got {:?}", other),
            }
        }
        other => panic!("expected Let, got {:?}", other),
    }
}

#[test]
fn rename_in_function_params_and_body() {
    let (program, mut interner) = parse("fn f(a) { return a; }");
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let map = HashMap::from([(sym_a, sym_b)]);
    let rewritten = rename(program, map);

    match &rewritten.statements[0] {
        Statement::Function {
            parameters, body, ..
        } => {
            assert_eq!(parameters[0], sym_b);
            match &body.statements[0] {
                Statement::Return {
                    value: Some(expr), ..
                } => match expr {
                    Expression::Identifier { name, .. } => assert_eq!(*name, sym_b),
                    other => panic!("expected Identifier, got {:?}", other),
                },
                other => panic!("expected Return, got {:?}", other),
            }
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn rename_expr_standalone() {
    let (program, mut interner) = parse("a + 1;");
    let sym_a = interner.intern("a");
    let sym_z = interner.intern("z");

    let expr = match program.statements.into_iter().next().unwrap() {
        Statement::Expression { expression, .. } => expression,
        other => panic!("expected Expression, got {:?}", other),
    };

    let map = HashMap::from([(sym_a, sym_z)]);
    let rewritten = rename_expr(expr, map);

    match &rewritten {
        Expression::Infix { left, .. } => match left.as_ref() {
            Expression::Identifier { name, .. } => assert_eq!(*name, sym_z),
            other => panic!("expected Identifier, got {:?}", other),
        },
        other => panic!("expected Infix, got {:?}", other),
    }
}

#[test]
fn rename_in_match_pattern() {
    let (program, mut interner) = parse("match x { Some(a) -> a, _ -> 0 };");
    let sym_a = interner.intern("a");
    let sym_b = interner.intern("b");

    let map = HashMap::from([(sym_a, sym_b)]);
    let rewritten = rename(program, map);

    // The pattern binding and body reference should both be renamed
    match &rewritten.statements[0] {
        Statement::Expression { expression, .. } => match expression {
            Expression::Match { arms, .. } => {
                // First arm: Some(b) -> b
                match &arms[0].pattern {
                    flux::syntax::expression::Pattern::Some { pattern, .. } => {
                        match pattern.as_ref() {
                            flux::syntax::expression::Pattern::Identifier { name, .. } => {
                                assert_eq!(*name, sym_b);
                            }
                            other => panic!("expected Pattern::Identifier, got {:?}", other),
                        }
                    }
                    other => panic!("expected Pattern::Some, got {:?}", other),
                }
                match &arms[0].body {
                    Expression::Identifier { name, .. } => assert_eq!(*name, sym_b),
                    other => panic!("expected Identifier, got {:?}", other),
                }
            }
            other => panic!("expected Match, got {:?}", other),
        },
        other => panic!("expected Expression, got {:?}", other),
    }
}
