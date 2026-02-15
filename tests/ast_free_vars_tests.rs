use flux::ast::free_vars::{collect_free_vars, collect_free_vars_in_program};
use flux::syntax::{lexer::Lexer, parser::Parser, program::Program};

fn parse(input: &str) -> (Program, flux::syntax::interner::Interner) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "Parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn free_var_names(input: &str) -> Vec<String> {
    let (program, interner) = parse(input);
    let free = collect_free_vars_in_program(&program);
    let mut names: Vec<String> = free
        .iter()
        .map(|s| interner.resolve(*s).to_string())
        .collect();
    names.sort();
    names
}

fn free_var_names_expr(input: &str) -> Vec<String> {
    let (program, interner) = parse(input);
    // Extract the expression from the first expression statement
    let expr = match &program.statements[0] {
        flux::syntax::statement::Statement::Expression { expression, .. } => expression,
        _ => panic!("Expected expression statement"),
    };
    let free = collect_free_vars(expr);
    let mut names: Vec<String> = free
        .iter()
        .map(|s| interner.resolve(*s).to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn simple_closure_captures_free_var() {
    // y is free in the lambda
    let names = free_var_names_expr(r#"\x -> y;"#);
    assert_eq!(names, vec!["y"]);
}

#[test]
fn bound_parameter_not_free() {
    // x is bound by the lambda parameter
    let names = free_var_names_expr(r#"\x -> x;"#);
    assert!(names.is_empty(), "Expected no free vars, got: {:?}", names);
}

#[test]
fn nested_closure_captures_outer_free() {
    // z is free; x is bound by outer lambda, y by inner
    let names = free_var_names_expr(r#"\x -> \y -> x + z;"#);
    assert_eq!(names, vec!["z"]);
}

#[test]
fn let_binding_removes_from_free() {
    // x is bound by let, not free in the lambda
    let names = free_var_names("let x = 1; let f = \\y -> x + y;");
    assert!(names.is_empty(), "Expected no free vars, got: {:?}", names);
}

#[test]
fn function_params_not_free() {
    let names = free_var_names("fn f(x) { x; }");
    assert!(names.is_empty(), "Expected no free vars, got: {:?}", names);
}

#[test]
fn function_body_references_outer() {
    // y is free inside g since it's not defined anywhere
    let names = free_var_names("fn g(x) { x + y; }");
    assert_eq!(names, vec!["y"]);
}

#[test]
fn match_pattern_bindings_not_free() {
    // x is bound by the pattern in the match arm
    let names = free_var_names_expr(r#"match v { Some(x) -> x, _ -> 0 };"#);
    // v is free (not defined), x is bound by pattern
    assert_eq!(names, vec!["v"]);
}

#[test]
fn multiple_free_vars() {
    let names = free_var_names_expr(r#"\x -> a + b + x;"#);
    assert_eq!(names, vec!["a", "b"]);
}

#[test]
fn recursive_function_name_not_free() {
    // f is defined as a function, so calling f inside its body is not free
    let names = free_var_names("fn f(n) { if n == 0 { 0; } else { f(n - 1); }; }");
    assert!(names.is_empty(), "Expected no free vars, got: {:?}", names);
}

#[test]
fn shadowing_hides_outer() {
    // Inner x shadows the let binding — x should not be free
    let names = free_var_names("let x = 1; let f = \\x -> x + 1;");
    assert!(names.is_empty(), "Expected no free vars, got: {:?}", names);
}

#[test]
fn let_value_cannot_reference_itself() {
    // In `let x = x + 1;`, the rhs x is free because it's evaluated before binding
    let names = free_var_names("let x = x + 1;");
    assert_eq!(names, vec!["x"]);
}
