use super::*;

fn spaced_ints(count: usize) -> String {
    let mut out = String::new();
    for i in 1..=count {
        if i > 1 {
            out.push(' ');
        }
        out.push_str(&i.to_string());
    }
    out
}

fn semicolon_separated_match_arms(count: usize) -> String {
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push_str("; ");
        }
        out.push_str(&format!("{i} -> {i}"));
    }
    out
}

#[test]
fn test_call_expression_trailing_comma_is_accepted() {
    let lexer = Lexer::new("f(1,)");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(
        parser.errors.is_empty(),
        "expected no diagnostics for trailing comma in call arguments"
    );
    match &program.statements[0] {
        Statement::Expression { expression: Expression::Call { arguments, .. }, .. } => {
            assert_eq!(arguments.len(), 1);
        }
        _ => panic!("expected call expression statement"),
    }
}

#[test]
fn test_array_literal_trailing_comma_is_accepted() {
    let lexer = Lexer::new("#[1,]");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(
        parser.errors.is_empty(),
        "expected no diagnostics for trailing comma in array literal"
    );
    match &program.statements[0] {
        Statement::Expression { expression: Expression::ArrayLiteral { elements, .. }, .. } => {
            assert_eq!(elements.len(), 1);
        }
        _ => panic!("expected array expression statement"),
    }
}

#[test]
fn test_bar_array_literal_trailing_comma_is_accepted() {
    let lexer = Lexer::new("[|1,|]");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(
        parser.errors.is_empty(),
        "expected no diagnostics for trailing comma in [|...|] array literal"
    );
    match &program.statements[0] {
        Statement::Expression { expression: Expression::ArrayLiteral { elements, .. }, .. } => {
            assert_eq!(elements.len(), 1);
        }
        _ => panic!("expected array expression statement"),
    }
}

#[test]
fn test_empty_bar_array_literal_is_accepted() {
    let lexer = Lexer::new("[||]");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(parser.errors.is_empty(), "expected no diagnostics for empty [||] array literal");
    match &program.statements[0] {
        Statement::Expression { expression: Expression::ArrayLiteral { elements, .. }, .. } => {
            assert!(elements.is_empty());
        }
        _ => panic!("expected array expression statement"),
    }
}

#[test]
fn test_call_expression_double_comma_reports_at_second_comma() {
    let lexer = Lexer::new("f(1,,2)");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for double comma in call args");
    let span = diag.span().expect("expected diagnostic span");
    assert_eq!(span.start, Position::new(1, 4));
    assert!(diag.message().is_some_and(|m| m.contains("Expected expression after `,`, got `,`")));
}

#[test]
fn test_hash_literal_trailing_comma_is_accepted() {
    let (program, interner) = parse(r#"{"one": 1,}"#);
    assert_eq!(program.statements.len(), 1);
    assert_eq!(program.display_with(&interner), "{\"one\": 1}");
}

#[test]
fn test_nested_trailing_commas_are_accepted() {
    let (program, _interner) = parse("f(#[1,2,], g(3,4,),)");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Expression { expression: Expression::Call { arguments, .. }, .. } => {
            assert_eq!(arguments.len(), 2);
            assert!(
                matches!(&arguments[0], Expression::ArrayLiteral { elements, .. } if elements.len() == 2)
            );
            assert!(
                matches!(&arguments[1], Expression::Call { arguments, .. } if arguments.len() == 2)
            );
        }
        _ => panic!("expected nested call expression statement"),
    }
}

#[test]
fn test_nested_trailing_commas_with_bar_arrays_are_accepted() {
    let (program, _interner) = parse("f([|1,2,|], g(3,4,),)");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Expression { expression: Expression::Call { arguments, .. }, .. } => {
            assert_eq!(arguments.len(), 2);
            assert!(
                matches!(&arguments[0], Expression::ArrayLiteral { elements, .. } if elements.len() == 2)
            );
            assert!(
                matches!(&arguments[1], Expression::Call { arguments, .. } if arguments.len() == 2)
            );
        }
        _ => panic!("expected nested call expression statement"),
    }
}

#[test]
fn test_missing_comma_call_arg_recovery_keeps_later_argument() {
    let lexer = Lexer::new("f(a, b c, d)");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(missing_comma_count, 1, "expected one missing comma diagnostic");
    assert!(
        parser.errors.iter().all(|d| d.code() == Some("E073")),
        "missing-comma recovery should avoid generic cascade diagnostics"
    );

    match &program.statements[0] {
        Statement::Expression { expression: Expression::Call { arguments, .. }, .. } => {
            assert_eq!(arguments.len(), 4, "expected trailing argument to be preserved");
            assert!(
                matches!(&arguments[3], Expression::Identifier { name, .. } if interner.resolve(*name) == "d"),
                "expected final argument `d` to be parsed after recovery"
            );
        }
        _ => panic!("expected call expression statement"),
    }
}

#[test]
fn test_assignment_is_not_valid_in_expression_position() {
    let lexer = Lexer::new("let x = 1;\nlet y = 2;\nif x = y { x; }\nlet after = 3;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E034")),
        "expected parser to reject assignment in expression position"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
        ),
        "expected parser recovery to continue after invalid assignment expression"
    );
}

#[test]
fn test_bad_assignment_statement_recovers_to_following_statement() {
    let lexer = Lexer::new("let x = 1;\nx = ;\nlet after = 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E031")),
        "expected missing-expression diagnostic for malformed assignment"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
        ),
        "expected parser recovery to continue after malformed assignment statement"
    );
}

#[test]
fn test_tuple_literal_parsing() {
    let (program, _interner) = parse("(1, 2, 3);");
    match &program.statements[0] {
        Statement::Expression { expression: Expression::TupleLiteral { elements, .. }, .. } => {
            assert_eq!(elements.len(), 3)
        }
        _ => panic!("expected tuple literal"),
    }
}

#[test]
fn test_singleton_and_unit_tuple_parsing() {
    let (program, _interner) = parse("(42,); ()");
    match &program.statements[0] {
        Statement::Expression { expression: Expression::TupleLiteral { elements, .. }, .. } => {
            assert_eq!(elements.len(), 1)
        }
        _ => panic!("expected singleton tuple"),
    }
    match &program.statements[1] {
        Statement::Expression { expression: Expression::TupleLiteral { elements, .. }, .. } => {
            assert!(elements.is_empty())
        }
        _ => panic!("expected unit tuple"),
    }
}

#[test]
fn test_parenthesized_grouping_stays_grouping() {
    let (program, _interner) = parse("(1 + 2);");
    match &program.statements[0] {
        Statement::Expression { expression: Expression::Infix { .. }, .. } => {}
        _ => panic!("expected grouped infix expression"),
    }
}

#[test]
fn test_tuple_field_access_parsing() {
    let (program, _interner) = parse("t.0;");
    match &program.statements[0] {
        Statement::Expression {
            expression: Expression::TupleFieldAccess { index, .. }, ..
        } => assert_eq!(*index, 0),
        _ => panic!("expected tuple field access"),
    }
}

#[test]
fn test_let_tuple_destructure_parsing() {
    let (program, _interner) = parse("let (a, b) = pair;");
    match &program.statements[0] {
        Statement::LetDestructure { pattern, .. } => match pattern {
            Pattern::Tuple { elements, .. } => assert_eq!(elements.len(), 2),
            _ => panic!("expected tuple pattern"),
        },
        _ => panic!("expected let destructure statement"),
    }
}

#[test]
fn test_assignment_parses_as_statement_top_level_and_in_block() {
    let (program, interner) = parse("x = 2;\nfn f() { y = 3; }");

    assert!(
        matches!(&program.statements[0], Statement::Assign { name, .. } if interner.resolve(*name) == "x"),
        "expected top-level assignment to parse as Statement::Assign"
    );

    match &program.statements[1] {
        Statement::Function { body, .. } => {
            assert!(
                    body.statements
                        .iter()
                        .any(|stmt| matches!(stmt, Statement::Assign { name, .. } if interner.resolve(*name) == "y")),
                    "expected block assignment to parse as Statement::Assign"
                );
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_call_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
    let input = format!("print({});\nlet ok = \"still parsed\";\nprint(ok);", spaced_ints(200));
    let lexer = Lexer::new(&input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let summary_diags: Vec<_> = parser
        .errors
        .iter()
        .filter(|d| d.message().is_some_and(|m| m.contains("Too many errors in this list")))
        .collect();
    assert_eq!(summary_diags.len(), 1, "expected exactly one list error-limit summary diagnostic");

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(
        missing_comma_count, 50,
        "expected missing-comma diagnostics to stop at the list limit"
    );
    assert!(
        parser.errors.len() <= 51,
        "expected list diagnostics to be capped at 50 plus one summary"
    );

    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")
        ),
        "expected parser to continue and parse trailing let statement"
    );
    assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if interner.resolve(*name) == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if interner.resolve(*name) == "ok")
                )
            }),
            "expected parser to continue and parse trailing print(ok) call"
        );
}

#[test]
fn test_array_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
    let input = format!("[{}];\nlet ok = \"still parsed\";\nprint(ok);", spaced_ints(200));
    let lexer = Lexer::new(&input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let summary_diags: Vec<_> = parser
        .errors
        .iter()
        .filter(|d| d.message().is_some_and(|m| m.contains("Too many errors in this list")))
        .collect();
    assert_eq!(summary_diags.len(), 1, "expected exactly one list error-limit summary diagnostic");

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(
        missing_comma_count, 50,
        "expected missing-comma diagnostics to stop at the list limit"
    );
    assert!(
        parser.errors.len() <= 51,
        "expected list diagnostics to be capped at 50 plus one summary"
    );

    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")
        ),
        "expected parser to continue and parse trailing let statement"
    );
    assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if interner.resolve(*name) == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if interner.resolve(*name) == "ok")
                )
            }),
            "expected parser to continue and parse trailing print(ok) call"
        );
}

#[test]
fn test_match_arm_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
    let input = format!("match x {{ {} }}\nlet ok = 1;", semicolon_separated_match_arms(200));
    let lexer = Lexer::new(&input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let summary_diags: Vec<_> = parser
        .errors
        .iter()
        .filter(|d| {
            d.message().is_some_and(|m| m.contains("Too many errors in this match arm list"))
        })
        .collect();
    assert_eq!(
        summary_diags.len(),
        1,
        "expected exactly one match arm error-limit summary diagnostic"
    );
    assert!(
        parser.errors.len() <= 51,
        "expected match-arm diagnostics to be capped at 50 plus one summary"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")
        ),
        "expected parser to continue and parse trailing let statement after match recovery"
    );
}
