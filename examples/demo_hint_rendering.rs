/// Direct demonstration of hint positioning feature
/// Run with: cargo run --example demo_hint_rendering

use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType},
    position::{Position, Span},
};

fn main() {
    println!("\n🎯 Hint Positioning Feature Demo\n");
    println!("This shows the NEW multi-location hint feature in action.\n");
    println!("{}", "=".repeat(70));

    // Example 1: What duplicate variable errors WILL look like
    // once the compiler is updated to use this feature
    example_duplicate_variable();

    // Example 2: Current working parser error with hint
    example_parser_error();

    // Example 3: Multiple hints showing complex scenarios
    example_complex_hints();

    // Example 4: NEW - Inline labels (like Rust compiler)
    example_inline_labels();

    // Example 5: Unknown operator with inline labels (real compiler integration)
    example_unknown_operator();
}

fn example_duplicate_variable() {
    println!("\n📍 Example 1: Duplicate Variable (Multi-Location Error)");
    println!("{}", "-".repeat(70));

    let source = "\
let x = 10;

let y = 20;

// Oops! 'x' is already defined above
let x = 30;

x + y
";

    let error_span = Span::new(Position::new(6, 4), Position::new(6, 5));
    let first_def_span = Span::new(Position::new(1, 4), Position::new(1, 5));

    let diagnostic = Diagnostic::error("Duplicate variable")
        .with_code("E001")
        .with_error_type(ErrorType::Compiler)
        .with_message("Variable 'x' is already defined in this scope")
        .with_file("example.flx")
        .with_span(error_span)
        .with_hint_text("Use a different name or remove the previous definition")
        .with_hint_labeled("", first_def_span, "first defined here");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("✅ Notice how the error shows BOTH locations:");
    println!("   • Line 6: Where the duplicate occurs");
    println!("   • Line 1: Where 'x' was first defined");
    println!();
}

fn example_parser_error() {
    println!("\n📍 Example 2: Parser Error (Real Current Error)");
    println!("{}", "-".repeat(70));

    let source = "\
// Using 'fn' instead of 'fun'
fn calculate(x) {
    x * 2
}
";

    let error_span = Span::new(Position::new(2, 0), Position::new(2, 2));

    let diagnostic = Diagnostic::error("Unknown keyword")
        .with_code("E101")
        .with_error_type(ErrorType::Compiler)
        .with_message("Flux uses `fun` for function declarations")
        .with_file("example.flx")
        .with_span(error_span)
        .with_hint_text("Replace 'fn' with 'fun'");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("ℹ️  This is a real parser error from the current compiler.");
    println!("   It uses a simple text hint (no additional location needed).");
    println!();
}

fn example_complex_hints() {
    println!("\n📍 Example 3: Type Mismatch (Multiple Hints)");
    println!("{}", "-".repeat(70));

    let source = "\
let name = \"Alice\";
let age = 25;
let city = \"NYC\";

// Type error: can't add String and Int
let result = name + age;
";

    let error_span = Span::new(Position::new(6, 13), Position::new(6, 23));
    let name_span = Span::new(Position::new(1, 4), Position::new(1, 8));
    let age_span = Span::new(Position::new(2, 4), Position::new(2, 7));

    let diagnostic = Diagnostic::error("Type mismatch")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_message("Cannot add String and Int")
        .with_file("example.flx")
        .with_span(error_span)
        .with_hint_text("Consider converting types or using string concatenation")
        .with_hint_labeled("", name_span, "'name' has type String")
        .with_hint_labeled("", age_span, "'age' has type Int");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("✨ Notice this error shows THREE locations:");
    println!("   • Line 6: The problematic operation");
    println!("   • Line 1: Where 'name' (String) was defined");
    println!("   • Line 2: Where 'age' (Int) was defined");
    println!();
}

fn example_inline_labels() {
    println!("\n📍 Example 4: Inline Labels (NEW FEATURE - Rust-Style)");
    println!("{}", "-".repeat(70));

    let source = "\
add(name, age)
";

    // Main error spans the whole function call
    let error_span = Span::new(Position::new(1, 0), Position::new(1, 14));
    // Label for first argument
    let arg1_span = Span::new(Position::new(1, 4), Position::new(1, 8));
    // Label for second argument
    let arg2_span = Span::new(Position::new(1, 10), Position::new(1, 13));

    let diagnostic = Diagnostic::error("Type mismatch in function call")
        .with_code("E020")
        .with_error_type(ErrorType::Compiler)
        .with_message("Function `add` expects (Int, Int) but got (String, Int)")
        .with_file("example.flx")
        .with_span(error_span)
        .with_secondary_label(arg1_span, "String value")
        .with_secondary_label(arg2_span, "expected Int");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("🎉 NEW: Inline labels annotate specific parts of the same line!");
    println!("   This is like Rust's compiler error messages.");
    println!("   • Primary caret (^^^) shows the whole problematic expression");
    println!("   • Secondary labels (---) point to specific arguments");
    println!("   • Each label explains what's wrong with that part");
    println!();
    println!("💡 You can add multiple labels with different styles:");
    println!("   • Primary labels (red) - main error location");
    println!("   • Secondary labels (blue) - additional context");
    println!("   • Note labels (cyan) - informational hints");
}

fn example_unknown_operator() {
    println!("\n📍 Example 5: Unknown Operator (Real Compiler Integration)");
    println!("{}", "-".repeat(70));

    let source = "\
let result = x ~ y;
";

    // Main error spans the whole infix expression
    let error_span = Span::new(Position::new(1, 13), Position::new(1, 18));
    // Label for left operand
    let left_span = Span::new(Position::new(1, 13), Position::new(1, 14));
    // Label for right operand
    let right_span = Span::new(Position::new(1, 17), Position::new(1, 18));

    let diagnostic = Diagnostic::error("Unknown infix operator")
        .with_code("E006")
        .with_error_type(ErrorType::Compiler)
        .with_message("The operator '~' is not recognized")
        .with_file("example.flx")
        .with_span(error_span)
        .with_hint_text("Valid operators are: +, -, *, /, ==, !=, <, >, <=, >=, &&, ||")
        .with_secondary_label(left_span, "left operand")
        .with_secondary_label(right_span, "right operand");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("🔧 This example shows inline labels integrated into the compiler!");
    println!("   • This is the ACTUAL error format from src/bytecode/compiler.rs:521");
    println!("   • The compiler now uses .with_secondary_label() for operands");
    println!("   • Labels help identify which parts of the expression are problematic");
    println!();
}
