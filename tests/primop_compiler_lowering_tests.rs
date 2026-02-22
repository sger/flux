use flux::bytecode::{compiler::Compiler, op_code::disassemble};
use flux::diagnostics::render_diagnostics;
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
    disassemble(&compiler.bytecode().instructions)
}

#[test]
fn compiler_emits_op_primop_for_builtin_len() {
    let asm = compile_disassembly(r#"len("abc")"#);
    assert!(
        asm.contains("OpPrimOp"),
        "expected OpPrimOp for len lowering:\n{}",
        asm
    );
}

#[test]
fn compiler_emits_op_primop_for_numeric_intrinsic() {
    let asm = compile_disassembly("iadd(1, 2)");
    assert!(
        asm.contains("OpPrimOp"),
        "expected OpPrimOp for iadd lowering:\n{}",
        asm
    );
}

#[test]
fn compiler_emits_op_primop_for_effectful_panic() {
    let asm = compile_disassembly(r#"panic("boom")"#);
    assert!(
        asm.contains("OpPrimOp"),
        "expected OpPrimOp for panic lowering:\n{}",
        asm
    );
}

#[test]
fn compiler_does_not_emit_op_primop_for_unmapped_builtin() {
    let asm = compile_disassembly(r#"trim("  hi  ")"#);
    assert!(
        !asm.contains("OpPrimOp"),
        "did not expect OpPrimOp for unmapped builtin trim:\n{}",
        asm
    );
}

#[test]
fn compiler_does_not_emit_op_primop_for_shadowed_name() {
    let asm = compile_disassembly(
        r#"
fn apply(len) { len("abc") }
apply(fn(x) { x })
"#,
    );
    assert!(
        !asm.contains("OpPrimOp"),
        "did not expect OpPrimOp for shadowed name:\n{}",
        asm
    );
}
