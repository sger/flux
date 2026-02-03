use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType, Hint},
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
        .with_hint_text("Hint:\n  foo")
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
-- Compiler error: TEST [E123]

message

  --> test.flx:1:5
  |
1 | let x = 1;
  |     ^
";

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
-- Compiler error: TEST [E123]

message

  --> test.flx:1:5
  |
1 | let x = 1;
  |     ^^^^^^
2 | let y = 2;
  | ^^^^^
";

    assert_eq!(output, expected);
}

#[test]
fn renders_hint_with_span() {
    set_no_color();
    let source = "let x = 1;\nlet x = 2;\n";
    let error_span = Span::new(Position::new(2, 4), Position::new(2, 5));
    let hint_span = Span::new(Position::new(1, 4), Position::new(1, 5));

    let output = Diagnostic::error("Duplicate variable")
        .with_code("E001")
        .with_error_type(ErrorType::Compiler)
        .with_message("Variable 'x' is already defined")
        .with_file("test.flx")
        .with_span(error_span)
        .with_hint_at("first defined here", hint_span)
        .render(Some(source), None);

    // Should contain the main error location
    assert!(output.contains("  --> test.flx:2:5"));
    assert!(output.contains("2 | let x = 2;"));

    // Should contain the hint location
    assert!(output.contains("   = note:"));
    assert!(output.contains("  --> test.flx:1:5"));
    assert!(output.contains("1 | let x = 1;"));

    // Should contain the hint text
    assert!(output.contains("Hint:\n  first defined here"));
}

#[test]
fn renders_hint_with_span_and_label() {
    set_no_color();
    let source = "let x = 1;\nlet x = 2;\n";
    let error_span = Span::new(Position::new(2, 4), Position::new(2, 5));
    let hint_span = Span::new(Position::new(1, 4), Position::new(1, 5));

    let output = Diagnostic::error("Duplicate variable")
        .with_code("E001")
        .with_error_type(ErrorType::Compiler)
        .with_message("Variable 'x' is already defined")
        .with_file("test.flx")
        .with_span(error_span)
        .with_hint_labeled("Use a different name", hint_span, "first defined here")
        .render(Some(source), None);

    // Should contain the label in the note
    assert!(output.contains("   = note: first defined here"));

    // Should contain the hint location
    assert!(output.contains("  --> test.flx:1:5"));
    assert!(output.contains("1 | let x = 1;"));

    // Should contain the hint text
    assert!(output.contains("Hint:\n  Use a different name"));
}

#[test]
fn renders_multiple_hints_mixed() {
    set_no_color();
    let source = "let x = 1;\nlet x = 2;\n";
    let error_span = Span::new(Position::new(2, 4), Position::new(2, 5));
    let hint_span = Span::new(Position::new(1, 4), Position::new(1, 5));

    let output = Diagnostic::error("Duplicate variable")
        .with_code("E001")
        .with_error_type(ErrorType::Compiler)
        .with_message("Variable 'x' is already defined")
        .with_file("test.flx")
        .with_span(error_span)
        .with_hint_text("Use a different name or remove the previous definition")
        .with_hint_at("first defined here", hint_span)
        .render(Some(source), None);

    // Text-only hints should appear first
    assert!(output.contains("Hint:\n  Use a different name or remove the previous definition"));

    // Span-based hints should appear after with location
    assert!(output.contains("   = note:"));
    assert!(output.contains("  --> test.flx:1:5"));
    assert!(output.contains("1 | let x = 1;"));
}

#[test]
fn renders_hint_text_only() {
    set_no_color();
    let output = Diagnostic::error("TEST")
        .with_code("E001")
        .with_file("test.flx")
        .with_hint_text("This is a simple hint")
        .render(None, None);

    assert!(output.contains("Hint:\n  This is a simple hint"));
    assert!(!output.contains("note:"));
}

#[test]
fn hint_builder_methods() {
    // Test Hint::text()
    let hint1 = Hint::text("simple hint");
    assert_eq!(hint1.text, "simple hint");
    assert_eq!(hint1.span, None);
    assert_eq!(hint1.label, None);

    // Test Hint::at()
    let span = Span::new(Position::new(1, 0), Position::new(1, 5));
    let hint2 = Hint::at("hint with span", span);
    assert_eq!(hint2.text, "hint with span");
    assert_eq!(hint2.span, Some(span));
    assert_eq!(hint2.label, None);

    // Test Hint::labeled()
    let hint3 = Hint::labeled("hint with label", span, "defined here");
    assert_eq!(hint3.text, "hint with label");
    assert_eq!(hint3.span, Some(span));
    assert_eq!(hint3.label, Some("defined here".to_string()));

    // Test with_label()
    let hint4 = Hint::at("hint", span).with_label("added label");
    assert_eq!(hint4.label, Some("added label".to_string()));
}
