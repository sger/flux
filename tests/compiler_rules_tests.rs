use flux::bytecode::compiler::Compiler;
use flux::frontend::{lexer::Lexer, parser::Parser};

fn compile_ok_in(file_path: &str, input: &str) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let mut compiler = Compiler::new_with_file_path(file_path);
    compiler.compile(&program).expect("expected compile ok");
}

fn compile_err(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let mut compiler = Compiler::new();
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code.clone().unwrap_or_default())
        .unwrap_or_default()
}

fn compile_err_in(file_path: &str, input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let mut compiler = Compiler::new_with_file_path(file_path);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code.clone().unwrap_or_default())
        .unwrap_or_default()
}

#[test]
fn import_top_level_ok() {
    compile_ok_in(
        "examples/test.flx",
        "import Math module Main { fun main() { 1; } }",
    );
}

#[test]
fn import_in_function_error() {
    let code = compile_err("module Main { fun main() { import Math } }");
    assert_eq!(code, "E031");
}

#[test]
fn import_name_collision_error() {
    let code = compile_err_in("examples/test.flx", "let Math = 1; import Math");
    assert_eq!(code, "E030");
}

#[test]
fn private_member_access_error() {
    let code = compile_err(
        "module Math { fun _private() { 1; } } module Main { fun main() { Math._private(); } }",
    );
    assert_eq!(code, "E021");
}

#[test]
fn module_name_lowercase_error() {
    let code = compile_err("module math { fun main() { 1; } }");
    assert_eq!(code, "E016");
}

#[test]
fn module_name_clash_error() {
    let code = compile_err("module Math { fun Math() { 1; } }");
    assert_eq!(code, "E018");
}

#[test]
fn duplicate_params_error() {
    let code = compile_err("fun f(x, x) { x; }");
    assert_eq!(code, "E012");
}

#[test]
fn duplicate_params_literal_error() {
    let code = compile_err("let f = fun(x, x) { x; };");
    assert_eq!(code, "E012");
}

#[test]
fn immutable_reassign_error() {
    let code = compile_err("let x = 1; x = 2;");
    assert_eq!(code, "E003");
}

#[test]
fn outer_assignment_error() {
    let code = compile_err("fun outer() { let x = 1; let f = fun() { x = 2; }; }");
    assert_eq!(code, "E004");
}

#[test]
fn match_non_exhaustive_error() {
    let code = compile_err("let x = 2; match x { 1 -> 10; }");
    assert_eq!(code, "E033");
}

#[test]
fn match_identifier_non_last_error() {
    let code = compile_err("let x = 2; match x { y -> 1; _ -> 2; }");
    assert_eq!(code, "E034");
}

#[test]
fn match_wildcard_non_last_error() {
    let code = compile_err("let x = 2; match x { _ -> 1; 2 -> 2; }");
    assert_eq!(code, "E034");
}
