use super::*;

fn strip_parity_metadata_header(source: &str) -> &str {
    let mut offset = 0;
    let mut lines = source.split_inclusive('\n').peekable();

    while let Some(line) = lines.peek().copied() {
        let trimmed = line.trim();
        if trimmed == "// parity-expected-stdout-begin" {
            offset += line.len();
            lines.next();
            for block_line in lines.by_ref() {
                offset += block_line.len();
                if block_line.trim() == "// parity-expected-stdout-end" {
                    break;
                }
            }
            continue;
        }
        if trimmed == "// parity-expected-stderr-begin" {
            offset += line.len();
            lines.next();
            for block_line in lines.by_ref() {
                offset += block_line.len();
                if block_line.trim() == "// parity-expected-stderr-end" {
                    break;
                }
            }
            continue;
        }
        if trimmed.starts_with("// expect:") || trimmed.starts_with("// expect-error:") {
            offset += line.len();
            lines.next();
            continue;
        }
        if trimmed.is_empty() {
            offset += line.len();
            lines.next();
            continue;
        }
        break;
    }

    &source[offset..]
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
        Case { name: "hash_missing_colon", input: "let h = { \"a\" 1 }", message_fragment: "hash" },
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

        let msg =
            diag.message().unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
        assert!(
            msg.to_lowercase().contains(case.message_fragment),
            "expected message to mention `{}` for case `{}`, got: {}",
            case.message_fragment,
            case.name,
            msg
        );
        assert!(!diag.hints().is_empty(), "expected help hint for case `{}`", case.name);
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
            input: include_str!("../../fixtures/recovery/t16_perform_missing_dot_contextual.flx"),
            message_fragment: "perform",
        },
        Case {
            name: "handle_missing_lbrace_fixture",
            input: include_str!("../../fixtures/recovery/t16_handle_missing_lbrace_contextual.flx"),
            message_fragment: "handle",
        },
        Case {
            name: "module_missing_lbrace_fixture",
            input: include_str!("../../fixtures/recovery/t16_module_missing_lbrace_contextual.flx"),
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

        let msg =
            diag.message().unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
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
                "../../../examples/type_system/failing/173_perform_missing_dot.flx"
            ),
            expected_message: "This `perform` expression needs `.` between the effect and operation.",
            expected_hint: "Perform expressions use `perform Effect.op(args...)`.",
        },
        Case {
            name: "handle_missing_lbrace",
            input: include_str!(
                "../../../examples/type_system/failing/174_handle_missing_lbrace.flx"
            ),
            expected_message: "Expected `{` to begin `handle` arms.",
            expected_hint: "Handle expressions use `expr handle Effect { ... }`.",
        },
        Case {
            name: "handle_arm_missing_arrow",
            input: include_str!(
                "../../../examples/type_system/failing/175_handle_arm_missing_arrow.flx"
            ),
            expected_message: "Expected `->` in handle arm.",
            expected_hint: "Handle arms use `op(resume, arg1, ...) -> body`.",
        },
        Case {
            name: "module_missing_lbrace",
            input: include_str!("../../fixtures/recovery/t16_module_missing_lbrace_contextual.flx"),
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

        let msg =
            diag.message().unwrap_or_else(|| panic!("expected message for case `{}`", case.name));
        assert_eq!(msg, case.expected_message, "unexpected E034 message for case `{}`", case.name);
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
        assert_eq!(first_hint, case.expected_hint, "unexpected E034 hint for case `{}`", case.name);
    }
}

#[test]
fn parser_torture_contextual_titles_and_no_token_jargon() {
    let cases = [
        (
            include_str!("../../../examples/parser_errors/malformed_data_constructor_fields.flx"),
            "constructor",
        ),
        (
            include_str!("../../../examples/parser_errors/malformed_type_variant_fields.flx"),
            "constructor",
        ),
        (include_str!("../../../examples/parser_errors/malformed_effect_op_list.flx"), "effect"),
        (include_str!("../../../examples/parser_errors/eof_after_perform_dot.flx"), "perform"),
    ];

    for (input, fragment) in cases {
        let input = strip_parity_metadata_header(input);
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        assert!(
            parser.errors.iter().any(|d| d.code() == Some("E034") || d.code() == Some("E076")),
            "expected structural parser diagnostic, got: {:?}",
            parser.errors
        );
        assert!(
            parser
                .errors
                .iter()
                .filter(|d| d.code() == Some("E034"))
                .all(|d| d.display_title().is_some_and(|title| title != "Unexpected Token")),
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
            include_str!("../../../examples/parser_errors/eof_after_if_condition.flx"),
            "Missing If Body",
            2usize,
            "this `if` branch starts here",
        ),
        (
            include_str!("../../../examples/parser_errors/eof_after_else.flx"),
            "Missing Else Body",
            2usize,
            "`else` starts here",
        ),
        (
            include_str!("../../../examples/parser_errors/eof_after_match_open.flx"),
            "Missing Match Body",
            2usize,
            "this match expression starts here",
        ),
        (
            include_str!("../../../examples/parser_errors/eof_after_lambda_params.flx"),
            "Missing Lambda Arrow",
            2usize,
            "this lambda parameter list ends here",
        ),
        (
            include_str!("../../../examples/parser_errors/eof_after_perform_dot.flx"),
            "Missing Effect Operation Name",
            2usize,
            "this `perform` expression ends here",
        ),
    ];

    for (input, expected_title, expected_line, expected_label) in cases {
        let input = strip_parity_metadata_header(input);
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let _ = parser.parse_program();

        let diag = parser.errors.first().expect("expected eof-truncation diagnostic");
        assert_eq!(diag.code(), Some("E034"));
        assert_eq!(diag.display_title(), Some(expected_title));

        let span = diag.span().expect("expected eof-truncation span");
        assert_eq!(span.start.line, expected_line);
        assert!(
            diag.labels().iter().any(|label| label.text == expected_label),
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

    let diag = parser.errors.first().expect("expected parser diagnostic for missing function body");
    assert!(
        diag.hints().iter().any(|hint| hint.text.contains("while parsing function `greet`")),
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
        diag.hints().iter().any(|hint| hint
            .text
            .contains("while parsing function `outer` > lambda expression > `match` expression")),
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
        !import_diag.hints().iter().any(|hint| hint.text.contains("while parsing module `Demo`")),
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
        diag.hints()
            .iter()
            .any(|hint| hint.text.contains("while parsing function `greet` > `match` expression")),
        "expected contextual breadcrumb on helper-emitted diagnostic, got: {:?}",
        diag.hints()
    );
}

#[test]
fn parses_radix_and_underscored_number_literals() {
    // The lexer tokenizes hex/binary/underscored numbers; the parser must
    // decode them to the right value (not just accept the decimal form).
    let (program, _) = parse(
        "let a = 0xFF\n\
             let b = 0b1010\n\
             let c = 1_000\n\
             let d = 0xDEAD_BEEF\n\
             let e = 1_000.5\n",
    );
    let int_value = |i: usize| -> i64 {
        let Statement::Let { value, .. } = &program.statements[i] else {
            panic!("expected let statement at {i}");
        };
        match value {
            Expression::Integer { value, .. } => *value,
            other => panic!("expected integer literal at {i}, got {other:?}"),
        }
    };
    assert_eq!(int_value(0), 255, "0xFF");
    assert_eq!(int_value(1), 10, "0b1010");
    assert_eq!(int_value(2), 1000, "1_000");
    assert_eq!(int_value(3), 0xDEAD_BEEF, "hex with separators");

    let Statement::Let { value, .. } = &program.statements[4] else {
        panic!("expected let statement at 4");
    };
    assert!(
        matches!(value, Expression::Float { value, .. } if (*value - 1000.5).abs() < f64::EPSILON),
        "expected float 1000.5, got {value:?}"
    );
}
