use flux::bytecode::op_code::disassemble;
use flux::compiler::Compiler;
use flux::diagnostics::render_diagnostics;
use flux::syntax::lexer::Lexer;
use flux::syntax::parser::Parser;

fn compile_asm(input: &str) -> String {
    let lexer = Lexer::new(input);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    disassemble(&compiler.bytecode().instructions)
}

#[test]
fn if_le_condition_uses_fused_cmp_jump() {
    let asm = compile_asm("if 1 <= 2 { 10 } else { 20 };");
    assert!(
        asm.contains("OpCmpLeJumpNotTruthy"),
        "expected fused jump:\n{asm}"
    );
    assert!(
        !asm.contains("OpLessThanOrEqual"),
        "unexpected standalone compare:\n{asm}"
    );
}

#[test]
fn if_eq_condition_uses_fused_cmp_jump() {
    let asm = compile_asm("if 1 == 1 { 10 } else { 20 };");
    assert!(
        asm.contains("OpCmpEqJumpNotTruthy"),
        "expected fused jump:\n{asm}"
    );
    assert!(
        !asm.contains("OpEqual"),
        "unexpected standalone compare:\n{asm}"
    );
}

#[test]
fn match_literal_check_uses_fused_cmp_jump() {
    let asm = compile_asm("match 1 { 1 -> 10, _ -> 20 };");
    assert!(
        asm.contains("OpCmpEqJumpNotTruthy"),
        "expected fused jump:\n{asm}"
    );
}

#[test]
fn match_guard_uses_fused_cmp_jump() {
    let asm = compile_asm("match 2 { x if x >= 2 -> 10, _ -> 20 };");
    assert!(
        asm.contains("OpCmpGeJumpNotTruthy"),
        "expected fused jump:\n{asm}"
    );
    assert!(
        !asm.contains("OpGreaterThanOrEqual"),
        "unexpected standalone compare:\n{asm}"
    );
}

#[test]
fn plain_comparison_expression_stays_standalone() {
    let asm = compile_asm("1 <= 2;");
    assert!(
        asm.contains("OpLessThanOrEqual"),
        "expected standalone compare:\n{asm}"
    );
    assert!(
        !asm.contains("OpCmpLeJumpNotTruthy"),
        "unexpected fused jump:\n{asm}"
    );
}

#[test]
fn short_circuit_and_stays_unfused() {
    let asm = compile_asm("true && false;");
    assert!(
        asm.contains("OpJumpNotTruthy"),
        "expected short-circuit jump:\n{asm}"
    );
    assert!(
        !asm.contains("OpCmp"),
        "unexpected fused compare jump:\n{asm}"
    );
}
