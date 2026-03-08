use std::panic::{self, AssertUnwindSafe};

use flux::diagnostics::Diagnostic;
use flux::syntax::{
    expression::Expression, interner::Interner, lexer::Lexer, parser::Parser, program::Program,
    statement::Statement,
};

#[derive(Clone, Copy)]
struct RecoveryCase {
    name: &'static str,
    input: &'static str,
    expect_followup_let: Option<&'static str>,
}

fn parse_no_panic(input: &str) -> Result<(Program, Vec<Diagnostic>, Interner), String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let interner = parser.take_interner();
        (program, parser.errors, interner)
    }))
    .map_err(|payload| {
        if let Some(msg) = payload.downcast_ref::<&str>() {
            (*msg).to_string()
        } else if let Some(msg) = payload.downcast_ref::<String>() {
            msg.clone()
        } else {
            "non-string panic payload".to_string()
        }
    })
}

fn has_let_binding(program: &Program, interner: &Interner, name: &str) -> bool {
    program
        .statements
        .iter()
        .any(|stmt| matches!(stmt, Statement::Let { name: n, .. } if interner.resolve(*n) == name))
}

fn has_print_call_with_int_arg(program: &Program, interner: &Interner, expected: i64) -> bool {
    program.statements.iter().any(|stmt| {
        matches!(
            stmt,
            Statement::Expression {
                expression: Expression::Call { function, arguments, .. },
                ..
            } if matches!(&**function, Expression::Identifier { name, .. } if interner.resolve(*name) == "print")
                && matches!(arguments.as_slice(), [Expression::Integer { value, .. }] if *value == expected)
        )
    })
}

fn has_function(program: &Program, interner: &Interner, name: &str) -> bool {
    program.statements.iter().any(
        |stmt| matches!(stmt, Statement::Function { name: n, .. } if interner.resolve(*n) == name),
    )
}

#[test]
fn malformed_expression_corpus_recovers_without_panicking() {
    let cases = [
        RecoveryCase {
            name: "missing_comma_call_args",
            input: "f(1 2)\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "missing_operator_between_literals",
            input: "let a = (1 2);\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "invalid_infix_chain",
            input: "a + * b;\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "unfinished_infix",
            input: "a +\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "unmatched_paren",
            input: "print((1 + 2;\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "unmatched_bracket",
            input: "let a = [1, 2;\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "extra_closing_paren",
            input: "print(1));\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "stray_closing_paren",
            input: ")\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "stray_arrow_token",
            input: "->\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "stray_comma",
            input: ",\nlet ok = 1;",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "unterminated_string_then_valid_statement",
            input: "let s = \"oops\nlet ok = 1;\nprint(ok);",
            expect_followup_let: None,
        },
        RecoveryCase {
            name: "malformed_match_arm_separator",
            input: "match x { 0 1 }\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
    ];

    for case in cases {
        let parsed = parse_no_panic(case.input);
        assert!(
            parsed.is_ok(),
            "parser panicked for case `{}`: {:?}",
            case.name,
            parsed.err()
        );

        let (program, diagnostics, interner) = parsed.expect("already checked panic state");

        assert!(
            !diagnostics.is_empty(),
            "expected diagnostics for malformed case `{}`",
            case.name
        );
        assert!(
            diagnostics.iter().any(|d| d.code().is_some()),
            "expected at least one coded diagnostic for case `{}`",
            case.name
        );

        // Program shape invariant: parser still returns a valid root Program span.
        let span = program.span();
        assert!(
            span.end.line >= 1,
            "expected a valid root Program span for case `{}`",
            case.name
        );

        if let Some(binding_name) = case.expect_followup_let {
            assert!(
                has_let_binding(&program, &interner, binding_name),
                "expected recovery to keep parsing trailing `let {}` in case `{}`",
                binding_name,
                case.name
            );
        }
    }
}

#[test]
fn malformed_statements_recover_and_keep_followup_statements() {
    let cases = [
        (
            "missing_let_initializer_then_followup_let",
            "let x = ;\nlet y = 1;",
            true,
            false,
        ),
        (
            "stray_closing_brace_then_followup_let",
            "}\nlet a = 1;",
            true,
            false,
        ),
        (
            "malformed_if_condition_then_followup_print",
            "if (true { print(1); }\nprint(2);",
            false,
            true,
        ),
    ];

    for (name, input, expect_let, expect_print) in cases {
        let parsed = parse_no_panic(input);
        assert!(
            parsed.is_ok(),
            "parser panicked for recovery case `{}`: {:?}",
            name,
            parsed.err()
        );

        let (program, diagnostics, interner) = parsed.expect("already checked panic state");
        assert!(
            !diagnostics.is_empty(),
            "expected diagnostics for recovery case `{}`",
            name
        );

        if expect_let {
            assert!(
                has_let_binding(
                    &program,
                    &interner,
                    if name.contains("initializer") {
                        "y"
                    } else {
                        "a"
                    }
                ),
                "expected follow-up let binding to be present in recovery case `{}`",
                name
            );
        }

        if expect_print {
            assert!(
                has_print_call_with_int_arg(&program, &interner, 2),
                "expected follow-up print(2) statement in recovery case `{}`",
                name
            );
        }
    }
}

#[test]
fn type_annotation_recovery_fixture_keeps_followup_statements() {
    let input = include_str!("fixtures/recovery/type_annotation_recovery.flx");
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("annotation recovery fixture should not panic");

    assert!(
        diagnostics.iter().any(|d| d.code() == Some("E034")),
        "expected parser diagnostics for malformed annotations"
    );
    assert!(
        has_let_binding(&program, &interner, "ok1")
            && has_let_binding(&program, &interner, "ok2")
            && has_let_binding(&program, &interner, "ok3")
            && has_let_binding(&program, &interner, "ok4"),
        "expected follow-up let bindings to survive malformed annotation recovery"
    );
    assert!(
        has_function(&program, &interner, "f") && has_function(&program, &interner, "g"),
        "expected malformed annotation functions to remain in parsed output"
    );
}

#[test]
fn unterminated_string_recovery_fixture_keeps_followup_statement() {
    let input = include_str!("../examples/type_system/failing/100_unclosed_string_recovery.flx");
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("unterminated-string recovery fixture should not panic");

    assert!(
        diagnostics.iter().any(|d| d.code() == Some("E071")),
        "expected unterminated string diagnostic"
    );
    assert!(
        has_let_binding(&program, &interner, "after"),
        "expected parser recovery to keep follow-up let binding after unterminated string"
    );
}

#[test]
fn kitchen_sink_many_errors_reports_broad_error_set_without_panicking() {
    let input = include_str!("../examples/parser_errors/kitchen_sink_many_errors.flx");
    let (_program, diagnostics, _interner) =
        parse_no_panic(input).expect("kitchen-sink parser fixture should not panic");

    assert!(
        diagnostics.len() >= 18,
        "expected broad parser coverage with at least 18 diagnostics, got {}: {:#?}",
        diagnostics.len(),
        diagnostics
            .iter()
            .map(|d| (d.code(), d.title(), d.message()))
            .collect::<Vec<_>>()
    );

    assert!(
        diagnostics.iter().any(|d| d.code() == Some("E073")),
        "expected at least one missing-comma diagnostic in kitchen-sink fixture"
    );
    assert!(
        diagnostics.iter().any(|d| d.code() == Some("E076")),
        "expected at least one missing-closing-delimiter diagnostic in kitchen-sink fixture"
    );
    assert!(
        diagnostics.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("This import needs a module path"))
        }),
        "expected missing-import-path diagnostic in kitchen-sink fixture"
    );
    assert!(
        diagnostics.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("Missing Handle Arm Arrow"))
                || d.message()
                    .is_some_and(|m| m.contains("Expected `->` in handle arm"))
        }),
        "expected late-file handle-arm diagnostic in kitchen-sink fixture"
    );
    assert!(
        diagnostics.iter().any(|d| {
            d.message()
                .is_some_and(|m| m.contains("This import alias needs a name after `as`."))
                || d.message().is_some_and(|m| m.contains("import alias"))
        }),
        "expected missing-import-alias diagnostic in kitchen-sink fixture"
    );
}

#[test]
fn long_functions_clean_many_errors_reports_broad_late_recovery_without_noise() {
    let input = include_str!("../examples/parser_errors/long_functions_clean_many_errors.flx");
    let (_program, diagnostics, _interner) =
        parse_no_panic(input).expect("clean long-functions fixture should not panic");

    assert!(
        diagnostics.len() >= 12,
        "expected many independent diagnostics in clean long-functions fixture, got {}: {:#?}",
        diagnostics.len(),
        diagnostics
            .iter()
            .map(|d| (d.code(), d.display_title(), d.message()))
            .collect::<Vec<_>>()
    );

    assert!(
        diagnostics
            .iter()
            .any(|d| d.display_title() == Some("Missing Hash Colon")),
        "expected missing-hash-colon diagnostic in clean long-functions fixture"
    );
    assert!(
        diagnostics.iter().any(|d| d.code() == Some("E073")),
        "expected missing-comma diagnostic in clean long-functions fixture"
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.display_title() == Some("Missing Else Body")),
        "expected missing-else-body diagnostic in clean long-functions fixture"
    );
    assert!(
        diagnostics
            .iter()
            .filter(|d| d.display_title() == Some("Missing Hash Colon"))
            .count()
            >= 4
            && diagnostics
                .iter()
                .filter(|d| d.code() == Some("E073"))
                .count()
                >= 4
            && diagnostics
                .iter()
                .filter(|d| d.display_title() == Some("Missing Else Body"))
                .count()
                >= 4,
        "expected repeated diagnostics from both long functions to survive recovery"
    );
    assert!(
        !diagnostics.iter().any(|d| {
            d.display_title() == Some("Unexpected Closing Delimiter")
                || d.message()
                    .is_some_and(|m| m.contains("Unexpected `}` outside of a block."))
                || d.message()
                    .is_some_and(|m| m.contains("Expected expression, found `)`."))
        }),
        "did not expect obvious stray-closer fallout in clean long-functions fixture: {diagnostics:#?}"
    );
}

#[test]
fn many_independent_parser_errors_are_all_reported() {
    let input = include_str!("../examples/parser_errors/many_independent_errors.flx");
    let (_program, diagnostics, _interner) =
        parse_no_panic(input).expect("many-errors fixture should not panic");

    assert!(
        diagnostics.len() >= 3,
        "expected several independent diagnostics, got: {diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .filter(|diag| diag.code() == Some("E034") || diag.code() == Some("E076"))
            .count()
            >= 3,
        "expected at least three structural parser diagnostics, got: {diagnostics:#?}"
    );
}

#[test]
fn missing_else_brace_does_not_emit_outer_unclosed_brace() {
    let input = include_str!("../examples/parser_errors/missing_else_brace.flx");
    let (_program, diagnostics, _) = parse_no_panic(input).expect("fixture should not panic");

    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message().is_some_and(|m| m.contains("`else` branch"))),
        "expected missing else-body diagnostic, got: {diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|diag| {
            diag.code() == Some("E076")
                && diag
                    .message()
                    .is_some_and(|m| m.contains("function `main`"))
        }),
        "did not expect false outer unclosed-brace diagnostic, got: {diagnostics:#?}"
    );
}

#[test]
fn missing_lambda_arrow_does_not_emit_trailing_paren_expression_error() {
    let input = include_str!("../examples/parser_errors/missing_lambda_arrow.flx");
    let (_program, diagnostics, _) = parse_no_panic(input).expect("fixture should not panic");

    assert!(
        diagnostics.iter().any(|diag| diag
            .message()
            .is_some_and(|m| m.contains("lambda parameters"))),
        "expected missing lambda arrow diagnostic, got: {diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|diag| {
            diag.code() == Some("E031")
                && diag
                    .message()
                    .is_some_and(|m| m.contains("Expected expression, found )"))
        }),
        "did not expect trailing `)` expression fallout, got: {diagnostics:#?}"
    );
}

#[test]
fn missing_bracket_does_not_emit_missing_comma_fallout() {
    let input = include_str!("../examples/parser_errors/missing_bracket.flx");
    let (_program, diagnostics, _) = parse_no_panic(input).expect("fixture should not panic");

    assert!(
        diagnostics.iter().any(|diag| diag.code() == Some("E076")),
        "expected missing closing bracket diagnostic, got: {diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|diag| diag.code() == Some("E073")),
        "did not expect derivative missing-comma diagnostic, got: {diagnostics:#?}"
    );
}

#[test]
fn missing_import_path_points_at_import_site() {
    let input = include_str!("../examples/parser_errors/missing_import_path.flx");
    let (_program, diagnostics, _) = parse_no_panic(input).expect("fixture should not panic");

    let import_diag = diagnostics
        .iter()
        .find(|diag| {
            diag.message()
                .is_some_and(|m| m.contains("module path after `import`"))
        })
        .expect("expected import-path diagnostic");

    let span = import_diag.span().expect("expected import-path span");
    assert_eq!(
        span.start.line, 5,
        "expected diagnostic to point at import line"
    );
}

#[test]
fn missing_module_brace_does_not_emit_stray_top_level_rbrace() {
    let input = include_str!("../examples/parser_errors/missing_module_brace.flx");
    let (_program, diagnostics, _) = parse_no_panic(input).expect("fixture should not panic");

    assert!(
        diagnostics
            .iter()
            .any(|diag| diag.message().is_some_and(|m| m.contains("module body"))),
        "expected missing module-body brace diagnostic, got: {diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|diag| {
            diag.code() == Some("E034")
                && diag
                    .message()
                    .is_some_and(|m| m.contains("Unexpected `}` outside of a block"))
        }),
        "did not expect stray top-level `}}` fallout, got: {diagnostics:#?}"
    );
}

#[test]
fn parser_error_experience_recovery_fixture_keeps_followup_statement() {
    let input = include_str!("fixtures/recovery/059_parser_error_recovery.flx");
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("059 parser-error recovery fixture should not panic");

    let messages: Vec<String> = diagnostics
        .iter()
        .filter_map(|d| d.message().map(ToString::to_string))
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Unknown keyword `def`")),
        "expected keyword-alias diagnostic"
    );
    assert!(
        messages.iter().any(|m| m.contains("needs `->`, not `=>`")),
        "expected contextual fat-arrow diagnostic"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Match arms are separated by `,` in Flux, not `|`")),
        "expected match `|` separator diagnostic"
    );

    assert!(
        has_let_binding(&program, &interner, "ok_after"),
        "expected parser recovery to keep follow-up let binding after 059-style errors"
    );
}

#[test]
fn mixed_closer_nested_delimiters_does_not_invent_outer_unclosed_brace() {
    let input = include_str!("../examples/parser_errors/mixed_closer_nested_delimiters.flx");
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("mixed-closer fixture should not panic");

    assert!(
        diagnostics.len() >= 1,
        "expected parser diagnostics for mixed closer fixture"
    );
    assert!(
        !diagnostics.iter().any(|diag| {
            diag.code() == Some("E076")
                && diag
                    .message()
                    .is_some_and(|m| m.contains("function `main`"))
        }),
        "did not expect false outer unclosed-brace diagnostic, got: {diagnostics:#?}"
    );
    assert!(
        !diagnostics.iter().any(|diag| diag.code() == Some("E031")),
        "did not expect trailing expected-expression fallout, got: {diagnostics:#?}"
    );
    let _ = (program, interner);
}

#[test]
fn eof_truncation_fixtures_report_root_construct_diagnostics() {
    let cases = [
        (
            "eof_after_if_condition",
            include_str!("../examples/parser_errors/eof_after_if_condition.flx"),
            "if",
        ),
        (
            "eof_after_else",
            include_str!("../examples/parser_errors/eof_after_else.flx"),
            "else",
        ),
        (
            "eof_after_match_open",
            include_str!("../examples/parser_errors/eof_after_match_open.flx"),
            "match",
        ),
        (
            "eof_after_lambda_params",
            include_str!("../examples/parser_errors/eof_after_lambda_params.flx"),
            "lambda",
        ),
        (
            "eof_after_module_name",
            include_str!("../examples/parser_errors/eof_after_module_name.flx"),
            "module",
        ),
        (
            "eof_after_perform_dot",
            include_str!("../examples/parser_errors/eof_after_perform_dot.flx"),
            "perform",
        ),
    ];

    for (name, input, fragment) in cases {
        let (_program, diagnostics, _) =
            parse_no_panic(input).unwrap_or_else(|e| panic!("{name} panicked: {e}"));
        assert!(
            diagnostics
                .iter()
                .filter_map(|d| d.message())
                .any(|m| m.to_lowercase().contains(fragment)),
            "expected contextual diagnostic mentioning `{fragment}` for `{name}`, got: {diagnostics:#?}"
        );
        if name != "eof_after_module_name" {
            assert!(
                !diagnostics.iter().any(|diag| {
                    diag.code() == Some("E076")
                        && diag
                            .message()
                            .is_some_and(|m| m.contains("function `main`"))
                }),
                "expected `{name}` to prefer the inner construct over the outer function block, got: {diagnostics:#?}"
            );
        }
        let first = diagnostics
            .first()
            .expect("expected eof-truncation diagnostic");
        let span = first.span().expect("expected eof-truncation span");
        assert!(
            span.start.line <= 2,
            "expected `{name}` to anchor on a real source line instead of synthetic EOF, got: {diagnostics:#?}"
        );
    }
}

#[test]
fn malformed_declaration_fixtures_keep_followup_main_when_recoverable() {
    let cases = [
        (
            "malformed_data_constructor_fields",
            include_str!("../examples/parser_errors/malformed_data_constructor_fields.flx"),
        ),
        (
            "malformed_type_variant_fields",
            include_str!("../examples/parser_errors/malformed_type_variant_fields.flx"),
        ),
        (
            "malformed_effect_op_list",
            include_str!("../examples/parser_errors/malformed_effect_op_list.flx"),
        ),
        (
            "broken_import_alias",
            include_str!("../examples/parser_errors/broken_import_alias.flx"),
        ),
    ];

    for (name, input) in cases {
        let (program, diagnostics, interner) =
            parse_no_panic(input).unwrap_or_else(|e| panic!("{name} panicked: {e}"));
        assert!(!diagnostics.is_empty(), "expected diagnostics for `{name}`");
        assert!(
            has_function(&program, &interner, "main"),
            "expected follow-up main declaration to survive for `{name}`"
        );
    }
}

#[test]
fn stray_top_level_tokens_do_not_prevent_followup_main() {
    let cases = [
        include_str!("../examples/parser_errors/stray_top_level_arrow.flx"),
        include_str!("../examples/parser_errors/stray_top_level_else.flx"),
    ];

    for input in cases {
        let (_program, diagnostics, _interner) =
            parse_no_panic(input).expect("stray top-level token fixture should not panic");
        assert!(!diagnostics.is_empty(), "expected stray-token diagnostic");
    }
}

#[test]
fn nested_multi_declaration_errors_are_all_reported() {
    let (_program, diagnostics, _) = parse_no_panic(include_str!(
        "../examples/parser_errors/nested_multi_declaration_errors.flx"
    ))
    .expect("nested multi-error fixture should not panic");

    assert!(
        diagnostics.len() >= 3,
        "expected multiple independent diagnostics, got: {diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .filter_map(|d| d.message())
            .any(|m| m.contains("constructor fields"))
            && diagnostics
                .iter()
                .filter_map(|d| d.message())
                .any(|m| m.contains("effect operation signature"))
            && diagnostics
                .iter()
                .filter_map(|d| d.message())
                .any(|m| m.contains("match arm")),
        "expected data/effect/match diagnostics together, got: {diagnostics:#?}"
    );
}

#[test]
fn t1_named_delimiter_fixtures_keep_followup_statement_when_recoverable() {
    let cases = [
        (
            "hash_close",
            include_str!("../examples/type_system/failing/122_hash_missing_close_brace.flx"),
            true,
        ),
        (
            "array_close",
            include_str!("../examples/type_system/failing/123_array_missing_close_bracket.flx"),
            true,
        ),
        (
            "lambda_close_paren",
            include_str!("../examples/type_system/failing/124_lambda_missing_close_paren.flx"),
            false,
        ),
        (
            "interpolation_close",
            include_str!(
                "../examples/type_system/failing/125_string_interpolation_missing_close_brace.flx"
            ),
            true,
        ),
        (
            "comprehension_close",
            include_str!(
                "../examples/type_system/failing/126_list_comprehension_missing_close_bracket.flx"
            ),
            true,
        ),
    ];

    for (name, input, expect_followup) in cases {
        let (program, diagnostics, interner) =
            parse_no_panic(input).expect("T1 fixture parse should not panic");
        assert!(
            diagnostics
                .iter()
                .any(|d| d.code() == Some("E034") && d.message().is_some()),
            "expected E034 parser diagnostic for case `{name}`"
        );
        if expect_followup {
            assert!(
                has_let_binding(&program, &interner, "after"),
                "expected trailing `let after` to remain after recovery for case `{name}`"
            );
        }
    }
}

#[test]
fn t2_arrow_fixtures_emit_contextual_e034_and_recover() {
    let cases = [
        (
            "match_missing_arrow",
            include_str!("../examples/type_system/failing/127_match_missing_arrow.flx"),
        ),
        (
            "lambda_missing_arrow",
            include_str!("../examples/type_system/failing/128_lambda_missing_arrow.flx"),
        ),
    ];

    for (name, input) in cases {
        let (program, diagnostics, interner) =
            parse_no_panic(input).expect("T2 fixture parse should not panic");
        assert!(
            diagnostics.iter().any(|d| {
                d.code() == Some("E034")
                    && d.message().is_some_and(|m| {
                        (m.contains("match arm") && m.contains("`->`"))
                            || (m.contains("lambda parameters") && m.contains("`->`"))
                    })
            }),
            "expected contextual E034 arrow diagnostic for case `{name}`"
        );
        assert!(
            has_let_binding(&program, &interner, "after"),
            "expected trailing `let after` to remain after recovery for case `{name}`"
        );
    }
}

#[test]
fn t3_orphan_constructor_fixture_emits_contextual_e034_and_recovers() {
    let input = include_str!(
        "../examples/type_system/failing/129_orphan_constructor_pattern_statement.flx"
    );
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("T3 fixture parse should not panic");
    assert!(
        diagnostics.iter().any(|d| {
            d.code() == Some("E034")
                && d.message()
                    .is_some_and(|m| m.contains("outside `match`") && m.contains("Some(...)"))
        }),
        "expected contextual orphan-constructor-pattern E034 diagnostic"
    );
    assert!(
        has_let_binding(&program, &interner, "after"),
        "expected trailing `let after` to remain after recovery"
    );
}

#[test]
fn t4_do_missing_brace_fixture_emits_contextual_e034_and_recovers() {
    let input = include_str!("../examples/type_system/failing/130_do_missing_brace.flx");
    let (program, diagnostics, interner) =
        parse_no_panic(input).expect("T4 fixture parse should not panic");
    assert!(
        diagnostics.iter().any(|d| {
            d.code() == Some("E034") && d.message().is_some_and(|m| m.contains("`do` block"))
        }),
        "expected contextual do-block E034 diagnostic"
    );
    assert!(
        has_let_binding(&program, &interner, "after"),
        "expected trailing `let after` to remain after recovery"
    );
}

#[test]
fn t15_broad_contextual_expect_peek_fixtures_emit_e034_and_recover() {
    let cases = [
        (
            "perform_missing_dot",
            include_str!("../examples/type_system/failing/173_perform_missing_dot.flx"),
        ),
        (
            "handle_missing_lbrace",
            include_str!("../examples/type_system/failing/174_handle_missing_lbrace.flx"),
        ),
        (
            "handle_arm_missing_arrow",
            include_str!("../examples/type_system/failing/175_handle_arm_missing_arrow.flx"),
        ),
        (
            "match_missing_open_brace",
            include_str!("../examples/type_system/failing/176_match_missing_open_brace.flx"),
        ),
        (
            "import_except_missing_open_bracket",
            include_str!(
                "../examples/type_system/failing/178_import_except_missing_open_bracket.flx"
            ),
        ),
        (
            "hash_missing_colon",
            include_str!("../examples/type_system/failing/183_hash_missing_colon.flx"),
        ),
        (
            "type_expr_missing_close_paren",
            include_str!("../examples/type_system/failing/184_type_expr_missing_close_paren.flx"),
        ),
    ];

    for (name, input) in cases {
        let (program, diagnostics, interner) =
            parse_no_panic(input).expect("T15 fixture parse should not panic");
        assert!(
            diagnostics.iter().any(|d| {
                d.code() == Some("E034") && d.message().is_some() && !d.hints().is_empty()
            }),
            "expected contextual E034 with hint for case `{name}`"
        );
        if name == "handle_arm_missing_arrow" {
            let diag = diagnostics
                .iter()
                .find(|d| {
                    d.code() == Some("E034")
                        && d.message()
                            .is_some_and(|m| m.contains("handle arm") && m.contains("`->`"))
                })
                .expect("expected exact handle-arm E034 diagnostic");
            let first_hint = diag
                .hints()
                .first()
                .map(|h| h.text.as_str())
                .expect("expected handle-arm hint");
            assert_eq!(
                first_hint, "Handle arms use `op(resume, arg1, ...) -> body`.",
                "handle-arm hint text regressed"
            );
        }
        assert!(
            has_let_binding(&program, &interner, "after"),
            "expected trailing `let after` to remain after recovery for case `{name}`"
        );
    }
}

#[test]
fn t16_contextual_recovery_fixtures_emit_e034_and_recover() {
    let cases = [
        (
            "perform_missing_dot_fixture",
            include_str!("fixtures/recovery/t16_perform_missing_dot_contextual.flx"),
            "This `perform` expression needs `.` between the effect and operation.",
            "Perform expressions use `perform Effect.op(args...)`.",
        ),
        (
            "handle_missing_lbrace_fixture",
            include_str!("fixtures/recovery/t16_handle_missing_lbrace_contextual.flx"),
            "Expected `{` to begin `handle` arms.",
            "Handle expressions use `expr handle Effect { ... }`.",
        ),
        (
            "module_missing_lbrace_fixture",
            include_str!("fixtures/recovery/t16_module_missing_lbrace_contextual.flx"),
            "This module body needs to start with `{`.",
            "Module declarations use `module Name { ... }`.",
        ),
    ];

    for (name, input, expected_message, expected_hint) in cases {
        let (program, diagnostics, interner) =
            parse_no_panic(input).expect("T16 fixture parse should not panic");
        let diag = diagnostics
            .iter()
            .find(|d| d.code() == Some("E034") && d.message() == Some(expected_message))
            .unwrap_or_else(|| panic!("expected exact E034 message for case `{name}`"));
        let msg = diag
            .message()
            .unwrap_or_else(|| panic!("expected message for case `{name}`"));
        assert_eq!(
            msg, expected_message,
            "unexpected E034 message for case `{name}`"
        );
        assert_ne!(
            msg.trim(),
            "Unexpected token.",
            "regressed to generic E034 wording for case `{name}`"
        );
        let first_hint = diag
            .hints()
            .first()
            .map(|h| h.text.as_str())
            .unwrap_or_else(|| panic!("expected hint for case `{name}`"));
        assert_eq!(
            first_hint, expected_hint,
            "unexpected E034 hint for case `{name}`"
        );
        assert!(
            has_let_binding(&program, &interner, "after"),
            "expected trailing `let after` to remain after recovery for case `{name}`"
        );
    }
}
