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
        messages
            .iter()
            .any(|m| m.contains("Expected `->` in match arm, found `=>`")),
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
                        m.contains("Expected `->` in match arm")
                            || m.contains("Expected `->` after lambda parameters")
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
            d.code() == Some("E034")
                && d.message()
                    .is_some_and(|m| m.contains("begin the `do` block"))
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
        assert!(
            has_let_binding(&program, &interner, "after"),
            "expected trailing `let after` to remain after recovery for case `{name}`"
        );
    }
}
