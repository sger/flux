use flux::bytecode::compiler::Compiler;
use flux::frontend::{lexer::Lexer, parser::Parser};

fn compile_ok(input: &str) {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let mut compiler = Compiler::new();
    compiler.compile(&program).expect("expected compile ok");
}

#[test]
fn match_some_literal_ok() {
    compile_ok("let x = Some(1); match x { Some(1) -> 1, None -> 0, _ -> 0 }");
}

#[test]
fn match_some_binding_ok() {
    compile_ok("let x = Some(1); match x { Some(v) -> v, None -> 0, _ -> 0 }");
}
