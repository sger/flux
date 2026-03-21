use flux::bytecode::compiler::Compiler;
use flux::diagnostics::Diagnostic;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn compile_ok_with_warnings(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<aether-fbip>", interner);
    compiler.compile(&program).expect("expected compile ok");
    compiler.take_warnings()
}

fn compile_err_diagnostics(input: &str) -> Vec<Diagnostic> {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(parser.errors.is_empty(), "parser errors: {:?}", parser.errors);
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<aether-fbip>", interner);
    compiler.compile(&program).expect_err("expected compile error")
}

#[test]
fn fip_warning_reports_fresh_allocation_cause() {
    let src = std::fs::read_to_string("examples/aether/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist")
        .replace("@fbip fn bounded(f, x) {", "@fip fn bounded(f, x) {");
    let warnings = compile_ok_with_warnings(&src);
    assert!(warnings.iter().any(|d| d.message().is_some_and(|m| m.contains("indirect, unknown, or unannotated function"))));
}

#[test]
fn fbip_failure_is_hard_error() {
    let src = std::fs::read_to_string("examples/aether/fbip_fail_nonfip_call.flx")
        .expect("fixture should exist");
    let diagnostics = compile_err_diagnostics(&src);
    assert!(diagnostics.iter().any(|d| d.message().is_some_and(|m| m.contains("indirect, unknown, or unannotated function"))));
}

#[test]
fn vacuous_annotation_is_advisory_warning() {
    let src = std::fs::read_to_string("examples/aether/fbip_vacuous.flx")
        .expect("fixture should exist");
    let warnings = compile_ok_with_warnings(&src);
    assert!(warnings.iter().any(|d| {
        d.title() == "FBIP Annotation Has No Effect"
            && d.message().is_some_and(|m| m.contains("no heap constructor sites"))
    }));
}
