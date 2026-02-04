mod diagnostics_env;

use flux::frontend::{
    diagnostics::{
        Diagnostic, ErrorType, Hint, HintChain, HintKind, InlineSuggestion, Label, LabelStyle,
    },
    position::{Position, Span},
};

#[test]
fn renders_hint_without_double_prefix() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
--> compiler error[E123]: TEST

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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
--> compiler error[E123]: TEST

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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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
    let example_pos = output
        .find("Example:\n")
        .expect("Example section not found");

    // Verify order: Hint < Note < Help < Example
    assert!(hint_pos < note_pos, "Hint should come before Note");
    assert!(note_pos < help_pos, "Note should come before Help");
    assert!(help_pos < example_pos, "Help should come before Example");
}

#[test]
fn builder_methods_for_categorized_hints() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
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

#[test]
fn hint_with_different_file() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let main_source = "calculate(x, y, z)\n";
    let main_span = Span::new(Position::new(1, 0), Position::new(1, 18));
    let lib_span = Span::new(Position::new(8, 4), Position::new(8, 9));

    let hint = Hint::at("Function defined with 2 parameters", lib_span)
        .with_label("defined here")
        .with_file("src/lib.flx");

    let output = Diagnostic::error("Function signature mismatch")
        .with_code("E050")
        .with_file("src/main.flx")
        .with_span(main_span)
        .with_message("Expected 2 arguments, found 3")
        .with_hint(hint)
        .render(Some(main_source), None);

    // Check that both files are mentioned
    assert!(output.contains("src/main.flx"));
    assert!(output.contains("src/lib.flx"));
    assert!(output.contains("defined here"));
}

#[test]
fn hint_with_file_builder() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let _span = Span::new(Position::new(10, 5), Position::new(10, 15));

    let hint = Hint::note("This was imported from another module").with_file("src/module.flx");

    assert_eq!(hint.file, Some("src/module.flx".to_string()));
    assert_eq!(hint.kind, HintKind::Note);
}

#[test]
fn hint_falls_back_to_diagnostic_file() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let source = "let x = 10;\n";
    let span = Span::new(Position::new(1, 4), Position::new(1, 5));

    // Hint without explicit file should use diagnostic's file
    let hint = Hint::at("variable defined here", span).with_label("first definition");

    let output = Diagnostic::error("Duplicate variable")
        .with_file("test.flx")
        .with_span(span)
        .with_hint(hint)
        .render(Some(source), None);

    // Should use diagnostic's file (test.flx)
    assert!(output.contains("test.flx"));
}

#[test]
fn inline_suggestion_basic() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let source = "fn calculate(x) { x + 1 }\n";
    let span = Span::new(Position::new(1, 0), Position::new(1, 2));

    let suggestion = InlineSuggestion::new(span, "fun");

    let output = Diagnostic::error("Unknown keyword")
        .with_code("E101")
        .with_file("test.flx")
        .with_span(span)
        .with_message("'fn' is not a valid keyword")
        .with_suggestion(suggestion)
        .render(Some(source), None);

    // Check that suggestion is rendered
    assert!(output.contains("help: Replace with 'fun'"));
    assert!(output.contains("fun calculate(x) { x + 1 }"));
    assert!(output.contains("~~~"));
}

#[test]
fn inline_suggestion_with_message() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let source = "fn main() {}\n";
    let span = Span::new(Position::new(1, 0), Position::new(1, 2));

    let output = Diagnostic::error("Invalid keyword")
        .with_span(span)
        .with_suggestion_message(span, "fun", "Use 'fun' instead of 'fn'")
        .render(Some(source), None);

    assert!(output.contains("help: Use 'fun' instead of 'fn'"));
    assert!(output.contains("fun main() {}"));
}

#[test]
fn inline_suggestion_builder() {
    let span = Span::new(Position::new(1, 0), Position::new(1, 5));
    let suggestion = InlineSuggestion::new(span, "const").with_message("Use const for constants");

    assert_eq!(suggestion.replacement, "const");
    assert_eq!(
        suggestion.message,
        Some("Use const for constants".to_string())
    );
}

#[test]
fn multiple_inline_suggestions() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let source = "fn add(a, b) { a + b }\n";
    let fn_span = Span::new(Position::new(1, 0), Position::new(1, 2));

    let output = Diagnostic::error("Multiple issues")
        .with_span(fn_span)
        .with_suggestion_replace(fn_span, "fun")
        .render(Some(source), None);

    assert!(output.contains("fun add(a, b)"));
    assert!(output.contains("~~~"));
}

#[test]
fn hint_chain_basic() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let chain = HintChain::new(vec![
        "Convert the String to Int using .parse()".to_string(),
        "Handle the potential parse error".to_string(),
        "Or change the function signature".to_string(),
    ]);

    let output = Diagnostic::error("Type mismatch")
        .with_code("E020")
        .with_hint_chain(chain)
        .render(None, Some("test.flx"));

    assert!(output.contains("Hint:"));
    assert!(output.contains("To fix this error:"));
    assert!(output.contains("1. Convert the String to Int using .parse()"));
    assert!(output.contains("2. Handle the potential parse error"));
    assert!(output.contains("3. Or change the function signature"));
}

#[test]
fn hint_chain_with_conclusion() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let chain = HintChain::from_steps(vec![
        "Check the variable type",
        "Ensure it matches the expected type",
    ])
    .with_conclusion("Type annotations can help prevent these errors");

    let output = Diagnostic::error("Type error")
        .with_hint_chain(chain)
        .render(None, Some("test.flx"));

    assert!(output.contains("1. Check the variable type"));
    assert!(output.contains("2. Ensure it matches the expected type"));
    assert!(output.contains("Type annotations can help prevent these errors"));
}

#[test]
fn hint_chain_builder_methods() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    // Test with_steps
    let output1 = Diagnostic::error("Error")
        .with_steps(vec!["Step 1", "Step 2"])
        .render(None, Some("test.flx"));

    assert!(output1.contains("1. Step 1"));
    assert!(output1.contains("2. Step 2"));

    // Test with_steps_and_conclusion
    let output2 = Diagnostic::error("Error")
        .with_steps_and_conclusion(
            vec!["Fix step 1", "Fix step 2"],
            "This should resolve the issue",
        )
        .render(None, Some("test.flx"));

    assert!(output2.contains("1. Fix step 1"));
    assert!(output2.contains("2. Fix step 2"));
    assert!(output2.contains("This should resolve the issue"));
}

#[test]
fn multiple_hint_chains() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));
    let chain1 = HintChain::from_steps(vec!["Option 1 step 1", "Option 1 step 2"]);
    let chain2 = HintChain::from_steps(vec!["Option 2 step 1", "Option 2 step 2"]);

    let output = Diagnostic::error("Multiple solutions")
        .with_hint_chain(chain1)
        .with_hint_chain(chain2)
        .render(None, Some("test.flx"));

    // Both chains should be present
    assert!(output.contains("Option 1 step 1"));
    assert!(output.contains("Option 2 step 1"));
}

#[test]
fn make_warning_from_code() {
    use flux::frontend::diagnostics::{ErrorCode, ErrorType};

    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    const TEST_WARNING: ErrorCode = ErrorCode {
        code: "W001",
        title: "Unused variable",
        message: "Variable '{}' is declared but never used",
        hint: Some("Remove the variable or prefix it with '_'"),
        error_type: ErrorType::Compiler,
    };

    let output = Diagnostic::make_warning_from_code(
        &TEST_WARNING,
        &["count"],
        "test.flx",
        Span::new(Position::new(5, 4), Position::new(5, 9)),
    )
    .render(None, None);

    assert!(output.contains("--> warning[W001]: Unused variable"));
    assert!(output.contains("W001"));
    assert!(output.contains("Unused variable"));
    assert!(output.contains("Variable 'count' is declared but never used"));
    assert!(output.contains("Remove the variable or prefix it with '_'"));
}

#[test]
fn all_severity_levels() {
    let (_lock, _guard) = diagnostics_env::with_no_color(Some("1"));

    // Test all severity levels can be created
    let error = Diagnostic::error("Error title").render(None, Some("test.flx"));
    assert!(error.contains("--> error[E000]: Error title"));

    let warning = Diagnostic::warning("Warning title").render(None, Some("test.flx"));
    assert!(warning.contains("--> warning[E000]: Warning title"));

    let span = Span::new(Position::new(1, 0), Position::new(1, 5));

    let note =
        Diagnostic::make_note("Note title", "This is a note", "test.flx", span).render(None, None);
    assert!(note.contains("--> note[E000]: Note title"));

    let help =
        Diagnostic::make_help("Help title", "This is help", "test.flx", span).render(None, None);
    assert!(help.contains("--> help[E000]: Help title"));
}
