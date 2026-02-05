#[cfg(test)]
mod tests {
    use flux::frontend::{
        expression::Expression, lexer::Lexer, parser::Parser, position::Position, program::Program,
        statement::Statement,
    };

    fn parse(input: &str) -> Program {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        assert!(
            parser.errors.is_empty(),
            "Parser errors: {:?}",
            parser.errors
        );
        program
    }

    #[test]
    fn let_statements() {
        let program = parse("let x = 5; let y = 10;");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(name, "x"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn return_statements() {
        let program = parse("return 5; return;");
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
            let program = parse(input);
            assert_eq!(program.to_string(), expected, "Failed for: {}", input);
        }
    }

    #[test]
    fn if_expression() {
        let program = parse("if x < y { x; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn if_else_expression() {
        let program = parse("if x < y { x; } else { y; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn function_literal() {
        let program = parse("fun(x, y) { x + y; };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_function_statement() {
        let program = parse("fun add(x, y) { x + y; }");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Function {
                name, parameters, ..
            } => {
                assert_eq!(name, "add");
                assert_eq!(parameters, &vec!["x".to_string(), "y".to_string()]);
            }
            _ => panic!("expected Function statement"),
        }
    }

    #[test]
    fn test_call_expression() {
        let program = parse("add(1, 2 * 3);");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_string_literal() {
        let program = parse(r#""hello world";"#);
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_unterminated_string_error_uses_lexer_end_position() {
        let input = "\"http://example.com";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E071"))
            .expect("expected E071 unterminated string diagnostic");
        let span = diag.span().expect("expected diagnostic span");

        assert_eq!(span.start, span.end);
        assert_eq!(span.start.line, 1);
        assert_eq!(span.start.column, input.chars().count());
    }

    #[test]
    fn test_unterminated_string_suppression_does_not_leak() {
        let input = "\"#{x}oops\n;\n\"later";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let e072_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E072"))
            .count();
        let e071_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E071"))
            .count();

        assert_eq!(e072_count, 1, "expected one interpolation error");
        assert_eq!(
            e071_count, 1,
            "expected one unterminated string error after recovery"
        );
    }

    #[test]
    fn test_string_interpolation_simple() {
        let program = parse(r#""Hello #{name}""#);
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "\"Hello #{name}\"");
    }

    #[test]
    fn test_string_interpolation_expression() {
        let program = parse(r#""Sum #{1 + 2}""#);
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "\"Sum #{(1 + 2)}\"");
    }

    #[test]
    fn test_string_interpolation_nested_hash_expression_terminates() {
        let program = parse("\"#{ {1: 2} }\"");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_boolean_literals() {
        let program = parse("true; false;");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_array_literal() {
        let program = parse("[1, 2 * 2, 3 + 3];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "[1, (2 * 2), (3 + 3)]");
    }

    #[test]
    fn test_index_expression() {
        let program = parse("arr[1 + 1];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "(arr[(1 + 1)])");
    }

    #[test]
    fn test_hash_literal() {
        let program = parse(r#"{"one": 1, "two": 2};"#);
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_empty_hash() {
        let program = parse("{};");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "{}");
    }

    // Lambda shorthand tests
    #[test]
    fn test_lambda_single_param() {
        let program = parse(r"\x -> x * 2;");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::Function { span, .. },
                ..
            } => {
                assert_eq!(span.start, Position::new(1, 0));
                assert_eq!(span.end, Position::new(1, 11));
            }
            _ => panic!("expected lambda function expression"),
        }
    }

    #[test]
    fn test_lambda_single_ident_param_span_and_arity() {
        let program = parse(r"\x -> x");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Expression {
                expression:
                    Expression::Function {
                        parameters, span, ..
                    },
                ..
            } => {
                assert_eq!(parameters, &vec!["x".to_string()]);
                assert_eq!(span.start, Position::new(1, 0));
                assert_eq!(span.end, Position::new(1, 7));
            }
            _ => panic!("expected lambda function expression"),
        }
    }

    #[test]
    fn test_lambda_multi_param() {
        let program = parse(r"\(x, y) -> x + y;");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_lambda_no_param() {
        let program = parse(r"\() -> 42;");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_lambda_block_body() {
        let program = parse(r"\x -> { let y = x * 2; y + 1 };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_lambda_in_let() {
        let program = parse(r"let double = \x -> x * 2;");
        assert_eq!(program.statements.len(), 1);
        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(name, "double"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn test_lambda_as_argument() {
        let program = parse(r"map(arr, \x -> x * 2);");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_lambda_missing_arrow_reports_expected_token_error() {
        let lexer = Lexer::new(r"\x x");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        assert_eq!(
            parser.errors.len(),
            1,
            "expected exactly one diagnostic for missing arrow"
        );
        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E036"))
            .expect("expected E036 diagnostic for missing lambda arrow");
        assert!(diag.message().is_some_and(|m| m.contains("Expected `->`")));
        assert!(
            diag.hints()
                .iter()
                .any(|h| h.text.contains(r"\x -> expr") || h.text.contains(r"\(x, y) -> expr"))
        );
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start.line, 1);
        assert_eq!(span.start.column, 3);
        assert_eq!(
            parser
                .errors
                .iter()
                .filter(|d| d.code() == Some("E034"))
                .count(),
            0,
            "missing-arrow lambda should not emit generic E034"
        );
    }

    #[test]
    fn test_non_identifier_function_parameter_reports_diagnostic() {
        let lexer = Lexer::new("fun add(x, 1) { x }");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for non-identifier function parameter");
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected identifier as parameter"))
        );
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
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected identifier as parameter"))
        );
    }

    #[test]
    fn test_invalid_function_parameter_recovers_and_continues() {
        let lexer = Lexer::new("fun f(1) { 1 }\nlet x = 2;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for non-identifier function parameter");
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 6));

        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "x")),
            "expected parser to continue and parse trailing let statement"
        );
    }

    #[test]
    fn test_function_parameter_recovery_keeps_later_valid_parameter() {
        let lexer = Lexer::new("fun f(1, x) { x }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for non-identifier function parameter");
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 6));

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["x".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_function_parameter_missing_comma_reports_at_offending_token() {
        let lexer = Lexer::new("fun f(a b) { }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for missing comma between parameters");
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 8));
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected `,` or `)` after parameter"))
        );

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["a".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_function_parameter_trailing_comma_is_rejected_with_one_clear_error() {
        let lexer = Lexer::new("fun f(a,) { }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let e105_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .collect();
        assert_eq!(
            e105_diags.len(),
            1,
            "expected one E034 diagnostic for trailing comma"
        );
        let span = e105_diags[0].span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 8));
        assert!(
            e105_diags[0]
                .message()
                .is_some_and(|m| m.contains("Expected identifier as parameter"))
        );

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["a".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_function_parameter_double_comma_recovers_and_keeps_later_parameter() {
        let lexer = Lexer::new("fun f(a,,b) { b }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let e105_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .collect();
        assert_eq!(e105_diags.len(), 1, "expected one E034 for extra comma");
        assert!(
            e105_diags[0]
                .message()
                .is_some_and(|m| m.contains("Expected identifier as parameter"))
        );

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["a".to_string(), "b".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_function_parameter_leading_comma_recovers_and_keeps_parameter() {
        let lexer = Lexer::new("fun f(,x) { x }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let e105_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .collect();
        assert_eq!(e105_diags.len(), 1, "expected one E034 for leading comma");
        assert!(
            e105_diags[0]
                .message()
                .is_some_and(|m| m.contains("Expected identifier as parameter"))
        );

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["x".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_call_expression_trailing_comma_reports_missing_expression_at_rparen() {
        let lexer = Lexer::new("f(1,)");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for trailing comma in call args");
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 4));
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected expression after `,`, got )"))
        );
    }

    #[test]
    fn test_array_literal_trailing_comma_reports_missing_expression_at_rbracket() {
        let lexer = Lexer::new("[1,]");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 for trailing comma in array literal");
        let span = diag.span().expect("expected diagnostic span");
        assert_eq!(span.start, Position::new(1, 3));
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected expression after `,`, got ]"))
        );
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
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected expression after `,`, got `,`"))
        );
    }

    // ========================================================================
    // Regression tests for optional semicolons
    // ========================================================================
    // Rule: Semicolons are OPTIONAL for top-level statements on separate lines
    // Rule: Semicolons are REQUIRED for multiple statements on the same line
    // Rule: Semicolons are REQUIRED inside blocks (except last statement)
    // See: examples/basics/semicolons.flx for comprehensive examples
    // ========================================================================

    #[test]
    fn test_optional_semicolons_let_statements() {
        let program = parse("let x = 5\nlet y = 10");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(name, "x"),
            _ => panic!("expected Let statement"),
        }
        match &program.statements[1] {
            Statement::Let { name, .. } => assert_eq!(name, "y"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn test_optional_semicolons_expressions() {
        let program = parse("1 + 2\n3 + 4");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_optional_semicolons_function_calls() {
        let program = parse("print(\"Hello\")\nprint(\"World\")");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_mixed_semicolons() {
        // Mix of statements with and without semicolons
        let program = parse("let x = 5;\nlet y = 10\nprint(x)\nprint(y);");
        assert_eq!(program.statements.len(), 4);
    }

    #[test]
    fn test_optional_semicolons_return() {
        let program = parse("return 5\nreturn 10");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Return { .. } => {}
            _ => panic!("expected Return statement"),
        }
    }

    #[test]
    fn test_optional_semicolons_if_statements() {
        let program = parse("if (x > 0) { print(\"positive\") }\nprint(\"done\")");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_optional_semicolons_multiple_expressions() {
        let program = parse("1 + 2\n3 * 4\n5 - 6\n7 / 8");
        assert_eq!(program.statements.len(), 4);
    }
}
