use flux::frontend::{
    diagnostics::Diagnostic,
    position::{Position, Span},
};

#[test]
fn renders_multi_line_span() {
    let source = "let x = 1;\nlet y = 2;\n";
    let span = Span::new(Position::new(1, 4), Position::new(2, 5));
    let output = Diagnostic::error("TEST")
        .with_span(span)
        .render(Some(source), Some("test.flx"));

    assert!(output.contains("1 | let x = 1;"));
    assert!(output.contains("2 | let y = 2;"));
    assert!(output.matches('^').count() > 1);
}
