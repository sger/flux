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
