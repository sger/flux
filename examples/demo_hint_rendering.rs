/// Direct demonstration of hint positioning feature
/// Run with: cargo run --example demo_hint_rendering

use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType, Hint},
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
