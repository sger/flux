mod diagnostics_env;

use flux::frontend::{
    diagnostics::{Diagnostic, EXPECTED_EXPRESSION, ErrorType, Severity},
    position::{Position, Span},
};

fn render_with_color(diag: &Diagnostic, source: &str, file: &str, color: bool) -> String {
    let (_lock, _guard) = if color {
        diagnostics_env::with_no_color(None)
    } else {
        diagnostics_env::with_no_color(Some("1"))
    };
    diag.render(Some(source), Some(file))
}

fn span(line: usize, column: usize, end_line: usize, end_column: usize) -> Span {
    Span::new(
        Position::new(line, column),
        Position::new(end_line, end_column),
    )
}

#[test]
fn make_error_sets_expected_fields() {
    let span = Span::new(Position::new(1, 0), Position::new(1, 1));
    let diag = Diagnostic::make_error(&EXPECTED_EXPRESSION, &["';'"], "test.flx", span);

    assert_eq!(diag.severity(), Severity::Error);
    assert_eq!(diag.code(), Some("E031"));
    assert_eq!(diag.error_type(), Some(ErrorType::Compiler));
    assert_eq!(diag.message(), Some("Expected expression, found ';'."));
    assert_eq!(diag.file(), Some("test.flx"));
    assert_eq!(diag.span(), Some(span));
    assert!(diag.hints().is_empty());
    assert!(diag.related().is_empty());
}

#[test]
fn empty_span_renders_single_caret() {
    let source = "let x = 1;\n";
    let diag = Diagnostic::error("EMPTY SPAN")
        .with_file("test.flx")
        .with_span(span(1, 4, 1, 4));

    let out = render_with_color(&diag, source, "test.flx", false);
    assert!(out.contains("1 | let x = 1;"));
    assert!(out.contains("^"));
}

#[test]
fn missing_span_renders_without_location() {
    let source = "let x = 1;\n";
    let diag = Diagnostic::error("NO SPAN")
        .with_file("test.flx")
        .with_message("no span message");

    let out = render_with_color(&diag, source, "test.flx", false);
    assert!(out.contains("--> error[E000]: NO SPAN"));
    assert!(out.contains("no span message"));
    assert!(!out.contains("  --> test.flx:"));
}

#[test]
fn span_at_beginning_and_end_of_file() {
    let source = "first\nlast";

    let begin = Diagnostic::error("BEGIN")
        .with_file("test.flx")
        .with_span(span(1, 0, 1, 1));
    let out_begin = render_with_color(&begin, source, "test.flx", false);
    assert!(out_begin.contains("1 | first"));
    assert!(out_begin.contains("^"));

    let end = Diagnostic::error("END")
        .with_file("test.flx")
        .with_span(span(2, 4, 2, 4));
    let out_end = render_with_color(&end, source, "test.flx", false);
    assert!(out_end.contains("2 | last"));
    assert!(out_end.contains("^"));
}

#[test]
fn span_on_blank_line_renders() {
    let source = "line1\n   \nline3\n";
    let diag = Diagnostic::error("BLANK")
        .with_file("test.flx")
        .with_span(span(2, 1, 2, 2));

    let out = render_with_color(&diag, source, "test.flx", false);
    assert!(out.contains("2 |    "));
    assert!(out.contains("^"));
}

#[test]
fn multi_line_span_renders_multiple_lines() {
    let source = "let x = 1;\nlet y = 2;\nlet z = 3;\n";
    let diag = Diagnostic::error("MULTI")
        .with_file("test.flx")
        .with_span(span(1, 4, 2, 5));

    let out = render_with_color(&diag, source, "test.flx", false);
    assert!(out.contains("1 | let x = 1;"));
    assert!(out.contains("2 | let y = 2;"));
    assert!(out.matches('^').count() >= 2);
}

#[test]
fn multi_line_labels_render_on_each_line() {
    let source = "let x = 1;\nlet y = 2;\n";
    let diag = Diagnostic::error("LABELS")
        .with_file("test.flx")
        .with_span(span(1, 4, 1, 5))
        .with_primary_label(span(1, 4, 1, 5), "first label")
        .with_secondary_label(span(2, 4, 2, 5), "second label");

    let out = render_with_color(&diag, source, "test.flx", false);
    assert!(out.contains("first label"));
    assert!(out.contains("second label"));
}

#[test]
fn color_output_toggle() {
    let source = "let x = 1;\n";
    let diag = Diagnostic::error("COLOR")
        .with_file("test.flx")
        .with_span(span(1, 0, 1, 1));

    let no_color = render_with_color(&diag, source, "test.flx", false);
    assert!(!no_color.contains("\u{1b}["));

    let color = render_with_color(&diag, source, "test.flx", true);
    assert!(color.contains("\u{1b}["));
}
