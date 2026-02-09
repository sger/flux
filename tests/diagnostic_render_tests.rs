mod diagnostics_env;

use flux::syntax::{
    diagnostics::{Diagnostic, DiagnosticBuilder},
    position::{Position, Span},
};

#[test]
fn render_uses_span_for_caret() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    let source = "let x = 1;\nlet y = x + 2;\n";
    let span = Span::new(Position::new(2, 8), Position::new(2, 9)); // points at 'x'
    let diag = Diagnostic::warning("TEST")
        .with_code("E999")
        .with_file("test.flx")
        .with_span(span)
        .with_message("span test");

    let out = diag.render(Some(source), Some("test.flx"));

    assert!(out.contains("  --> test.flx:2:9"));
    assert!(out.contains("2 | let y = x + 2;"));
    let lines: Vec<&str> = out.lines().collect();
    let snippet_idx = lines
        .iter()
        .position(|line| line.contains("2 | let y = x + 2;"))
        .expect("missing snippet line");
    let caret_line = lines.get(snippet_idx + 1).expect("missing caret line");
    let caret_col = caret_line.find('^').expect("missing caret");
    let snippet_line = lines[snippet_idx];
    let x_col = snippet_line.find("x + 2;").expect("missing x in snippet");
    assert_eq!(caret_col, x_col);
}
