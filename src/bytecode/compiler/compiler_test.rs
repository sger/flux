use crate::{
    bytecode::compiler::Compiler,
    frontend::{lexer::Lexer, parser::Parser},
    runtime::object::Object,
};

fn parse_program(source: &str) -> crate::frontend::program::Program {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    program
}

#[test]
fn compile_integer_literals_emits_constants() {
    let program = parse_program("1; 2;");
    let mut compiler = Compiler::new();
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(bytecode.constants.contains(&Object::Integer(1)));
    assert!(bytecode.constants.contains(&Object::Integer(2)));
}

#[test]
fn compile_string_literal_emits_constant() {
    let program = parse_program("\"hello\";");
    let mut compiler = Compiler::new();
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(bytecode
        .constants
        .contains(&Object::String("hello".to_string())));
}

#[test]
fn compile_function_decl_emits_function_constant() {
    let program = parse_program("fun add(x, y) { return x + y; }");
    let mut compiler = Compiler::new();
    compiler.compile(&program).unwrap();

    let bytecode = compiler.bytecode();

    assert!(bytecode
        .constants
        .iter()
        .any(|obj| matches!(obj, Object::Function(_))));
}
