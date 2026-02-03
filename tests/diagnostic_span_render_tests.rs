use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType},
    position::{Position, Span},
};

fn set_no_color() {
    // set_var is unsafe in newer Rust editions due to global process mutation.
    unsafe {
        std::env::set_var("NO_COLOR", "1");
    }
}

#[test]
fn renders_hint_without_double_prefix() {
    set_no_color();
    let output = Diagnostic::error("TEST")
        .with_hint("Hint:\n  foo")
        .render(None, Some("test.flx"));

    assert!(output.contains("\n\nHint:\n  foo\n"));
    assert!(!output.contains("Hint:\n  Hint:"));
}

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

#[test]
fn renders_single_line_snapshot() {
    set_no_color();
    let source = "let x = 1;\n";
    let span = Span::new(Position::new(1, 4), Position::new(1, 5));
    let output = Diagnostic::error("TEST")
        .with_code("E123")
        .with_error_type(ErrorType::Compiler)
        .with_message("message")
        .with_file("test.flx")
        .with_span(span)
        .render(Some(source), None);

    let expected = "\
-- compiler error: test [E123]

message

  --> test.flx:1:5
  |
1 | let x = 1;
  |     ^";

    assert_eq!(output, expected);
}

#[test]
fn renders_multi_line_snapshot() {
    set_no_color();
    let source = "let x = 1;\nlet y = 2;\n";
    let span = Span::new(Position::new(1, 4), Position::new(2, 5));
    let output = Diagnostic::error("TEST")
        .with_code("E123")
        .with_error_type(ErrorType::Compiler)
        .with_message("message")
        .with_file("test.flx")
        .with_span(span)
        .render(Some(source), None);

    let expected = "\
-- compiler error: test [E123]

message

  --> test.flx:1:5
  |
1 | let x = 1;
  |     ^^^^^^
2 | let y = 2;
  | ^^^^^";

    assert_eq!(output, expected);
}
