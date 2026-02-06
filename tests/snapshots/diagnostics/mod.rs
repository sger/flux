mod diagnostics_env;

use flux::frontend::{
    diagnostics::{Diagnostic, DiagnosticBuilder, DiagnosticsAggregator, ErrorType},
    position::{Position, Span},
};
use insta::assert_snapshot;

fn span(line: usize, column: usize, end_line: usize, end_column: usize) -> Span {
    Span::new(
        Position::new(line, column),
        Position::new(end_line, end_column),
    )
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
