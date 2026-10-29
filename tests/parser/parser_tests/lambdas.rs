use super::*;

// Lambda shorthand tests
#[test]
fn test_lambda_single_param() {
    let (program, _interner) = parse(r"\x -> x * 2;");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Expression { expression: Expression::Function { span, .. }, .. } => {
            assert_eq!(span.start, Position::new(1, 0));
            assert_eq!(span.end, Position::new(1, 11));
        }
        _ => panic!("expected lambda function expression"),
    }
}

#[test]
fn test_lambda_single_ident_param_span_and_arity() {
    let (program, interner) = parse(r"\x -> x");
    assert_eq!(program.statements.len(), 1);

    match &program.statements[0] {
        Statement::Expression {
            expression: Expression::Function { parameters, span, .. }, ..
        } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["x"]);
            assert_eq!(span.start, Position::new(1, 0));
            assert_eq!(span.end, Position::new(1, 7));
        }
        _ => panic!("expected lambda function expression"),
    }
}

#[test]
fn test_lambda_single_param_sugar_and_parenthesized_have_same_ast_shape() {
    let (sugar, sugar_interner) = parse(r"\x -> x");
    let (parenthesized, paren_interner) = parse(r"\(x) -> x");

    let sugar_params = match &sugar.statements[0] {
        Statement::Expression { expression: Expression::Function { parameters, .. }, .. } => {
            parameters.clone()
        }
        _ => panic!("expected lambda function expression"),
    };
    let paren_params = match &parenthesized.statements[0] {
        Statement::Expression { expression: Expression::Function { parameters, .. }, .. } => {
            parameters.clone()
        }
        _ => panic!("expected lambda function expression"),
    };

    assert_eq!(sugar_params.len(), 1);
    assert_eq!(paren_params.len(), 1);
    let sugar_names: Vec<_> = sugar_params.iter().map(|p| sugar_interner.resolve(*p)).collect();
    let paren_names: Vec<_> = paren_params.iter().map(|p| paren_interner.resolve(*p)).collect();
    assert_eq!(sugar_names, paren_names);
    assert_eq!(sugar_interner.resolve(sugar_params[0]), "x");
}

#[test]
fn test_lambda_multi_param() {
    let (program, _interner) = parse(r"\(x, y) -> x + y;");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_lambda_no_param() {
    let (program, _interner) = parse(r"\() -> 42;");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_lambda_block_body() {
    let (program, _interner) = parse(r"\x -> { let y = x * 2; y + 1 };");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_lambda_in_let() {
    let (program, interner) = parse(r"let double = \x -> x * 2;");
    assert_eq!(program.statements.len(), 1);
    match &program.statements[0] {
        Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "double"),
        _ => panic!("expected Let statement"),
    }
}

#[test]
fn test_let_in_expression_position_reports_targeted_diagnostic() {
    let lexer = Lexer::new(r"let f = \x -> let y = x + 1");
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    let targeted = parser.errors.iter().find(|d| {
        d.code() == Some("E031")
            && d.message().is_some_and(|m| {
                m.contains("`let` is a statement and cannot appear in an expression position.")
            })
    });
    assert!(
        targeted.is_some(),
        "expected targeted let-in-expression diagnostic, got: {:?}",
        parser.errors
    );

    let e034_count = parser.errors.iter().filter(|d| d.code() == Some("E034")).count();
    assert!(
        e034_count <= 1,
        "expected no flood of unrelated E034 diagnostics, got: {:?}",
        parser.errors
    );
}

#[test]
fn test_let_in_if_and_match_expression_positions_reports_targeted_diagnostic() {
    let inputs = ["if let x = 1 { 1 } else { 0 }", "match 1 { 1 -> let y = 2, _ -> 0 }"];

    for input in inputs {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();
        assert!(
            parser.errors.iter().any(|d| {
                d.code() == Some("E031")
                    && d.message().is_some_and(|m| {
                        m.contains(
                            "`let` is a statement and cannot appear in an expression position.",
                        )
                    })
            }),
            "expected targeted let-in-expression diagnostic for input `{}`; got: {:?}",
            input,
            parser.errors
        );
    }
}

#[test]
fn test_lambda_as_argument() {
    let (program, _interner) = parse(r"map(arr, \x -> x * 2);");
    assert_eq!(program.statements.len(), 1);
}

#[test]
fn test_lambda_missing_arrow_reports_expected_token_error() {
    let lexer = Lexer::new(r"\x x");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    assert_eq!(parser.errors.len(), 1, "expected exactly one diagnostic for missing arrow");
    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 diagnostic for missing lambda arrow");
    assert!(diag.message().is_some_and(|m| m.contains("lambda parameters")));
    assert!(
        diag.hints()
            .iter()
            .any(|h| h.text.contains(r"\x -> expr") || h.text.contains(r"\(x, y) -> expr"))
    );
    let span = diag.span().expect("expected diagnostic span");
    assert_eq!(span.start.line, 1);
    assert_eq!(span.start.column, 1);
}

#[test]
fn test_match_missing_arrow_reports_contextual_e034() {
    let lexer = Lexer::new("match 1 { 0 1, _ -> 0 }");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 diagnostic for missing match arm arrow");
    assert!(
        diag.message().is_some_and(|m| m.contains("match arm") && m.contains("`->`")),
        "expected contextual match-arrow message, got: {:?}",
        diag.message()
    );
}

#[test]
fn test_orphan_constructor_pattern_statement_reports_contextual_e034() {
    let lexer = Lexer::new("Some(x);\nlet after = 1;");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 orphan constructor-pattern diagnostic");
    assert!(
        diag.message().is_some_and(|m| m.contains("outside `match`")),
        "expected orphan-constructor contextual message, got: {:?}",
        diag.message()
    );
}

#[test]
fn test_orphan_constructor_pattern_does_not_misfire_in_expression_contexts() {
    let (program, _interner) = parse(
        r#"
let v = Some(1);
print(Some(1));
let w = match x { Some(n) -> n, None -> 0 };
"#,
    );
    assert_eq!(program.statements.len(), 3);
}

#[test]
fn test_non_identifier_function_parameter_reports_diagnostic() {
    let lexer = Lexer::new("fn add(x, 1) { x }");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for non-identifier function parameter");
    assert!(diag.message().is_some_and(|m| m.contains("parameter name here")));
}

#[test]
fn test_non_identifier_lambda_parameter_reports_diagnostic() {
    let lexer = Lexer::new(r"\(x, 1) -> x");
    let mut parser = Parser::new(lexer);
    let _ = parser.parse_program();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for non-identifier lambda parameter");
    assert!(diag.message().is_some_and(|m| m.contains("parameter name here")));
}

#[test]
fn test_single_param_keyword_lambda_matches_parenthesized_diagnostic() {
    let mut single_parser = Parser::new(Lexer::new(r"\if -> 1"));
    let _ = single_parser.parse_program();

    let mut paren_parser = Parser::new(Lexer::new(r"\(if) -> 1"));
    let _ = paren_parser.parse_program();

    let single_diag = single_parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for single-param keyword lambda");
    let paren_diag = paren_parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for parenthesized keyword lambda");

    assert!(
        single_diag
            .message()
            .is_some_and(|m| m.contains("parameter name here") && m.contains("`if`"))
    );
    assert_eq!(single_diag.message(), paren_diag.message());
    assert_eq!(single_parser.errors.len(), 1);
    assert_eq!(paren_parser.errors.len(), 1);
    assert!(
        single_parser.errors.iter().all(|d| d.code() != Some("E036")),
        "single-param keyword lambda should use parameter validation, not syntax error"
    );

    let single_span = single_diag.span().expect("expected single-param span");
    let paren_span = paren_diag.span().expect("expected parenthesized span");
    assert_eq!(single_span.start, Position::new(1, 1));
    assert_eq!(paren_span.start, Position::new(1, 2));
}

#[test]
fn test_single_param_illegal_token_lambda_matches_parenthesized_diagnostic() {
    let mut single_parser = Parser::new(Lexer::new(r"\1 -> 1"));
    let _ = single_parser.parse_program();

    let mut paren_parser = Parser::new(Lexer::new(r"\(1) -> 1"));
    let _ = paren_parser.parse_program();

    let single_diag = single_parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for single-param illegal token lambda");
    let paren_diag = paren_parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for parenthesized illegal token lambda");

    assert!(
        single_diag
            .message()
            .is_some_and(|m| m.contains("parameter name here") && m.contains("integer literal"))
    );
    assert_eq!(single_diag.message(), paren_diag.message());
    assert_eq!(single_parser.errors.len(), 1);
    assert_eq!(paren_parser.errors.len(), 1);

    let single_span = single_diag.span().expect("expected single-param span");
    let paren_span = paren_diag.span().expect("expected parenthesized span");
    assert_eq!(single_span.start, Position::new(1, 1));
    assert_eq!(paren_span.start, Position::new(1, 2));
}

#[test]
fn test_invalid_function_parameter_recovers_and_continues() {
    let lexer = Lexer::new("fn f(1) { 1 }\nlet x = 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for non-identifier function parameter");
    let span = diag.span().expect("expected diagnostic span");
    assert_eq!(span.start, Position::new(1, 5));

    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "x")
        ),
        "expected parser to continue and parse trailing let statement"
    );
}

#[test]
fn test_function_parameter_recovery_keeps_later_valid_parameter() {
    let lexer = Lexer::new("fn f(1, x) { x }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for non-identifier function parameter");
    let span = diag.span().expect("expected diagnostic span");
    assert_eq!(span.start, Position::new(1, 5));

    match &program.statements[0] {
        Statement::Function { parameters, .. } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["x"]);
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_function_parameter_missing_comma_reports_at_offending_token() {
    let lexer = Lexer::new("fn f(a b) { }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let diag = parser
        .errors
        .iter()
        .find(|d| d.code() == Some("E034"))
        .expect("expected E034 for missing comma between parameters");
    let span = diag.span().expect("expected diagnostic span");
    assert_eq!(span.start, Position::new(1, 7));
    assert!(diag.message().is_some_and(|m| m.contains("after function parameter")));

    match &program.statements[0] {
        Statement::Function { parameters, .. } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["a"]);
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_function_parameter_trailing_comma_is_accepted() {
    let lexer = Lexer::new("fn f(a,) { }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.is_empty(),
        "expected no diagnostics for trailing comma in function parameters"
    );

    match &program.statements[0] {
        Statement::Function { parameters, .. } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["a"]);
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_function_parameter_double_comma_recovers_and_keeps_later_parameter() {
    let lexer = Lexer::new("fn f(a,,b) { b }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let e105_diags: Vec<_> = parser.errors.iter().filter(|d| d.code() == Some("E034")).collect();
    assert_eq!(e105_diags.len(), 1, "expected one E034 for extra comma");
    assert!(e105_diags[0].message().is_some_and(|m| m.contains("parameter name here")));

    match &program.statements[0] {
        Statement::Function { parameters, .. } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["a", "b"]);
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_function_parameter_leading_comma_recovers_and_keeps_parameter() {
    let lexer = Lexer::new("fn f(,x) { x }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let e105_diags: Vec<_> = parser.errors.iter().filter(|d| d.code() == Some("E034")).collect();
    assert_eq!(e105_diags.len(), 1, "expected one E034 for leading comma");
    assert!(e105_diags[0].message().is_some_and(|m| m.contains("parameter name here")));

    match &program.statements[0] {
        Statement::Function { parameters, .. } => {
            let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
            assert_eq!(param_names, vec!["x"]);
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_malformed_let_type_annotation_keeps_value_and_followup_statement() {
    let lexer = Lexer::new("let x: = 1;\nlet y = 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E034")),
        "expected malformed let annotation diagnostic"
    );

    match &program.statements[0] {
        Statement::Let { name, type_annotation, value, .. } => {
            assert_eq!(interner.resolve(*name), "x");
            assert!(type_annotation.is_none(), "annotation should be dropped");
            assert!(matches!(value, Expression::Integer { value, .. } if *value == 1));
        }
        _ => panic!("expected first statement to remain a let binding"),
    }

    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ),
        "expected follow-up let statement after malformed annotation"
    );
}

#[test]
fn test_malformed_function_param_annotation_keeps_later_valid_parameter_type() {
    let lexer = Lexer::new("fn f(a: , b: Int) { b }");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E034")),
        "expected malformed function parameter annotation diagnostic"
    );

    match &program.statements[0] {
        Statement::Function { parameters, parameter_types, .. } => {
            assert_eq!(parameters.len(), 2);
            assert_eq!(parameter_types.len(), 2);
            assert!(parameter_types[0].is_none(), "malformed annotation should become None");
            assert!(
                parameter_types[1].is_some(),
                "later valid parameter annotation should be preserved"
            );
        }
        _ => panic!("expected function statement"),
    }
}

#[test]
fn test_malformed_function_return_annotation_recovers_to_body() {
    let lexer = Lexer::new("fn f() -> { 1 }\nlet after = 2;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E034")),
        "expected malformed return annotation diagnostic"
    );

    match &program.statements[0] {
        Statement::Function { return_type, body, .. } => {
            assert!(return_type.is_none(), "malformed return annotation should be dropped");
            assert!(
                !body.statements.is_empty(),
                "function body should still be parsed after recovery"
            );
        }
        _ => panic!("expected function statement"),
    }

    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
        ),
        "expected parser to keep follow-up statement after malformed function return annotation"
    );
}

#[test]
fn test_malformed_lambda_annotation_recovers_and_keeps_followup_statement() {
    let lexer = Lexer::new("let g = \\x: -> x;\nlet after = 1;");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| d.code() == Some("E034")),
        "expected malformed lambda annotation diagnostic"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "g")
        ),
        "expected malformed lambda statement to remain parseable"
    );
    assert!(
        program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
        ),
        "expected follow-up let statement after malformed lambda annotation"
    );
}
