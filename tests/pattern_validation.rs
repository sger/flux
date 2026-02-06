use std::panic::{self, AssertUnwindSafe};

use flux::{
    bytecode::compiler::Compiler,
    frontend::{
        diagnostics::Diagnostic, lexer::Lexer, parser::Parser,
        pattern_validate::validate_program_patterns, program::Program,
    },
};

type CompileOutcome = Result<(), Vec<Diagnostic>>;

fn parse_and_validate_patterns_no_panic(input: &str) -> Result<(Program, Vec<Diagnostic>), String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let parser_errors = parser.errors;
        let pattern_diags = validate_program_patterns(&program, "pattern_validation_test.flx");
        (program, parser_errors, pattern_diags)
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
    .map(|(program, parser_errors, pattern_diags)| {
        let mut all_diags = parser_errors;
        all_diags.extend(pattern_diags);
        (program, all_diags)
    })
}

fn parse_and_validate_no_panic(
    input: &str,
) -> Result<(Program, Vec<Diagnostic>, CompileOutcome), String> {
    panic::catch_unwind(AssertUnwindSafe(|| {
        let lexer = Lexer::new(input);
        let mut parser = Parser::new(lexer);
        let program = parser.parse_program();
        let parser_errors = parser.errors;

        let mut compiler = Compiler::new_with_file_path("pattern_validation_test.flx");
        let compile_result = compiler.compile(&program);

        (program, parser_errors, compile_result)
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

fn find_diag_by_code<'a>(diags: &'a [Diagnostic], code: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.code() == Some(code))
        .unwrap_or_else(|| {
            panic!(
                "expected diagnostic code {code}, got {:?}",
                diag_codes(diags)
            )
        })
}

fn diag_codes(diags: &[Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .map(|d| d.code().unwrap_or("<none>").to_string())
        .collect()
}

#[test]
fn valid_match_patterns_emit_no_pattern_validation_diagnostics() {
    let src = r#"
let x = Some(1);
match x { Some(v) -> v, None -> 0, _ -> 0 };
"#;

    let result = parse_and_validate_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, parser_errors, compile_result) = result.expect("already checked panic");
    assert!(
        parser_errors.is_empty(),
        "expected no parser errors for valid pattern input, got: {:?}",
        parser_errors
    );
    assert!(
        compile_result.is_ok(),
        "expected pattern validation to pass, got compile diagnostics: {:?}",
        compile_result.err()
    );
}

#[test]
fn catchall_not_last_reports_code_message_and_span() {
    let src = r#"
let x = 2;
match x { _ -> 1, 2 -> 2 };
"#;

    let result = parse_and_validate_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, parser_errors, compile_result) = result.expect("already checked panic");
    assert!(
        parser_errors.is_empty(),
        "expected parser-clean input for pattern validation test, got: {:?}",
        parser_errors
    );

    let compile_diags = compile_result.expect_err("expected E016 from pattern validation");
    let diag = find_diag_by_code(&compile_diags, "E016");
    assert_eq!(diag.title(), "INVALID PATTERN");
    assert!(
        diag.message()
            .is_some_and(|m| m.contains("Catch-all patterns must be the final match arm")),
        "unexpected E016 message: {:?}",
        diag.message()
    );

    let span = diag.span().expect("expected primary span for E016");
    assert_eq!(span.start.line, 3);
    assert_eq!(span.start.column, 10);
}

#[test]
fn non_exhaustive_match_reports_code_and_message_fragment() {
    let src = r#"
let x = 2;
match x { 1 -> 10 };
"#;

    let result = parse_and_validate_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, parser_errors, compile_result) = result.expect("already checked panic");
    assert!(
        parser_errors.is_empty(),
        "expected parser-clean input for pattern validation test, got: {:?}",
        parser_errors
    );

    let compile_diags = compile_result.expect_err("expected E015 from pattern validation");
    let diag = find_diag_by_code(&compile_diags, "E015");
    assert_eq!(diag.title(), "NON-EXHAUSTIVE MATCH");
    assert!(
        diag.message()
            .is_some_and(|m| m.contains("must end with a `_` or identifier arm")),
        "unexpected E015 message: {:?}",
        diag.message()
    );
}

#[test]
fn empty_match_reports_e014() {
    let src = r#"
let x = 2;
match x { };
"#;

    let result = parse_and_validate_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, parser_errors, compile_result) = result.expect("already checked panic");
    assert!(
        parser_errors.is_empty(),
        "expected parser-clean input for pattern validation test, got: {:?}",
        parser_errors
    );

    let compile_diags = compile_result.expect_err("expected E014 from pattern validation");
    let diag = find_diag_by_code(&compile_diags, "E014");
    assert_eq!(diag.title(), "EMPTY MATCH");
    assert!(
        diag.message()
            .is_some_and(|m| m.contains("must have at least one arm")),
        "unexpected E014 message: {:?}",
        diag.message()
    );
}

#[test]
fn pattern_validation_reports_multiple_errors_without_early_abort() {
    let src = r#"
let x = 2;
match x { _ -> 1, 2 -> 2 };
match x { 1 -> 10 };
"#;

    let result = parse_and_validate_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, parser_errors, compile_result) = result.expect("already checked panic");
    assert!(
        parser_errors.is_empty(),
        "expected parser-clean input for pattern validation test, got: {:?}",
        parser_errors
    );

    let compile_diags = compile_result.expect_err("expected multiple pattern diagnostics");
    let codes = diag_codes(&compile_diags);
    assert!(
        codes.iter().any(|c| c == "E016"),
        "expected E016 in multi-error pattern validation output, got: {:?}",
        codes
    );
    assert!(
        codes.iter().any(|c| c == "E015"),
        "expected E015 in multi-error pattern validation output, got: {:?}",
        codes
    );
    assert!(
        compile_diags.len() >= 2,
        "expected validator to continue and report multiple diagnostics, got {}",
        compile_diags.len()
    );
}

#[test]
fn dedicated_pattern_validation_pass_accepts_valid_nested_patterns() {
    let src = r#"
let x = Some(Left(1));
match x { Some(Left(v)) -> v, _ -> 0 };
"#;

    let result = parse_and_validate_patterns_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, all_diags) = result.expect("already checked panic");
    assert!(
        all_diags.is_empty(),
        "expected no diagnostics for valid nested pattern validation input, got: {:?}",
        all_diags
    );
}

#[test]
fn dedicated_pattern_validation_pass_reports_multiple_match_issues() {
    let src = r#"
let x = 2;
match x { };
match x { _ -> 1, 2 -> 2 };
match x { 1 -> 10 };
"#;

    let result = parse_and_validate_patterns_no_panic(src);
    assert!(result.is_ok(), "unexpected panic: {:?}", result.err());

    let (_program, all_diags) = result.expect("already checked panic");
    let codes = diag_codes(&all_diags);
    assert!(
        codes.iter().any(|c| c == "E014"),
        "expected E014 (empty match), got: {:?}",
        codes
    );
    assert!(
        codes.iter().any(|c| c == "E016"),
        "expected E016 (catch-all not last), got: {:?}",
        codes
    );
    assert!(
        codes.iter().any(|c| c == "E015"),
        "expected E015 (non-exhaustive), got: {:?}",
        codes
    );
}
