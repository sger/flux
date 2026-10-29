use super::*;

#[test]
fn let_statements() {
    let (program, interner) = parse("let x = 5; let y = 10;");
    assert_eq!(program.statements.len(), 2);

    match &program.statements[0] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "x"),
        _ => panic!("expected Let statement"),
    }
}

#[test]
fn return_statements() {
    let (program, _interner) = parse("return 5; return;");
    assert_eq!(program.statements.len(), 2);

    match &program.statements[1] {
        Statement::Return { value: None, .. } => {}
        _ => panic!("expected Return with no value"),
    }
}

#[test]
fn operator_precedence() {
    let tests = vec![
        ("a + b * c;", "(a + (b * c))"),
        ("(a + b) * c;", "((a + b) * c)"),
        ("-a * b;", "((-a) * b)"),
        ("!-a;", "(!(-a))"),
        ("a + b + c;", "((a + b) + c)"),
    ];

    for (input, expected) in tests {
        let (program, interner) = parse(input);
        assert_eq!(
            program.display_with(&interner),
            format!("{};", expected),
            "Failed for: {}",
            input
        );
    }
}

#[test]
fn if_expression() {
    let (program, _interner) = parse("if x < y { x; };");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn if_else_expression() {
    let (program, _interner) = parse("if x < y { x; } else { y; };");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn do_block_expression_parses() {
    let (program, _interner) = parse("let y = do { let x = 1; x + 1 };");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Let { value: Expression::DoBlock { .. }, .. } => {}
        _ => panic!("expected let initializer to be a do block"),
    }
}

#[test]
fn test_do_missing_brace_reports_contextual_e034() {
    let lexer = Lexer::new("let y = do x + 1;");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 diagnostic for missing do-block brace");
    assert!(
        diag.message().is_some_and(|m| m.contains("`do` block")),
        "expected contextual do-block message, got: {:?}",
        diag.message()
    );
}

#[test]
fn expression_statement_tracks_semicolon_presence() {
    let (program, _interner) = parse("x; y");
    assert_eq!(program.statements.len(), 2);
    match &program.statements[0] {
        Statement::Expression { has_semicolon, .. } => assert!(*has_semicolon),
        _ => panic!("expected expression statement"),
    }
    match &program.statements[1] {
        Statement::Expression { has_semicolon, .. } => assert!(!*has_semicolon),
        _ => panic!("expected expression statement"),
    }
}

#[test]
fn test_match_arms_comma_separated_is_valid() {
    let (program, _interner) = parse("match x { 0 -> 1, 1 -> 2 };");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_match_arms_trailing_comma_is_valid() {
    let (program, _interner) = parse(
        r#"
            match x {
                0 -> 1,
                1 -> 2,
            };
        "#,
    );
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_match_arm_guard_parses_and_attaches_to_arm() {
    let (program, _interner) = parse("match x { a if a > 0 -> 1, _ -> 0 };");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Expression { expression: Expression::Match { arms, .. }, .. } => {
            assert_eq!(arms.len(), 2);
            assert!(
                matches!(arms[0].guard.as_ref(), Some(Expression::Infix { .. })),
                "expected first arm guard to parse as expression"
            );
            assert!(arms[1].guard.is_none(), "expected second arm to be unguarded");
        }
        _ => panic!("expected match expression statement"),
    }
}

#[test]
fn test_malformed_match_guard_reports_diagnostic_and_recovers() {
    let lexer = Lexer::new("match x { a if -> 1, _ -> 0 }\nlet y = 3;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E031")),
        "expected malformed guard to report EXPECTED_EXPRESSION"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ),
        "expected parser to continue after malformed match guard"
    );
}

#[test]
fn test_match_arms_semicolon_separator_reports_clear_diagnostic_and_recovers() {
    let lexer = Lexer::new("match x { 0 -> 1; 1 -> 2 }\nlet y = 3;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let sep_diags: Vec<_> = parser
        .errors
        .iter()
        .filter(|d| {
            d.message().is_some_and(|m| m.contains("Match arms must be separated by `,`, not `;`"))
        })
        .collect();
    assert_eq!(sep_diags.len(), 1, "expected one separator diagnostic");
    assert_eq!(
        parser.errors.iter().filter(|d| d.code() == Some("E031")).count(),
        0,
        "semicolon separator recovery should avoid EXPECTED_EXPRESSION cascades"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ),
        "expected parser to continue after invalid match separator"
    );
}

#[test]
fn test_match_arms_mixed_separators_reports_semicolon_and_recovers() {
    let lexer = Lexer::new("match x { 0 -> 1, 1 -> 2; _ -> 3 }\nlet y = 3;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let sep_diags: Vec<_> = parser
        .errors
        .iter()
        .filter(|d| {
            d.message().is_some_and(|m| m.contains("Match arms must be separated by `,`, not `;`"))
        })
        .collect();
    assert_eq!(sep_diags.len(), 1, "expected one separator diagnostic");
    assert_eq!(
        parser.errors.iter().filter(|d| d.code() == Some("E031")).count(),
        0,
        "mixed separator recovery should avoid EXPECTED_EXPRESSION cascades"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ),
        "expected parser to continue after mixed match separators"
    );
}

#[test]
fn function_literal() {
    let (program, _interner) = parse("fn(x, y) { x + y; };");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_function_statement() {
    let (program, interner) = parse("fn add(x, y) { x + y; }");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Function { name, parameters, .. } => {
            assert_eq!(interner.resolve(*name), "add");
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["x", "y"]);
        }
        _ => panic!("expected Function statement"),
    }
}

#[test]
fn test_call_expression() {
    let (program, _interner) = parse("add(1, 2 * 3);");
    assert_eq!(program.statements.len(), 1);
}
