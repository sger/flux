use flux::compiler::Compiler;
use flux::diagnostics::{Diagnostic, LabelStyle, render_diagnostics};
use flux::syntax::{expression::Expression, lexer::Lexer, parser::Parser, statement::Statement};

fn compile_ok_in(file_path: &str, input: &str) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    compiler.compile(&program).expect("expected compile ok");
}

fn compile_err(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().map(|d| d.code().unwrap_or("").to_string()).unwrap_or_default()
}

fn compile_err_in(file_path: &str, input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().map(|d| d.code().unwrap_or("").to_string()).unwrap_or_default()
}

fn compile_err_strict(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.set_strict_mode(true);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().map(|d| d.code().unwrap_or("").to_string()).unwrap_or_default()
}

fn compile_ok_strict(input: &str) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);

    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.set_strict_mode(true);
    compiler.compile(&program).expect("expected compile ok")
}

fn compile_err_static_typing(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().map(|d| d.code().unwrap_or("").to_string()).unwrap_or_default()
}

fn compile_ok_with_warnings_in(file_path: &str, input: &str, strict_mode: bool) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    compiler.set_strict_mode(strict_mode);
    compiler.compile(&program).expect("expected compile ok");
    compiler.take_warnings()
}

fn compile_err_strict_rendered(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.set_strict_mode(true);
    let err = compiler.compile(&program).expect_err("expected compile error");
    render_diagnostics(&err, None, None)
}

fn compile_err_title(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().map(|d| d.title().to_string()).unwrap_or_default()
}

fn compile_err_message(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    err.first().and_then(|d| d.message().map(ToOwned::to_owned)).unwrap_or_default()
}

fn compile_err_rendered(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    render_diagnostics(&err, None, None)
}

fn compile_err_rendered_in(file_path: &str, input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner(file_path, interner);
    let err = compiler.compile(&program).expect_err("expected compile error");
    render_diagnostics(&err, None, None)
}

fn compile_err_diagnostics(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler.compile(&program).expect_err("expected compile error")
}

fn compile_rendered_or_empty(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    match compiler.compile(&program) {
        Ok(()) => String::new(),
        Err(err) => render_diagnostics(&err, None, None),
    }
}

mod arity;
mod effects;
mod imports_and_privacy;
mod modules_and_matches;
mod recursion_and_handlers;
mod strict_hm;
