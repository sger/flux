/// Direct demonstration of hint positioning feature
/// Run with: cargo run --example demo_hint_rendering

use flux::frontend::{
    diagnostics::{Diagnostic, ErrorType, Hint, InlineSuggestion},
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

    // Example 6: Categorized hints (Note, Help, Example)
    example_categorized_hints();

    // Example 7: Multi-file support (cross-file references)
    example_multi_file_hints();

    // Example 8: Inline suggestions (code fixes)
    example_inline_suggestions();
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

fn example_categorized_hints() {
    println!("\n📍 Example 6: Categorized Hints (NEW FEATURE)");
    println!("{}", "-".repeat(70));

    let source = "\
let myVariable = 10;
let MyOtherVariable = 20;
";

    let error_span = Span::new(Position::new(2, 4), Position::new(2, 19));

    let diagnostic = Diagnostic::error("Invalid variable name")
        .with_code("E015")
        .with_error_type(ErrorType::Compiler)
        .with_message("Variable names must start with a lowercase letter")
        .with_file("example.flx")
        .with_span(error_span)
        .with_note("Variables are case-sensitive in Flux")
        .with_help("Change 'MyOtherVariable' to start with a lowercase letter")
        .with_example("let myVariable = 10;\nlet myOtherVariable = 20;");

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("✨ NEW: Hints are now categorized by type!");
    println!("   • Note (cyan) - Additional context or information");
    println!("   • Help (green) - Explicit instructions on how to fix");
    println!("   • Example (blue) - Code examples demonstrating the solution");
    println!("   • Hint (blue) - General hints or suggestions");
    println!();
    println!("💡 Benefits:");
    println!("   • Easier to scan and find the information you need");
    println!("   • Clear distinction between context, guidance, and examples");
    println!("   • Consistent formatting across all error messages");
}

fn example_multi_file_hints() {
    println!("\n📍 Example 7: Multi-File Support (NEW FEATURE)");
    println!("{}", "-".repeat(70));

    // Simulate code in main.flx calling a function from lib.flx
    let main_source = "\
calculate(x, y, z)
";

    let lib_source = "\
// Library file
fun calculate(a, b) {
    return a + b;
}
";

    // Error in main.flx
    let call_span = Span::new(Position::new(1, 0), Position::new(1, 18));
    
    // Function definition in lib.flx
    let def_span = Span::new(Position::new(2, 14), Position::new(2, 20));

    // Create hint that points to a different file
    let hint = Hint::at("Function defined with 2 parameters here", def_span)
        .with_label("defined with 2 parameters")
        .with_file("src/lib.flx");

    let diagnostic = Diagnostic::error("Function signature mismatch")
        .with_code("E050")
        .with_error_type(ErrorType::Compiler)
        .with_message("Expected 2 arguments, found 3")
        .with_file("src/main.flx")
        .with_span(call_span)
        .with_hint(hint);

    println!("{}\n", diagnostic.render(Some(main_source), None));

    println!("✨ NEW: Hints can reference code in different files!");
    println!("   • Main error shows location in src/main.flx");
    println!("   • Hint points to function definition in src/lib.flx");
    println!("   • Perfect for module imports and cross-file references");
    println!();
    println!("💡 Use cases:");
    println!("   • Function signature mismatches across modules");
    println!("   • Type definitions in other files");
    println!("   • Variable declarations in imported modules");
    println!("   • Any cross-file reference that helps explain an error");
}

fn example_inline_suggestions() {
    println!("\n📍 Example 8: Inline Suggestions (NEW FEATURE)");
    println!("{}", "-".repeat(70));

    let source = "\
fn calculate(x, y) {
    return x + y;
}
";

    let error_span = Span::new(Position::new(1, 0), Position::new(1, 2));
    let suggestion = InlineSuggestion::new(error_span, "fun")
        .with_message("Use 'fun' for function declarations");

    let diagnostic = Diagnostic::error("Unknown keyword")
        .with_code("E101")
        .with_error_type(ErrorType::Compiler)
        .with_message("Flux uses 'fun' for function declarations")
        .with_file("example.flx")
        .with_span(error_span)
        .with_suggestion(suggestion);

    println!("{}\n", diagnostic.render(Some(source), None));

    println!("🎉 NEW: Inline suggestions show how to fix the code!");
    println!("   This is like Rust's compiler suggestions");
    println!("   • Shows the exact fix inline with the error");
    println!("   • Uses tildes (~~~) to highlight the replacement");
    println!("   • Improves reading flow - fix is right there");
    println!();
    println!("💡 Perfect for:");
    println!("   • Keyword typos (fn → fun)");
    println!("   • Syntax corrections");
    println!("   • Simple find-and-replace fixes");
    println!("   • Any fix that can be shown as a text replacement");
}
