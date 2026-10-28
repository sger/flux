use super::*;

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
        Statement::Function { name, parameters, body, .. } => {
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

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(missing_comma_count, 2, "expected one E073 per missing comma gap");
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

    let missing_comma: Vec<_> = parser.errors.iter().filter(|d| d.code() == Some("E073")).collect();
    assert_eq!(missing_comma.len(), 1, "expected exactly one missing-comma diagnostic");
    let span = missing_comma[0].span().expect("expected missing-comma span");
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
fn test_multiline_string_normalizes_crlf_line_endings() {
    let (program, _interner) = parse("\"\"\"alpha\r\nbeta\r\ngamma\"\"\"");

    match &program.statements[0] {
        Statement::Expression { expression: Expression::String { value, .. }, .. } => {
            assert_eq!(value, "alpha\nbeta\ngamma")
        }
        other => panic!("expected string expression, got {other:?}"),
    }
}

#[test]
fn test_interpolated_multiline_string_normalizes_crlf_line_endings() {
    let (program, _interner) = parse("\"\"\"alpha\r\n#{name}\r\nomega\"\"\"");

    match &program.statements[0] {
        Statement::Expression {
            expression: Expression::InterpolatedString { parts, .. }, ..
        } => {
            assert!(
                matches!(&parts[0], flux::syntax::expression::StringPart::Literal(s) if s == "alpha\n")
            );
            assert!(
                matches!(&parts[2], flux::syntax::expression::StringPart::Literal(s) if s == "\nomega")
            );
        }
        other => panic!("expected interpolated string expression, got {other:?}"),
    }
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

    let e072_count = parser.errors.iter().filter(|d| d.code() == Some("E072")).count();
    let e071_count = parser.errors.iter().filter(|d| d.code() == Some("E071")).count();

    assert_eq!(e072_count, 1, "expected one interpolation error");
    assert_eq!(e071_count, 1, "expected one unterminated string error after recovery");
}

#[test]
fn t1_named_delimiter_diagnostics_are_contextual() {
    let cases = [
        ("let h = {one: 1, two: 2", "Expected `}` to close hash literal."),
        ("let xs = [|1, 2|", "Expected `]` to close array literal."),
        ("let add = \\(x, y -> x + y", "Expected `)` to close lambda parameter list."),
        ("let msg = \"Hello #{name\"", "Expected `}` to close string interpolation."),
        ("let ys = [x + 1 | x <- [1, 2]", "Expected `]` to close list comprehension."),
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

    let missing_comma_count = parser.errors.iter().filter(|d| d.code() == Some("E073")).count();
    assert_eq!(missing_comma_count, 2, "expected one E073 per missing comma gap");
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

    let missing_comma: Vec<_> = parser.errors.iter().filter(|d| d.code() == Some("E073")).collect();
    assert_eq!(missing_comma.len(), 1, "expected exactly one missing-comma diagnostic");
    let span = missing_comma[0].span().expect("expected missing-comma span");
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
        parser.errors.iter().any(|d| d.code() == Some("E076") || d.code() == Some("E034")),
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
        first.message().is_some_and(|m| m.contains("identifier or tuple field index after `.`")),
        "unexpected message: {:?}",
        first.message()
    );
}

#[test]
fn test_dangling_member_access_across_newline_avoids_duplicate_delimiter_error() {
    let lexer = Lexer::new("print(point.\nprint(point.1)\nlet x = 1");
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    let e034_count = parser.errors.iter().filter(|d| d.code() == Some("E034")).count();
    assert_eq!(e034_count, 1, "expected only one E034 for dangling member access");
}

#[test]
fn test_missing_tuple_rparen_before_new_statement_does_not_cascade() {
    let lexer = Lexer::new("let point = (1, 2, 3\nlet single = (42,)\nprint(single)");
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    assert!(
        parser.errors.iter().any(|d| (d.code() == Some("E076") || d.code() == Some("E034"))
            && d.message().is_some_and(|m| m.contains(")") || m.contains("closing"))),
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
    let first = parser.errors.first().expect("expected at least one parse error");
    assert!(
        first.code() == Some("E076") || first.code() == Some("E034"),
        "expected UNCLOSED_DELIMITER or UNEXPECTED_TOKEN, got: {:?}",
        first.code()
    );
    assert!(
        first.message().is_some_and(|m| m.contains(")") || m.contains("closing")),
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
    let first = parser.errors.first().expect("expected at least one parse error");
    assert_eq!(first.code(), Some("E034"));
    assert!(
        first.message().is_some_and(|m| m.contains("identifier or tuple field index after `.`")),
        "expected first diagnostic to be about incomplete member access, got: {:?}",
        first.message()
    );
}

#[test]
fn test_malformed_signature_missing_tuple_close_suppresses_redundant_parameter_followups() {
    let lexer = Lexer::new(include_str!(
        "../../../examples/type_system/failing/184_type_expr_missing_close_paren.flx"
    ));
    let mut parser = Parser::new(lexer);
    let _program = parser.parse_program();

    let root_missing_tuple_close = parser.errors.iter().any(|d| {
        d.code() == Some("E034")
            && d.message()
                .is_some_and(|m| m.contains("close tuple type") || m.contains("Tuple types use"))
    });
    assert!(
        root_missing_tuple_close,
        "expected a root tuple-close diagnostic: {:?}",
        parser.errors
    );

    let redundant_param_followup = parser.errors.iter().any(|d| {
        d.code() == Some("E034")
            && d.message().is_some_and(|m| m.contains("after function parameter"))
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
