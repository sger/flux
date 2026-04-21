#[cfg(test)]
mod tests {
    use flux::diagnostics::position::Position;
    use flux::syntax::{
        expression::{Expression, Pattern},
        interner::Interner,
        lexer::Lexer,
        parser::Parser,
        program::Program,
        statement::Statement,
    };

    const SEMICOLON_VERIFICATION_PROGRAM: &str = r#"fn f(a, b, c, d) {
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
            Statement::Let {
                value: Expression::DoBlock { .. },
                ..
            } => {}
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
            Statement::Expression {
                expression: Expression::Match { arms, .. },
                ..
            } => {
                assert_eq!(arms.len(), 2);
                assert!(
                    matches!(arms[0].guard.as_ref(), Some(Expression::Infix { .. })),
                    "expected first arm guard to parse as expression"
                );
                assert!(
                    arms[1].guard.is_none(),
                    "expected second arm to be unguarded"
                );
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
            Statement::Function {
                name, parameters, ..
            } => {
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

    #[test]
    fn test_binding_shadowing_sample_program_parses_without_semicolons_and_keeps_ast_shape() {
        let (program, interner) = parse(
            r#"
let x = 3

fn t(x) {
    let x = x;
}
"#,
        );

        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "x"),
            _ => panic!("expected top-level let statement"),
        }

        match &program.statements[1] {
            Statement::Function {
                name,
                parameters,
                body,
                ..
            } => {
                assert_eq!(interner.resolve(*name), "t");
                let param_names: Vec<_> = parameters.iter().map(|p| interner.resolve(*p)).collect();
                assert_eq!(param_names, vec!["x"]);
                assert_eq!(body.statements.len(), 1);
                match &body.statements[0] {
                    Statement::Let { name, value, .. } => {
                        assert_eq!(interner.resolve(*name), "x");
                        assert!(
                            matches!(value, Expression::Identifier { name, .. } if interner.resolve(*name) == "x"),
                            "expected RHS of inner let to parse as identifier `x`"
                        );
                    }
                    _ => panic!("expected inner let statement"),
                }
            }
            _ => panic!("expected function statement"),
        }
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
        let interner = parser.take_interner();

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
            program.statements.iter().any(
                |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "x")
            ),
            "expected parser recovery to continue after call argument error"
        );
    }

    #[test]
    fn test_string_literal() {
        let (program, _interner) = parse(r#""hello world";"#);
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
    fn t1_named_delimiter_diagnostics_are_contextual() {
        let cases = [
            (
                "let h = {one: 1, two: 2",
                "Expected `}` to close hash literal.",
            ),
            ("let xs = [|1, 2|", "Expected `]` to close array literal."),
            (
                "let add = \\(x, y -> x + y",
                "Expected `)` to close lambda parameter list.",
            ),
            (
                "let msg = \"Hello #{name\"",
                "Expected `}` to close string interpolation.",
            ),
            (
                "let ys = [x + 1 | x <- [1, 2]",
                "Expected `]` to close list comprehension.",
            ),
        ];

        for (input, expected_msg) in cases {
            let lexer = Lexer::new(input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            assert!(
                parser.errors.iter().any(|d| d.code() == Some("E034")
                    && d.message().is_some_and(|m| m.contains(expected_msg))),
                "expected contextual E034 `{expected_msg}` for input `{input}`, got: {:?}",
                parser.errors
            );
        }
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
        let (program, interner) = parse(r#""Hello #{name}""#);
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.display_with(&interner), "\"Hello #{name}\"");
    }

    #[test]
    fn test_string_interpolation_expression() {
        let (program, interner) = parse(r#""Sum #{1 + 2}""#);
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.display_with(&interner), "\"Sum #{(1 + 2)}\"");
    }

    #[test]
    fn test_string_interpolation_nested_hash_expression_terminates() {
        let (program, _interner) = parse("\"#{ {1: 2} }\"");
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_boolean_literals() {
        let (program, _interner) = parse("true; false;");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_array_literal() {
        let (program, interner) = parse("[1, 2 * 2, 3 + 3];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.display_with(&interner), "[1, (2 * 2), (3 + 3)];");
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
        let interner = parser.take_interner();

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
            program.statements.iter().any(
                |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "x")
            ),
            "expected parser recovery to continue after array item error"
        );
    }

    #[test]
    fn test_index_expression() {
        let (program, interner) = parse("arr[1 + 1];");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.display_with(&interner), "(arr[(1 + 1)]);");
    }

    #[test]
    fn test_parenthesized_missing_comma_reports_error_and_recovers() {
        let lexer = Lexer::new("(\"a\" \"b\")\nlet x = 1;");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        assert!(
            !parser.errors.is_empty(),
            "expected a parse error for missing comma in tuple-like syntax"
        );
        assert!(
            parser.errors.iter().any(|d| d.code() == Some("E034")),
            "expected an E034 unexpected-token diagnostic, got: {:?}",
            parser.errors
        );
        let _ = (program, interner);
    }

    #[test]
    fn test_missing_rparen_in_call_newline_does_not_report_missing_comma() {
        let lexer = Lexer::new("print(point.0\nprint(point.1)");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(
            parser
                .errors
                .iter()
                .any(|d| d.code() == Some("E076") || d.code() == Some("E034")),
            "expected an unclosed-delimiter or unexpected-token diagnostic for missing `)`"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() != Some("E073")),
            "missing `)` should not be reported as missing comma: {:?}",
            parser.errors
        );
    }

    #[test]
    fn test_dangling_member_access_reports_once_without_cascade() {
        let lexer = Lexer::new("print(point.)\nprint(point.1)");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(
            parser.errors.iter().any(|d| d.code() == Some("E034")),
            "expected member-access parse error"
        );
        assert!(
            parser.errors.iter().all(|d| d.code() != Some("E031")),
            "dangling member access should not cascade to expected-expression errors: {:?}",
            parser.errors
        );
    }

    #[test]
    fn test_dangling_member_access_across_newline_reports_at_dot() {
        let lexer = Lexer::new("print(point.\nprint(point.1))");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        let first = parser.errors.first().expect("expected parse error");
        assert_eq!(first.code(), Some("E034"));
        assert!(
            first
                .message()
                .is_some_and(|m| m.contains("identifier or tuple field index after `.`")),
            "unexpected message: {:?}",
            first.message()
        );
    }

    #[test]
    fn test_dangling_member_access_across_newline_avoids_duplicate_delimiter_error() {
        let lexer = Lexer::new("print(point.\nprint(point.1)\nlet x = 1");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        let e034_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .count();
        assert_eq!(
            e034_count, 1,
            "expected only one E034 for dangling member access"
        );
    }

    #[test]
    fn test_missing_tuple_rparen_before_new_statement_does_not_cascade() {
        let lexer = Lexer::new("let point = (1, 2, 3\nlet single = (42,)\nprint(single)");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        assert!(
            parser
                .errors
                .iter()
                .any(|d| (d.code() == Some("E076") || d.code() == Some("E034"))
                    && d.message()
                        .is_some_and(|m| m.contains(")") || m.contains("closing"))),
            "expected missing `)` diagnostic: {:?}",
            parser.errors
        );
        assert!(
            parser.errors.iter().all(|d| d.code() != Some("E031")),
            "missing tuple `)` should not cascade to expected-expression errors: {:?}",
            parser.errors
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "single")),
            "expected parser recovery to continue with next statement"
        );
    }

    #[test]
    fn test_missing_grouped_delimiter_error_count_stays_bounded() {
        let lexer = Lexer::new("let point = (1, 2, 3\nlet single = (42,)\nlet unit = ()");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(
            parser.errors.len() <= 3,
            "expected bounded cascade (<= 3 errors), got: {:?}",
            parser.errors
        );
        let first = parser
            .errors
            .first()
            .expect("expected at least one parse error");
        assert!(
            first.code() == Some("E076") || first.code() == Some("E034"),
            "expected UNCLOSED_DELIMITER or UNEXPECTED_TOKEN, got: {:?}",
            first.code()
        );
        assert!(
            first
                .message()
                .is_some_and(|m| m.contains(")") || m.contains("closing")),
            "expected first diagnostic to be about missing `)`, got: {:?}",
            first.message()
        );
    }

    #[test]
    fn test_dangling_member_access_error_count_stays_bounded() {
        let lexer = Lexer::new("print(point.\nprint(point.1)");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(
            parser.errors.len() <= 3,
            "expected bounded cascade (<= 3 errors), got: {:?}",
            parser.errors
        );
        let first = parser
            .errors
            .first()
            .expect("expected at least one parse error");
        assert_eq!(first.code(), Some("E034"));
        assert!(
            first
                .message()
                .is_some_and(|m| m.contains("identifier or tuple field index after `.`")),
            "expected first diagnostic to be about incomplete member access, got: {:?}",
            first.message()
        );
    }

    #[test]
    fn test_malformed_signature_missing_tuple_close_suppresses_redundant_parameter_followups() {
        let lexer = Lexer::new(include_str!(
            "../../examples/type_system/failing/184_type_expr_missing_close_paren.flx"
        ));
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        let root_missing_tuple_close = parser.errors.iter().any(|d| {
            d.code() == Some("E034")
                && d.message().is_some_and(|m| {
                    m.contains("close tuple type") || m.contains("Tuple types use")
                })
        });
        assert!(
            root_missing_tuple_close,
            "expected a root tuple-close diagnostic: {:?}",
            parser.errors
        );

        let redundant_param_followup = parser.errors.iter().any(|d| {
            d.code() == Some("E034")
                && d.message()
                    .is_some_and(|m| m.contains("after function parameter"))
        });
        assert!(
            !redundant_param_followup,
            "expected parameter-separator followup to be suppressed: {:?}",
            parser.errors
        );
        assert!(
            parser.errors.len() <= 2,
            "expected root + minimal follow-up (<=2 errors), got: {:?}",
            parser.errors
        );
    }

    #[test]
    fn test_hash_literal() {
        let (program, _interner) = parse(r#"{"one": 1, "two": 2};"#);
        assert_eq!(program.statements.len(), 1);
    }

    #[test]
    fn test_empty_hash() {
        let (program, interner) = parse("{};");
        assert_eq!(program.statements.len(), 1);
        assert_eq!(program.display_with(&interner), "{};");
    }

    // Lambda shorthand tests
    #[test]
    fn test_lambda_single_param() {
        let (program, _interner) = parse(r"\x -> x * 2;");
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
        let (program, interner) = parse(r"\x -> x");
        assert_eq!(program.statements.len(), 1);

        match &program.statements[0] {
            Statement::Expression {
                expression:
                    Expression::Function {
                        parameters, span, ..
                    },
                ..
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
        let sugar_names: Vec<_> = sugar_params
            .iter()
            .map(|p| sugar_interner.resolve(*p))
            .collect();
        let paren_names: Vec<_> = paren_params
            .iter()
            .map(|p| paren_interner.resolve(*p))
            .collect();
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

        let e034_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .count();
        assert!(
            e034_count <= 1,
            "expected no flood of unrelated E034 diagnostics, got: {:?}",
            parser.errors
        );
    }

    #[test]
    fn test_let_in_if_and_match_expression_positions_reports_targeted_diagnostic() {
        let inputs = [
            "if let x = 1 { 1 } else { 0 }",
            "match 1 { 1 -> let y = 2, _ -> 0 }",
        ];

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

        assert_eq!(
            parser.errors.len(),
            1,
            "expected exactly one diagnostic for missing arrow"
        );
        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected E034 diagnostic for missing lambda arrow");
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("lambda parameters"))
        );
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
            diag.message()
                .is_some_and(|m| m.contains("match arm") && m.contains("`->`")),
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
            diag.message()
                .is_some_and(|m| m.contains("outside `match`")),
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
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("parameter name here"))
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
                .is_some_and(|m| m.contains("parameter name here"))
        );
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
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("after function parameter"))
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

        let e105_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .collect();
        assert_eq!(e105_diags.len(), 1, "expected one E034 for extra comma");
        assert!(
            e105_diags[0]
                .message()
                .is_some_and(|m| m.contains("parameter name here"))
        );

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

        let e105_diags: Vec<_> = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E034"))
            .collect();
        assert_eq!(e105_diags.len(), 1, "expected one E034 for leading comma");
        assert!(
            e105_diags[0]
                .message()
                .is_some_and(|m| m.contains("parameter name here"))
        );

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
            Statement::Let {
                name,
                type_annotation,
                value,
                ..
            } => {
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
            Statement::Function {
                parameters,
                parameter_types,
                ..
            } => {
                assert_eq!(parameters.len(), 2);
                assert_eq!(parameter_types.len(), 2);
                assert!(
                    parameter_types[0].is_none(),
                    "malformed annotation should become None"
                );
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
            Statement::Function {
                return_type, body, ..
            } => {
                assert!(
                    return_type.is_none(),
                    "malformed return annotation should be dropped"
                );
                assert!(
                    !body.statements.is_empty(),
                    "function body should still be parsed after recovery"
                );
            }
            _ => panic!("expected function statement"),
        }

        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")),
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
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")),
            "expected follow-up let statement after malformed lambda annotation"
        );
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
        let lexer = Lexer::new("#[1,]");
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();

        assert!(
            parser.errors.is_empty(),
            "expected no diagnostics for trailing comma in array literal"
        );
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::ArrayLiteral { elements, .. },
                ..
            } => {
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
            Statement::Expression {
                expression: Expression::ArrayLiteral { elements, .. },
                ..
            } => {
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

        assert!(
            parser.errors.is_empty(),
            "expected no diagnostics for empty [||] array literal"
        );
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::ArrayLiteral { elements, .. },
                ..
            } => {
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
        assert!(
            diag.message()
                .is_some_and(|m| m.contains("Expected expression after `,`, got `,`"))
        );
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
            Statement::Expression {
                expression: Expression::Call { arguments, .. },
                ..
            } => {
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
            Statement::Expression {
                expression: Expression::Call { arguments, .. },
                ..
            } => {
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
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")),
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
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")),
            "expected parser recovery to continue after malformed assignment statement"
        );
    }

    #[test]
    fn test_tuple_literal_parsing() {
        let (program, _interner) = parse("(1, 2, 3);");
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::TupleLiteral { elements, .. },
                ..
            } => assert_eq!(elements.len(), 3),
            _ => panic!("expected tuple literal"),
        }
    }

    #[test]
    fn test_singleton_and_unit_tuple_parsing() {
        let (program, _interner) = parse("(42,); ()");
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::TupleLiteral { elements, .. },
                ..
            } => assert_eq!(elements.len(), 1),
            _ => panic!("expected singleton tuple"),
        }
        match &program.statements[1] {
            Statement::Expression {
                expression: Expression::TupleLiteral { elements, .. },
                ..
            } => assert!(elements.is_empty()),
            _ => panic!("expected unit tuple"),
        }
    }

    #[test]
    fn test_parenthesized_grouping_stays_grouping() {
        let (program, _interner) = parse("(1 + 2);");
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::Infix { .. },
                ..
            } => {}
            _ => panic!("expected grouped infix expression"),
        }
    }

    #[test]
    fn test_tuple_field_access_parsing() {
        let (program, _interner) = parse("t.0;");
        match &program.statements[0] {
            Statement::Expression {
                expression: Expression::TupleFieldAccess { index, .. },
                ..
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
        let input = format!(
            "print({});\nlet ok = \"still parsed\";\nprint(ok);",
            spaced_ints(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

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
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")),
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
        let input = format!(
            "[{}];\nlet ok = \"still parsed\";\nprint(ok);",
            spaced_ints(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

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
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")),
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
        let input = format!(
            "match x {{ {} }}\nlet ok = 1;",
            semicolon_separated_match_arms(200)
        );
        let lexer = Lexer::new(&input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

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
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")),
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
        let (program, interner) = parse("let x = 5\nlet y = 10");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "x"),
            _ => panic!("expected Let statement"),
        }
        match &program.statements[1] {
            Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "y"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn test_optional_semicolons_expressions() {
        let (program, _interner) = parse("1 + 2\n3 + 4");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_optional_semicolons_function_calls() {
        let (program, _interner) = parse("print(\"Hello\")\nprint(\"World\")");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_top_level_call_with_and_without_semicolon_both_parse() {
        let (program, _interner) = parse("print(\"hi\")\nprint(\"there\");");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_top_level_let_with_and_without_semicolon_both_parse() {
        let (program, interner) =
            parse("let test = \"this compiles\"\nlet test2 = \"this compiles\";");
        assert_eq!(program.statements.len(), 2);
        match &program.statements[0] {
            Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "test"),
            _ => panic!("expected Let statement"),
        }
        match &program.statements[1] {
            Statement::Let { name, .. } => assert_eq!(interner.resolve(*name), "test2"),
            _ => panic!("expected Let statement"),
        }
    }

    #[test]
    fn test_semicolon_verification_program_recovers_and_parses_late_statements() {
        let lexer = Lexer::new(SEMICOLON_VERIFICATION_PROGRAM);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

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
                |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "parsed_after_errors")
            ),
            "expected recovery to keep parsing `parsed_after_errors` binding"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "test")),
            "expected recovery to keep parsing `test` binding without semicolon"
        );
        assert!(
            program
                .statements
                .iter()
                .any(|stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "test2")),
            "expected recovery to keep parsing `test2` binding with semicolon"
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
                        && matches!(&arguments[0], Expression::Identifier { name, .. } if interner.resolve(*name) == "parsed_after_errors")
                )
            }),
            "expected recovery to keep parsing `print(parsed_after_errors)` call"
        );
    }

    #[test]
    fn test_mixed_semicolons() {
        // Mix of statements with and without semicolons
        let (program, _interner) = parse("let x = 5;\nlet y = 10\nprint(x)\nprint(y);");
        assert_eq!(program.statements.len(), 4);
    }

    #[test]
    fn test_optional_semicolons_return() {
        let (program, _interner) = parse("return 5\nreturn 10");
        assert_eq!(program.statements.len(), 2);

        match &program.statements[0] {
            Statement::Return { .. } => {}
            _ => panic!("expected Return statement"),
        }
    }

    #[test]
    fn test_optional_semicolons_if_statements() {
        let (program, _interner) = parse("if (x > 0) { print(\"positive\") }\nprint(\"done\")");
        assert_eq!(program.statements.len(), 2);
    }

    #[test]
    fn test_optional_semicolons_multiple_expressions() {
        let (program, _interner) = parse("1 + 2\n3 * 4\n5 - 6\n7 / 8");
        assert_eq!(program.statements.len(), 4);
    }

    #[test]
    fn missing_colon_let_annotation_has_targeted_message_and_recovers() {
        let input = "let x Int = 1;\nlet y = 2;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        assert!(parser.errors.iter().any(|d| {
            d.message().is_some_and(|m| {
                m.contains("Missing `:` in let binding type annotation") && m.contains("`x: Type`")
            })
        }));
        assert!(program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
        ));
    }

    #[test]
    fn missing_colon_function_param_has_targeted_message() {
        let input = "fn f(x Int) -> Int { x }";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(parser.errors.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("Missing `:` in function parameter type annotation"))
        }));
    }

    #[test]
    fn missing_colon_lambda_param_has_targeted_message() {
        let input = "let g = \\x Int -> x;\nlet after = 1;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        assert!(parser.errors.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("Missing `:` in lambda parameter type annotation"))
        }));
        assert!(program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "after")
        ));
    }

    #[test]
    fn missing_colon_effect_op_has_targeted_message() {
        let input = "effect Console { print String -> Unit }";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        assert!(parser.errors.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("Missing `:` in effect operation signature"))
        }));
    }

    #[test]
    fn missing_function_return_arrow_has_targeted_message() {
        let input = "fn f() Int { 1 }\nlet ok = 1;";
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        assert!(parser.errors.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("Missing `->` before function return type"))
        }));
        assert!(program.statements.iter().any(
            |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "ok")
        ));
    }

    #[test]
    fn keyword_aliases_report_e030_with_targeted_suggestions() {
        let collect = |input: &str| -> Vec<String> {
            let lexer = Lexer::new(input);
            let mut parser = Parser::new(lexer);
            let _program = parser.parse_program();
            parser
                .errors
                .iter()
                .filter_map(|d| d.message().map(ToString::to_string))
                .collect()
        };

        let def_msgs = collect("def foo() { 1 }");

        assert!(
            def_msgs.iter().any(|m| m.contains("Unknown keyword `def`")),
            "expected `def` alias diagnostic, got: {:#?}",
            def_msgs
        );

        let var_msgs = collect("var x = 1");
        assert!(
            var_msgs.iter().any(|m| m.contains("Unknown keyword `var`")),
            "expected `var` alias diagnostic, got: {:#?}",
            var_msgs
        );

        let case_msgs = collect("case 1 { 0 -> 0, _ -> 1 }");
        assert!(
            case_msgs
                .iter()
                .any(|m| m.contains("Unknown keyword `case`")),
            "expected `case` alias diagnostic, got: {:#?}",
            case_msgs
        );
    }

    #[test]
    fn elif_and_end_report_targeted_diagnostics() {
        let lexer = Lexer::new("if true { 1 } elif false { 2 } else { 3 }\nfn f() { 1 end }");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        let msgs: Vec<String> = parser
            .errors
            .iter()
            .filter_map(|d| d.message().map(ToString::to_string))
            .collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("Unknown keyword `elif`")
                    || m.contains("Unknown keyword `elsif`")),
            "expected elif/elsif alias diagnostic, got: {:#?}",
            msgs
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("`end` is not a keyword in Flux")),
            "expected end-keyword diagnostic, got: {:#?}",
            msgs
        );
    }

    #[test]
    fn structural_context_messages_are_emitted() {
        let collect = |input: &str| -> Vec<String> {
            let lexer = Lexer::new(input);
            let mut parser = Parser::new(lexer);
            let _program = parser.parse_program();
            parser
                .errors
                .iter()
                .filter_map(|d| d.message().map(ToString::to_string))
                .collect()
        };

        let fn_msgs = collect("fn foo -> Int { 1 }");
        assert!(
            fn_msgs
                .iter()
                .any(|m| m.contains("parameter list after `foo`")),
            "expected missing-parameter-list message, got: {:#?}",
            fn_msgs
        );

        let if_msgs = collect("if true 1 else 2");
        assert!(
            if_msgs
                .iter()
                .any(|m| m.contains("`if` branch needs to start with `{`")),
            "expected missing-if-brace message, got: {:#?}",
            if_msgs
        );

        let let_msgs = collect("let x 1");
        assert!(
            let_msgs
                .iter()
                .any(|m| m.contains("Expected `=` after `let x`")),
            "expected missing-let-assign message, got: {:#?}",
            let_msgs
        );
    }

    #[test]
    fn match_pipe_and_fat_arrow_emit_targeted_messages() {
        let lexer = Lexer::new(
            "match x { 0 -> 1 | 1 -> 2 }\nlet y = 3;\nmatch z { 0 => 1, _ -> 2 }\nlet q = 4;",
        );
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();

        let msgs: Vec<String> = parser
            .errors
            .iter()
            .filter_map(|d| d.message().map(ToString::to_string))
            .collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("Match arms are separated by `,` in Flux, not `|`")),
            "expected `|` separator diagnostic, got: {:#?}",
            msgs
        );
        assert!(
            msgs.iter().any(|m| m.contains("needs `->`, not `=>`")),
            "expected fat-arrow diagnostic, got: {:#?}",
            msgs
        );
        assert!(
            program.statements.iter().any(
                |stmt| matches!(stmt, Statement::Let { name, .. } if interner.resolve(*name) == "y")
            ),
            "expected parser recovery after `|` separator diagnostic"
        );
    }

    #[test]
    fn alias_words_as_identifiers_do_not_misfire_at_expression_positions() {
        let lexer = Lexer::new("let val = 1;\nlet end = 2;\nlet x = Foo.end(val);");
        let mut parser = Parser::new(lexer);
        let _program = parser.parse_program();

        let alias_diag_count = parser
            .errors
            .iter()
            .filter(|d| d.code() == Some("E030"))
            .count();
        assert_eq!(
            alias_diag_count, 0,
            "did not expect statement-keyword alias diagnostics for identifier usage"
        );
    }

    #[test]
    fn t15_remaining_expect_peek_sites_emit_contextual_e034_messages() {
        struct Case {
            name: &'static str,
            input: &'static str,
            message_fragment: &'static str,
        }

        let cases = [
            Case {
                name: "perform_missing_dot",
                input: "let x = perform IO print(\"hi\")",
                message_fragment: "perform",
            },
            Case {
                name: "handle_missing_lbrace",
                input: "let x = 1 handle IO print(resume, msg) -> resume(msg)",
                message_fragment: "handle",
            },
            Case {
                name: "match_missing_open_brace",
                input: "let x = match 1 0 -> 0",
                message_fragment: "match",
            },
            Case {
                name: "module_missing_lbrace",
                input: "module Demo\nlet x = 1",
                message_fragment: "module",
            },
            Case {
                name: "type_sugar_missing_assign",
                input: "type Maybe<T> Some(T) | None",
                message_fragment: "type",
            },
            Case {
                name: "hash_missing_colon",
                input: "let h = { \"a\" 1 }",
                message_fragment: "hash",
            },
            Case {
                name: "list_comprehension_missing_generator_ident",
                input: "let xs = [x | <- [1, 2, 3]]",
                message_fragment: "comprehension",
            },
            Case {
                name: "type_expr_missing_rparen",
                input: "fn bad(x: (Int, String -> Int { x }",
                message_fragment: "type",
            },
        ];

        for case in cases {
            let lexer = Lexer::new(case.input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            let diag = parser
                .errors
                .iter()
                .find(|d| d.code() == Some("E034"))
                .unwrap_or_else(|| panic!("expected E034 for case `{}`", case.name));

            let msg = diag
                .message()
                .unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
            assert!(
                msg.to_lowercase().contains(case.message_fragment),
                "expected message to mention `{}` for case `{}`, got: {}",
                case.message_fragment,
                case.name,
                msg
            );
            assert!(
                !diag.hints().is_empty(),
                "expected help hint for case `{}`",
                case.name
            );
        }
    }

    #[test]
    fn t16_contextual_recovery_fixtures_emit_e034_messages() {
        struct Case {
            name: &'static str,
            input: &'static str,
            message_fragment: &'static str,
        }

        let cases = [
            Case {
                name: "perform_missing_dot_fixture",
                input: include_str!("../fixtures/recovery/t16_perform_missing_dot_contextual.flx"),
                message_fragment: "perform",
            },
            Case {
                name: "handle_missing_lbrace_fixture",
                input: include_str!(
                    "../fixtures/recovery/t16_handle_missing_lbrace_contextual.flx"
                ),
                message_fragment: "handle",
            },
            Case {
                name: "module_missing_lbrace_fixture",
                input: include_str!(
                    "../fixtures/recovery/t16_module_missing_lbrace_contextual.flx"
                ),
                message_fragment: "module",
            },
        ];

        for case in cases {
            let lexer = Lexer::new(case.input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            let diag = parser
                .errors
                .iter()
                .find(|d| d.code() == Some("E034"))
                .unwrap_or_else(|| panic!("expected E034 for case `{}`", case.name));

            let msg = diag
                .message()
                .unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
            assert!(
                msg.to_lowercase().contains(case.message_fragment),
                "expected contextual message to mention `{}` for case `{}`, got: {}",
                case.message_fragment,
                case.name,
                msg
            );
        }
    }

    #[test]
    fn t17_contextual_e034_regression_guard_exact_messages() {
        struct Case {
            name: &'static str,
            input: &'static str,
            expected_message: &'static str,
            expected_hint: &'static str,
        }

        let cases = [
            Case {
                name: "perform_missing_dot",
                input: include_str!(
                    "../../examples/type_system/failing/173_perform_missing_dot.flx"
                ),
                expected_message: "This `perform` expression needs `.` between the effect and operation.",
                expected_hint: "Perform expressions use `perform Effect.op(args...)`.",
            },
            Case {
                name: "handle_missing_lbrace",
                input: include_str!(
                    "../../examples/type_system/failing/174_handle_missing_lbrace.flx"
                ),
                expected_message: "Expected `{` to begin `handle` arms.",
                expected_hint: "Handle expressions use `expr handle Effect { ... }`.",
            },
            Case {
                name: "handle_arm_missing_arrow",
                input: include_str!(
                    "../../examples/type_system/failing/175_handle_arm_missing_arrow.flx"
                ),
                expected_message: "Expected `->` in handle arm.",
                expected_hint: "Handle arms use `op(resume, arg1, ...) -> body`.",
            },
            Case {
                name: "module_missing_lbrace",
                input: include_str!(
                    "../fixtures/recovery/t16_module_missing_lbrace_contextual.flx"
                ),
                expected_message: "This module body needs to start with `{`.",
                expected_hint: "Module declarations use `module Name { ... }`.",
            },
        ];

        for case in cases {
            let lexer = Lexer::new(case.input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            let diag = parser
                .errors
                .iter()
                .find(|d| d.code() == Some("E034") && d.message() == Some(case.expected_message))
                .unwrap_or_else(|| panic!("expected exact E034 message for `{}`", case.name));

            let msg = diag
                .message()
                .unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
            assert_eq!(
                msg, case.expected_message,
                "unexpected E034 message for case `{}`",
                case.name
            );
            assert_ne!(
                msg.trim(),
                "Unexpected token.",
                "regressed to generic E034 wording for case `{}`",
                case.name
            );

            let first_hint = diag
                .hints()
                .first()
                .map(|h| h.text.as_str())
                .unwrap_or_else(|| panic!("expected hint for case `{}`", case.name));
            assert_eq!(
                first_hint, case.expected_hint,
                "unexpected E034 hint for case `{}`",
                case.name
            );
        }
    }

    #[test]
    fn parser_torture_contextual_titles_and_no_token_jargon() {
        let cases = [
            (
                include_str!("../../examples/parser_errors/malformed_data_constructor_fields.flx"),
                "constructor",
            ),
            (
                include_str!("../../examples/parser_errors/malformed_type_variant_fields.flx"),
                "constructor",
            ),
            (
                include_str!("../../examples/parser_errors/malformed_effect_op_list.flx"),
                "effect",
            ),
            (
                include_str!("../../examples/parser_errors/eof_after_perform_dot.flx"),
                "perform",
            ),
        ];

        for (input, fragment) in cases {
            let lexer = Lexer::new(input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            assert!(
                parser
                    .errors
                    .iter()
                    .any(|d| d.code() == Some("E034") || d.code() == Some("E076")),
                "expected structural parser diagnostic, got: {:?}",
                parser.errors
            );
            assert!(
                parser
                    .errors
                    .iter()
                    .filter(|d| d.code() == Some("E034"))
                    .all(|d| d
                        .display_title()
                        .is_some_and(|title| title != "Unexpected Token")),
                "expected contextual E034 display titles, got: {:?}",
                parser.errors
            );
            for diag in &parser.errors {
                if let Some(message) = diag.message() {
                    assert!(
                        !message.contains(" STRING")
                            && !message.contains(" INT")
                            && !message.contains(" IDENT")
                            && !message.contains(" EOF"),
                        "unexpected token-enum jargon in message: {message}"
                    );
                }
            }
            assert!(
                parser
                    .errors
                    .iter()
                    .filter_map(|d| d.message())
                    .any(|m| m.to_lowercase().contains(fragment)),
                "expected contextual message mentioning `{fragment}`, got: {:?}",
                parser.errors
            );
        }
    }

    #[test]
    fn eof_truncation_diagnostics_anchor_to_real_source_tokens() {
        let cases = [
            (
                include_str!("../../examples/parser_errors/eof_after_if_condition.flx"),
                "Missing If Body",
                2usize,
                "this `if` branch starts here",
            ),
            (
                include_str!("../../examples/parser_errors/eof_after_else.flx"),
                "Missing Else Body",
                2usize,
                "`else` starts here",
            ),
            (
                include_str!("../../examples/parser_errors/eof_after_match_open.flx"),
                "Missing Match Body",
                2usize,
                "this match expression starts here",
            ),
            (
                include_str!("../../examples/parser_errors/eof_after_lambda_params.flx"),
                "Missing Lambda Arrow",
                2usize,
                "this lambda parameter list ends here",
            ),
            (
                include_str!("../../examples/parser_errors/eof_after_perform_dot.flx"),
                "Missing Effect Operation Name",
                2usize,
                "this `perform` expression ends here",
            ),
        ];

        for (input, expected_title, expected_line, expected_label) in cases {
            let lexer = Lexer::new(input);
            let mut parser = Parser::new(lexer);
            let _ = parser.parse_program();

            let diag = parser
                .errors
                .first()
                .expect("expected eof-truncation diagnostic");
            assert_eq!(diag.code(), Some("E034"));
            assert_eq!(diag.display_title(), Some(expected_title));

            let span = diag.span().expect("expected eof-truncation span");
            assert_eq!(span.start.line, expected_line);
            assert!(
                diag.labels()
                    .iter()
                    .any(|label| label.text == expected_label),
                "expected origin label `{expected_label}`, got: {:?}",
                diag.labels()
            );
        }
    }

    #[test]
    fn parser_contextual_diagnostics_include_breadcrumb_notes() {
        let lexer = Lexer::new("fn greet(name: String) -> String return name");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .first()
            .expect("expected parser diagnostic for missing function body");
        assert!(
            diag.hints()
                .iter()
                .any(|hint| hint.text.contains("while parsing function `greet`")),
            "expected breadcrumb note on parser diagnostic, got: {:?}",
            diag.hints()
        );
    }

    #[test]
    fn nested_parser_diagnostics_include_breadcrumb_chain() {
        let lexer = Lexer::new("fn outer() { let f = \\x -> match x { 0 \"zero\" } }");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|d| d.code() == Some("E034"))
            .expect("expected nested parser diagnostic");
        assert!(
            diag.hints().iter().any(|hint| hint.text.contains(
                "while parsing function `outer` > lambda expression > `match` expression"
            )),
            "expected breadcrumb chain on nested diagnostic, got: {:?}",
            diag.hints()
        );
    }

    #[test]
    fn function_breadcrumb_does_not_leak_to_following_top_level_error() {
        let lexer = Lexer::new("fn greet(name: String) -> String return name\nimport");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let import_diag = parser
            .errors
            .iter()
            .find(|diag| diag.display_title() == Some("Missing Import Path"))
            .expect("expected missing import path diagnostic");

        assert!(
            !import_diag
                .hints()
                .iter()
                .any(|hint| hint.text.contains("while parsing function `greet`")),
            "function breadcrumb leaked into later top-level diagnostic: {:?}",
            import_diag.hints()
        );
    }

    #[test]
    fn module_breadcrumb_does_not_leak_to_following_top_level_error() {
        let lexer = Lexer::new("module Demo\nimport");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let import_diag = parser
            .errors
            .iter()
            .find(|diag| diag.display_title() == Some("Missing Import Path"))
            .expect("expected missing import path diagnostic");

        assert!(
            !import_diag
                .hints()
                .iter()
                .any(|hint| hint.text.contains("while parsing module `Demo`")),
            "module breadcrumb leaked into later top-level diagnostic: {:?}",
            import_diag.hints()
        );
    }

    #[test]
    fn match_breadcrumb_does_not_leak_to_following_top_level_error() {
        let lexer = Lexer::new("match 1 { 0 \"zero\" }\nimport");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let import_diag = parser
            .errors
            .iter()
            .find(|diag| diag.display_title() == Some("Missing Import Path"))
            .expect("expected missing import path diagnostic");

        assert!(
            !import_diag
                .hints()
                .iter()
                .any(|hint| hint.text.contains("while parsing `match` expression")),
            "match breadcrumb leaked into later top-level diagnostic: {:?}",
            import_diag.hints()
        );
    }

    #[test]
    fn helper_emitted_diagnostics_keep_current_breadcrumb() {
        let lexer = Lexer::new("fn greet(name: String) -> String { match 1 { 0 \"zero\" } }");
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser
            .errors
            .iter()
            .find(|diag| diag.display_title() == Some("Missing Match Arm Arrow"))
            .expect("expected missing match arm arrow diagnostic");

        assert!(
            diag.hints().iter().any(|hint| hint
                .text
                .contains("while parsing function `greet` > `match` expression")),
            "expected contextual breadcrumb on helper-emitted diagnostic, got: {:?}",
            diag.hints()
        );
    }
}
