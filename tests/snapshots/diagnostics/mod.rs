mod diagnostics_env;

use flux::bytecode::compiler::Compiler;
use flux::diagnostics::{
    Diagnostic, DiagnosticBuilder, DiagnosticsAggregator, ErrorType,
    position::{Position, Span},
};
use flux::syntax::{lexer::Lexer, parser::Parser};
use insta::assert_snapshot;

fn span(line: usize, column: usize, end_line: usize, end_column: usize) -> Span {
    Span::new(
        Position::new(line, column),
        Position::new(end_line, end_column),
    )
}

fn compile_fixture_diagnostics(file_path: &str, source: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "expected parser-clean fixture `{file_path}`, got parser errors: {:?}",
        parser.errors
    );

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    compiler
        .compile(&program)
        .expect_err("expected compile diagnostics for snapshot fixture")
}

fn render_fixture_diagnostic(
    file_path: &str,
    source: &str,
    code: &str,
    message_contains: &str,
) -> String {
    let diagnostics = compile_fixture_diagnostics(file_path, source);
    let diag = diagnostics
        .iter()
        .find(|d| {
            d.code() == Some(code)
                && d.message()
                    .is_some_and(|message| message.contains(message_contains))
        })
        .unwrap_or_else(|| {
            panic!(
                "expected diagnostic `{code}` containing `{message_contains}` for fixture `{file_path}`, got: {:?}",
                diagnostics
                    .iter()
                    .map(|d| (d.code(), d.title().to_string(), d.message().unwrap_or("")))
                    .collect::<Vec<_>>()
            )
        });
    diag.render(Some(source), Some(file_path))
}

#[test]
fn snapshot_aggregated_output() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let source_a = "let x = 1;\n";
    let source_b = "let y = 2;\n";

    let diag_a = Diagnostic::warning("TEST A")
        .with_code("E100")
        .with_error_type(ErrorType::Compiler)
        .with_message("message a")
        .with_file("a.flx")
        .with_span(span(1, 4, 1, 5));
    let diag_b = Diagnostic::warning("TEST B")
        .with_code("W200")
        .with_message("message b")
        .with_file("b.flx")
        .with_span(span(1, 4, 1, 5));

    let output = DiagnosticsAggregator::new(&[diag_a, diag_b])
        .with_source("a.flx", source_a)
        .with_source("b.flx", source_b)
        .report()
        .rendered;

    assert_snapshot!(output);
}

#[test]
fn snapshot_multi_line_span_output() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let source = "let x = 1;\nlet y = 2;\n";
    let diag = Diagnostic::warning("MULTI")
        .with_code("E123")
        .with_error_type(ErrorType::Compiler)
        .with_message("multi")
        .with_file("multi.flx")
        .with_span(span(1, 4, 2, 5));

    let output = DiagnosticsAggregator::new(&[diag])
        .with_source("multi.flx", source)
        .report()
        .rendered;

    assert_snapshot!(output);
}

#[test]
fn snapshot_colorized_output() {
    let (_lock, _guard) = diagnostics_env::with_no_color(None);

    let source = "let x = 1;\n";
    let diag = Diagnostic::warning("COLOR")
        .with_code("E123")
        .with_error_type(ErrorType::Compiler)
        .with_message("message")
        .with_file("test.flx")
        .with_span(span(1, 4, 1, 5));

    let output = diag.render(Some(source), None);

    assert_snapshot!(output);
}

#[test]
fn snapshot_0058_call_arg_named_fn_full_rendered() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let file_path = "examples/type_system/failing/110_call_arg_named_fn.flx";
    let source = include_str!("../../../examples/type_system/failing/110_call_arg_named_fn.flx");

    let output = render_fixture_diagnostic(
        file_path,
        source,
        "E300",
        "I found the wrong type in the 1st argument to `greet`.",
    );

    assert_snapshot!(output);
}

#[test]
fn snapshot_0058_let_annotation_dual_span_full_rendered() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let file_path = "examples/type_system/failing/106_let_annotation_int_string.flx";
    let source =
        include_str!("../../../examples/type_system/failing/106_let_annotation_int_string.flx");

    let output = render_fixture_diagnostic(
        file_path,
        source,
        "E300",
        "does not match its type annotation",
    );

    assert_snapshot!(output);
}

#[test]
fn snapshot_0058_return_mismatch_dual_span_full_rendered() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let file_path = "examples/type_system/failing/108_fun_return_string_vs_int.flx";
    let source =
        include_str!("../../../examples/type_system/failing/108_fun_return_string_vs_int.flx");

    let output = render_fixture_diagnostic(
        file_path,
        source,
        "E300",
        "The body of `bad` does not match its declared return type.",
    );

    assert_snapshot!(output);
}
