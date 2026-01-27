use flux::frontend::{lexer::Lexer, linter::Linter, parser::Parser};

fn lint(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors in test input");
    let diagnostics = Linter::new(None).lint(&program);
    diagnostics
        .iter()
        .map(|d| format!("{}:{}", d.code.as_deref().unwrap_or(""), d.title))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn warns_on_unused_let() {
    let output = lint("let x = 1;");
    assert!(output.contains("W001:UNUSED VARIABLE"));
}

#[test]
fn warns_on_unused_param() {
    let output = lint("fun f(x) { 1; }");
    assert!(output.contains("W002:UNUSED PARAMETER"));
}

#[test]
fn warns_on_unused_import() {
    let output = lint("import Math module Main { fun main() { 1; } }");
    assert!(output.contains("W003:UNUSED IMPORT"));
}

#[test]
fn warns_on_shadowed_name() {
    let output = lint("let x = 1; fun f() { let x = 2; x; }");
    assert!(output.contains("W004:SHADOWED NAME"));
}

#[test]
fn warns_on_function_name_style() {
    let output = lint("fun NotSnakeCase() { 1; }");
    assert!(output.contains("W005:FUNCTION NAME STYLE"));
}

#[test]
fn warns_on_import_name_style() {
    let output = lint("import math module Main { fun main() { 1; } }");
    assert!(output.contains("W006:IMPORT NAME STYLE"));
}
