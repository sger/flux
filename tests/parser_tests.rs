#[cfg(test)]
mod tests {
    use flux::frontend::{
        expression::Expression, lexer::Lexer, parser::Parser, position::Position, program::Program,
        statement::Statement,
    };

    const SEMICOLON_VERIFICATION_PROGRAM: &str = r#"fun f(a, b, c, d) {
    print(a);
    print(b);
    print(c);
    print(d);
}

print(1 2 3);         // missing commas between call arguments
[true false true];    // missing commas between array items
f(10, 20 30, 40);     // missing comma in the middle argument gap

let parsed_after_errors = "parser should still reach this statement";
print(parsed_after_errors);

print(1 2 3)          // also test without semicolon
let test = "this compiles"
let test2 = "this compiles";
"#;

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
    fn test_match_arms_comma_separated_is_valid() {
        let program = parse("match x { 0 -> 1, 1 -> 2 };");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_match_arms_trailing_comma_is_valid() {
        let program = parse(
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
    fn test_match_arms_semicolon_separator_reports_clear_diagnostic_and_recovers() {
        let lexer = Lexer::new("match x { 0 -> 1; 1 -> 2 }\nlet y = 3;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let sep_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| {
                d.message()
                    .is_some_and(|m| m.contains("Match arms must be separated by `,`, not `;`"))
            })
            .collect();
        assert_eq!(sep_diags.len(), 1, "expected one separator diagnostic");
        assert_eq!(
            parser
                .errors
                .iter()
                .filter(|d| d.code() == Some("E031"))
                .count(),
            0,
            "semicolon separator recovery should avoid EXPECTED_EXPRESSION cascades"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "y")),
            "expected parser to continue after invalid match separator"
        );
    }

    #[test]
    fn test_match_arms_mixed_separators_reports_semicolon_and_recovers() {
        let lexer = Lexer::new("match x { 0 -> 1, 1 -> 2; _ -> 3 }\nlet y = 3;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let sep_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| {
                d.message()
                    .is_some_and(|m| m.contains("Match arms must be separated by `,`, not `;`"))
            })
            .collect();
        assert_eq!(sep_diags.len(), 1, "expected one separator diagnostic");
        assert_eq!(
            parser
                .errors
                .iter()
                .filter(|d| d.code() == Some("E031"))
                .count(),
            0,
            "mixed separator recovery should avoid EXPECTED_EXPRESSION cascades"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "y")),
            "expected parser to continue after mixed match separators"
        );
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
    fn test_missing_commas_in_numeric_call_args_emit_e073_without_cascade() {
        let lexer = Lexer::new("print(1 2 3)");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 2,
            "expected one E073 per missing comma gap"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "missing-comma recovery should avoid generic cascade diagnostics"
        );
        assert_eq!(
            program.statements.len(),
            1,
            "top-level call without semicolon should still parse as one statement"
        );
    }

    #[test]
    fn test_missing_comma_in_call_args_reports_single_root_diagnostic_and_recovers() {
        let lexer = Lexer::new("f(\"a\" \"b\")\nlet x = 1;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let missing_comma: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .collect();
        assert_eq!(
            missing_comma.len(),
            1,
            "expected exactly one missing-comma diagnostic"
        );
        let span = missing_comma[0]
            .span()
            .expect("expected missing-comma span");
        assert_eq!(span.start, Position::new(1, 6));
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "missing comma should avoid generic cascade diagnostics"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "x")),
            "expected parser recovery to continue after call argument error"
        );
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

        assert_eq!(span.start.line, 1);
        assert_eq!(span.start.column, 0);
        assert_eq!(span.end.line, 1);
        assert_eq!(span.end.column, input.chars().count());
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
    fn test_unterminated_block_comment_error_uses_lexer_end_position() {
        let input = "let x = 1; /* unterminated";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E074"))
            .expect("expected E074 unterminated block comment diagnostic");
        let span = diag.span().expect("expected diagnostic span");
        let start_col = input.find("/*").expect("expected block comment start");

        assert_eq!(span.start.line, 1);
        assert_eq!(span.start.column, start_col);
        assert_eq!(span.end.line, 1);
        assert_eq!(span.end.column, input.chars().count());
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
    fn test_missing_commas_in_boolean_array_emit_e073_without_cascade() {
        let lexer = Lexer::new("[true false true]");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 2,
            "expected one E073 per missing comma gap"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "missing-comma recovery should avoid generic cascade diagnostics"
        );
    }

    #[test]
    fn test_missing_comma_in_array_reports_single_root_diagnostic_and_recovers() {
        let lexer = Lexer::new("[\"a\" \"b\"]\nlet x = 1;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let missing_comma: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .collect();
        assert_eq!(
            missing_comma.len(),
            1,
            "expected exactly one missing-comma diagnostic"
        );
        let span = missing_comma[0]
            .span()
            .expect("expected missing-comma span");
        assert_eq!(span.start, Position::new(1, 5));
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "missing comma should avoid generic cascade diagnostics"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "x")),
            "expected parser recovery to continue after array item error"
        );
    }

    #[test]
    fn test_index_expression() {
        let program = parse("arr[1 + 1];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "(arr[(1 + 1)])");
    }

    #[test]
    fn test_missing_comma_in_parenthesized_tuple_like_input_reports_single_diagnostic() {
        let lexer = Lexer::new("(\"a\" \"b\")\nlet x = 1;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let missing_comma: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .collect();
        assert_eq!(
            missing_comma.len(),
            1,
            "expected exactly one missing-comma diagnostic"
        );
        let span = missing_comma[0]
            .span()
            .expect("expected missing-comma span");
        assert_eq!(span.start, Position::new(1, 5));
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "tuple-like missing comma should avoid generic cascade diagnostics"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "x")),
            "expected parser recovery to continue after parenthesized error"
        );
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
    fn test_lambda_single_param_sugar_and_parenthesized_have_same_ast_shape() {
        let sugar = parse(r"\x -> x");
        let parenthesized = parse(r"\(x) -> x");

        let sugar_params = match &sugar.statements[0] {
            Statement::Expression {
                expression: Expression::Function { parameters, .. },
                ..
            } => parameters.clone(),
            _ => panic!("expected lambda function expression"),
        };
        let paren_params = match &parenthesized.statements[0] {
            Statement::Expression {
                expression: Expression::Function { parameters, .. },
                ..
            } => parameters.clone(),
            _ => panic!("expected lambda function expression"),
        };

        assert_eq!(sugar_params.len(), 1);
        assert_eq!(paren_params.len(), 1);
        assert_eq!(sugar_params, paren_params);
        assert_eq!(sugar_params[0], "x");
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
        assert!(diag
            .hints()
            .iter()
            .any(|h| h.text.contains(r"\x -> expr") || h.text.contains(r"\(x, y) -> expr")));
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
        assert!(diag
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter")));
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
        assert!(diag
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter")));
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

        assert!(single_diag
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter, got if")));
        assert_eq!(single_diag.message(), paren_diag.message());
        assert_eq!(single_parser.errors.len(), 1);
        assert_eq!(paren_parser.errors.len(), 1);
        assert!(
            single_parser
                .errors
                .iter()
                .all(|d| d.code() != Some("E036")),
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

        assert!(single_diag
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter, got INT")));
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
        assert!(diag
            .message()
            .is_some_and(|m| m.contains("Expected `,` or `)` after parameter")));

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["a".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
    }

    #[test]
    fn test_function_parameter_trailing_comma_is_accepted() {
        let lexer = Lexer::new("fun f(a,) { }");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        assert!(
            parser.errors.is_empty(),
            "expected no diagnostics for trailing comma in function parameters"
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
        assert!(e105_diags[0]
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter")));

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
        assert!(e105_diags[0]
            .message()
            .is_some_and(|m| m.contains("Expected identifier as parameter")));

        match &program.statements[0] {
            Statement::Function { parameters, .. } => {
                assert_eq!(parameters, &vec!["x".to_string()]);
            }
            _ => panic!("expected function statement"),
        }
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
            Statement::Expression {
                expression: Expression::Call { arguments, .. },
                ..
            } => {
                assert_eq!(arguments.len(), 1);
            }
            _ => panic!("expected call expression statement"),
        }
    }

    #[test]
    fn test_array_literal_trailing_comma_is_accepted() {
        let lexer = Lexer::new("[1,]");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        assert!(
            parser.errors.is_empty(),
            "expected no diagnostics for trailing comma in array literal"
        );
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::Array { elements, .. },
                ..
            } => {
                assert_eq!(elements.len(), 1);
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
        assert!(diag
            .message()
            .is_some_and(|m| m.contains("Expected expression after `,`, got `,`")));
    }

    #[test]
    fn test_hash_literal_trailing_comma_is_accepted() {
        let program = parse(r#"{"one": 1,}"#);
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.to_string(), "{\"one\": 1}");
    }

    #[test]
    fn test_nested_trailing_commas_are_accepted() {
        let program = parse("f([1,2,], g(3,4,),)");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::Call { arguments, .. },
                ..
            } => {
                assert_eq!(arguments.len(), 2);
                assert!(
                    matches!(&arguments[0], Expression::Array { elements, .. } if elements.len() == 2)
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

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 1,
            "expected one missing comma diagnostic"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "missing-comma recovery should avoid generic cascade diagnostics"
        );

        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::Call { arguments, .. },
                ..
            } => {
                assert_eq!(
                    arguments.len(),
                    4,
                    "expected trailing argument to be preserved"
                );
                assert!(
                    matches!(&arguments[3], Expression::Identifier { name, .. } if name == "d"),
                    "expected final argument `d` to be parsed after recovery"
                );
            }
            _ => panic!("expected call expression statement"),
        }
    }

    #[test]
    fn test_call_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
        let input = format!(
            "print({});\nlet ok = \"still parsed\";\nprint(ok);",
            spaced_ints(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let summary_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| {
                d.message()
                    .is_some_and(|m| m.contains("Too many errors in this list"))
            })
            .collect();
        assert_eq!(
            summary_diags.len(),
            1,
            "expected exactly one list error-limit summary diagnostic"
        );

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 50,
            "expected missing-comma diagnostics to stop at the list limit"
        );
        assert!(
            parser.errors.len() <= 51,
            "expected list diagnostics to be capped at 50 plus one summary"
        );

        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "ok")),
            "expected parser to continue and parse trailing let statement"
        );
        assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if name == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if name == "ok")
                )
            }),
            "expected parser to continue and parse trailing print(ok) call"
        );
    }

    #[test]
    fn test_array_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
        let input = format!(
            "[{}];\nlet ok = \"still parsed\";\nprint(ok);",
            spaced_ints(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let summary_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| {
                d.message()
                    .is_some_and(|m| m.contains("Too many errors in this list"))
            })
            .collect();
        assert_eq!(
            summary_diags.len(),
            1,
            "expected exactly one list error-limit summary diagnostic"
        );

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 50,
            "expected missing-comma diagnostics to stop at the list limit"
        );
        assert!(
            parser.errors.len() <= 51,
            "expected list diagnostics to be capped at 50 plus one summary"
        );

        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "ok")),
            "expected parser to continue and parse trailing let statement"
        );
        assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if name == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if name == "ok")
                )
            }),
            "expected parser to continue and parse trailing print(ok) call"
        );
    }

    #[test]
    fn test_match_arm_list_error_limit_caps_diagnostics_and_still_parses_following_statements() {
        let input = format!(
            "match x {{ {} }}\nlet ok = 1;",
            semicolon_separated_match_arms(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let summary_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| {
                d.message()
                    .is_some_and(|m| m.contains("Too many errors in this match arm list"))
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
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "ok")),
            "expected parser to continue and parse trailing let statement after match recovery"
        );
    }

    // ========================================================================
    // Regression tests for optional semicolons
    // ========================================================================
    // Current parser behavior:
    // - Semicolons are optional for top-level statements.
    // - Statements are separated by parser progress; semicolons are accepted but
    //   not required for the cases covered below.
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
    fn test_top_level_call_with_and_without_semicolon_both_parse() {
        let program = parse("print(\"hi\")\nprint(\"there\");");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_top_level_let_with_and_without_semicolon_both_parse() {
        let program = parse("let test = \"this compiles\"\nlet test2 = \"this compiles\";");
        assert_eq!(program.statements.len(), 2);
        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(name, "test"),
            _ => panic!("expected Let statement"),
        }
        match &program.statements[1] {
            Statement::Let { name, .. } => assert_eq!(name, "test2"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn test_semicolon_verification_program_recovers_and_parses_late_statements() {
        let lexer = Lexer::new(SEMICOLON_VERIFICATION_PROGRAM);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        let missing_comma_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E073"))
            .count();
        assert_eq!(
            missing_comma_count, 7,
            "expected missing-comma diagnostics at all intended gaps"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() == Some("E073")),
            "verification program should avoid generic cascade diagnostics"
        );
        assert!(
            !parser.errors.iter().any(|d| {
                d.message()
                    .is_some_and(|m| m.to_ascii_lowercase().contains("semicolon"))
            }),
            "expected no missing-semicolon diagnostics"
        );

        assert!(
            program.statements.iter().any(
                |stmt| matches!(stmt, Statement::Let { name, .. } if name == "parsed_after_errors")
            ),
            "expected recovery to keep parsing `parsed_after_errors` binding"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "test")),
            "expected recovery to keep parsing `test` binding without semicolon"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if name == "test2")),
            "expected recovery to keep parsing `test2` binding with semicolon"
        );
        assert!(
            program.statements.iter().any(|stmt| {
                matches!(
                    stmt,
                    Statement::Expression {
                        expression: Expression::Call { function, arguments, .. },
                        ..
                    } if matches!(&**function, Expression::Identifier { name, .. } if name == "print")
                        && arguments.len() == 1
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if name == "parsed_after_errors")
                )
            }),
            "expected recovery to keep parsing `print(parsed_after_errors)` call"
        );
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
