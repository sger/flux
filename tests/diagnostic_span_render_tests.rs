use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType, Hint, HintKind, Label, LabelStyle},
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

#[test]
fn renders_single_label() {
    set_no_color();
    let source = "add(name, age)\n";
    let span = Span::new(Position::new(1, 0), Position::new(1, 14));
    let label_span = Span::new(Position::new(1, 4), Position::new(1, 8));

    let output = Diagnostic::error("Type mismatch")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_file("test.flx")
        .with_span(span)
        .with_secondary_label(label_span, "expected Int, got String")
        .render(Some(source), None);

    // Should show primary caret
    assert!(output.contains("^^^^^^^^^^^^^^"));
    // Should show secondary label with dashes
    assert!(output.contains("----"));
    // Should show label text
    assert!(output.contains("expected Int, got String"));
}

#[test]
fn renders_multiple_labels_on_same_line() {
    set_no_color();
    let source = "add(name, age)\n";
    let span = Span::new(Position::new(1, 0), Position::new(1, 14));
    let label1_span = Span::new(Position::new(1, 4), Position::new(1, 8));
    let label2_span = Span::new(Position::new(1, 10), Position::new(1, 13));

    let output = Diagnostic::error("Type mismatch")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_file("test.flx")
        .with_span(span)
        .with_secondary_label(label1_span, "String value")
        .with_note_label(label2_span, "expected Int")
        .render(Some(source), None);

    // Should show primary caret
    assert!(output.contains("^^^^^^^^^^^^^^"));
    // Should show both labels
    assert!(output.contains("String value"));
    assert!(output.contains("expected Int"));
    // Should have dashes for both labels (checking for at least one dash followed by text)
    assert!(output.contains("---- String value"));
    assert!(output.contains("--- expected Int"));
}

#[test]
fn label_builder_methods() {
    let span = Span::new(Position::new(1, 0), Position::new(1, 5));

    // Test Label::primary()
    let label1 = Label::primary(span, "main error");
    assert_eq!(label1.span, span);
    assert_eq!(label1.text, "main error");
    assert_eq!(label1.style, LabelStyle::Primary);

    // Test Label::secondary()
    let label2 = Label::secondary(span, "additional context");
    assert_eq!(label2.span, span);
    assert_eq!(label2.text, "additional context");
    assert_eq!(label2.style, LabelStyle::Secondary);

    // Test Label::note()
    let label3 = Label::note(span, "informational");
    assert_eq!(label3.span, span);
    assert_eq!(label3.text, "informational");
    assert_eq!(label3.style, LabelStyle::Note);
}

#[test]
fn renders_labels_without_primary_span_on_same_line() {
    set_no_color();
    let source = "let x = add(name, age);\n";
    // Primary span is on the entire line
    let span = Span::new(Position::new(1, 0), Position::new(1, 23));
    // Labels are on specific parts
    let label1_span = Span::new(Position::new(1, 12), Position::new(1, 16));
    let label2_span = Span::new(Position::new(1, 18), Position::new(1, 21));

    let output = Diagnostic::error("Type error in function call")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_file("test.flx")
        .with_span(span)
        .with_secondary_label(label1_span, "String")
        .with_secondary_label(label2_span, "requires Int")
        .render(Some(source), None);

    // Should show the source line once
    assert_eq!(output.matches("let x = add(name, age);").count(), 1);
    // Should show both label texts
    assert!(output.contains("String"));
    assert!(output.contains("requires Int"));
}

#[test]
fn diagnostic_with_label_method() {
    set_no_color();
    let source = "foo(x, y)\n";
    let span = Span::new(Position::new(1, 0), Position::new(1, 9));
    let label_span = Span::new(Position::new(1, 4), Position::new(1, 5));
    let label = Label::secondary(label_span, "wrong type");

    let output = Diagnostic::error("Type error")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_file("test.flx")
        .with_span(span)
        .with_label(label)
        .render(Some(source), None);

    assert!(output.contains("wrong type"));
}

#[test]
fn renders_categorized_hints() {
    set_no_color();
    let output = Diagnostic::error("Test error")
        .with_note("This is a note")
        .with_help("This is help")
        .with_example("let x = 10;")
        .with_hint_text("This is a hint")
        .render(None, Some("test.flx"));

    // Check all hint types are present
    assert!(output.contains("\n\nNote:\n"));
    assert!(output.contains("  This is a note\n"));
    assert!(output.contains("\n\nHelp:\n"));
    assert!(output.contains("  This is help\n"));
    assert!(output.contains("\n\nExample:\n"));
    assert!(output.contains("  let x = 10;\n"));
    assert!(output.contains("\n\nHint:\n"));
    assert!(output.contains("  This is a hint\n"));
}

#[test]
fn hint_kind_constructors() {
    // Test that different constructors create correct hint kinds
    let hint = Hint::note("note text");
    assert_eq!(hint.kind, HintKind::Note);
    assert_eq!(hint.text, "note text");

    let help = Hint::help("help text");
    assert_eq!(help.kind, HintKind::Help);
    assert_eq!(help.text, "help text");

    let example = Hint::example("example text");
    assert_eq!(example.kind, HintKind::Example);
    assert_eq!(example.text, "example text");

    let regular = Hint::text("regular hint");
    assert_eq!(regular.kind, HintKind::Hint);
    assert_eq!(regular.text, "regular hint");
}

#[test]
fn categorized_hints_ordering() {
    set_no_color();
    // Hints should be rendered in order: Hint, Note, Help, Example
    let output = Diagnostic::error("Test")
        .with_example("example")
        .with_hint_text("hint")
        .with_help("help")
        .with_note("note")
        .render(None, Some("test.flx"));

    // Find positions of each section
    let hint_pos = output.find("Hint:\n").expect("Hint section not found");
    let note_pos = output.find("Note:\n").expect("Note section not found");
    let help_pos = output.find("Help:\n").expect("Help section not found");
    let example_pos = output.find("Example:\n").expect("Example section not found");

    // Verify order: Hint < Note < Help < Example
    assert!(hint_pos < note_pos, "Hint should come before Note");
    assert!(note_pos < help_pos, "Note should come before Help");
    assert!(help_pos < example_pos, "Help should come before Example");
}

#[test]
fn builder_methods_for_categorized_hints() {
    set_no_color();
    let output = Diagnostic::error("Variable name error")
        .with_code("E015")
        .with_note("Variables must follow naming conventions")
        .with_help("Use camelCase or snake_case")
        .with_example("let myVariable = 10;\nlet my_variable = 20;")
        .render(None, Some("test.flx"));

    assert!(output.contains("Note:"));
    assert!(output.contains("Variables must follow naming conventions"));
    assert!(output.contains("Help:"));
    assert!(output.contains("Use camelCase or snake_case"));
    assert!(output.contains("Example:"));
    assert!(output.contains("let myVariable = 10;"));
}
