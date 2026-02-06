use std::panic::{self, AssertUnwindSafe};

use flux::frontend::{diagnostics::Diagnostic, lexer::Lexer, parser::Parser, program::Program, statement::Statement};

#[derive(Clone, Copy)]
struct RecoveryCase {
    name: &'static str,
    input: &'static str,
    expect_followup_let: Option<&'static str>,
}

fn parse_no_panic(input: &str) -> Result<(Program, Vec<Diagnostic>), String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        (program, parser.errors)
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

fn has_let_binding(program: &Program, name: &str) -> bool {
    program
        .statements
        .iter()
        .any(|stmt| matches!(stmt, Statement::Let { name: n, .. } if n == name))
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
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "stray_arrow_token",
            input: "->\nlet ok = 1;",
            expect_followup_let: Some("ok"),
        },
        RecoveryCase {
            name: "stray_comma",
            input: ",\nlet ok = 1;",
            expect_followup_let: Some("ok"),
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

        let (program, diagnostics) = parsed.expect("already checked panic state");

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
                has_let_binding(&program, binding_name),
                "expected recovery to keep parsing trailing `let {}` in case `{}`",
                binding_name,
                case.name
            );
        }
    }
}
