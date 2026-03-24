use flux::bytecode::vm::VM;
use flux::bytecode::{compiler::Compiler, op_code::disassemble};
use flux::diagnostics::render_diagnostics;
use flux::runtime::value::Value;
use flux::syntax::{lexer::Lexer, parser::Parser};

fn compile_disassembly(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let bytecode = compiler.bytecode();
    disassemble(&bytecode.instructions)
}

fn run_vm(input: &str) -> Value {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().unwrap();
    vm.last_popped_stack_elem().clone()
}

fn run_vm_err(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "{}",
        render_diagnostics(&parser.errors, Some(input), None)
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    let mut vm = VM::new(compiler.bytecode());
    vm.run().expect_err("expected runtime error")
}

#[test]
fn compiler_emits_op_call_base_for_allowlisted_base_function() {
    let asm = compile_disassembly("map(list(1, 2), fn(x) { x + 1 })");
    assert!(
        asm.contains("OpCallBase"),
        "expected OpCallBase in disassembly:\n{}",
        asm
    );
}

#[test]
fn compiler_does_not_emit_op_call_base_for_non_allowlisted_base_function() {
    let asm = compile_disassembly(r#"split("a,b", ",")"#);
    assert!(
        !asm.contains("OpCallBase"),
        "did not expect OpCallBase in disassembly:\n{}",
        asm
    );
}

#[test]
fn compiler_emits_op_call_base_for_promoted_allowlisted_base_function() {
    let asm = compile_disassembly("zip(list(1), list(2))");
    assert!(
        asm.contains("OpCallBase"),
        "expected OpCallBase in disassembly:\n{}",
        asm
    );
}

#[test]
fn compiler_does_not_emit_op_call_base_for_shadowed_name() {
    let asm = compile_disassembly(
        r#"
fn apply(map) { map(list(1, 2), fn(x) { x + 1 }) }
apply(fn(xs, f) { xs })
"#,
    );
    assert!(
        !asm.contains("OpCallBase"),
        "did not expect OpCallBase for shadowed name:\n{}",
        asm
    );
}

#[test]
fn vm_allowlisted_base_function_behavior_is_preserved() {
    let value = run_vm("to_array(map(list(1, 2, 3), fn(x) { x + 1 }))");
    assert_eq!(
        value,
        Value::Array(std::rc::Rc::new(vec![
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
        ]))
    );
}

#[test]
fn vm_allowlisted_base_function_wrong_arity_error_is_preserved() {
    let err = run_vm_err("map(list(1, 2, 3))");
    assert!(
        err.contains("wrong number of arguments"),
        "unexpected error: {}",
        err
    );
}

