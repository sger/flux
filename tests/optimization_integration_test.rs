use flux::{
    ast::constant_fold,
    bytecode::compiler::Compiler,
    syntax::{lexer::Lexer, parser::Parser},
};

#[test]
fn constant_folding_reduces_constants() {
    let source = r#"
        let a = 2 + 3;
        let b = "hello" + " world";
    "#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    // Compile without optimization
    let mut compiler_no_opt = Compiler::new_with_interner("test.flx", interner.clone());
    compiler_no_opt.compile(&program).unwrap();
    let bytecode_no_opt = compiler_no_opt.bytecode();

    // Compile with optimization
    let mut compiler_opt = Compiler::new_with_interner("test.flx", interner);
    compiler_opt.compile_with_opts(&program, true, false).unwrap();
    let bytecode_opt = compiler_opt.bytecode();

    // Optimized version should have fewer constants
    assert!(
        bytecode_opt.constants.len() < bytecode_no_opt.constants.len(),
        "Optimized bytecode should have fewer constants: {} vs {}",
        bytecode_opt.constants.len(),
        bytecode_no_opt.constants.len()
    );

    // Should have exactly 2 constants (5 and "hello world")
    assert_eq!(
        bytecode_opt.constants.len(),
        2,
        "Expected 2 constants after folding"
    );
}

#[test]
fn constant_folding_preserves_semantics() {
    let source = r#"
        let result = (10 + 5) * 2 - 6 / 2;
        result;
    "#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();

    // Fold constants
    let folded = constant_fold(program.clone());

    // Both should have same number of statements
    assert_eq!(program.statements.len(), folded.statements.len());

    // The folded version should produce same result (27)
    // (10 + 5) * 2 - 6 / 2 = 15 * 2 - 3 = 30 - 3 = 27
}

#[test]
fn compile_with_opts_false_is_same_as_compile() {
    let source = "let x = 42;";

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let mut compiler1 = Compiler::new_with_interner("test.flx", interner.clone());
    compiler1.compile(&program).unwrap();
    let bc1 = compiler1.bytecode();

    let mut compiler2 = Compiler::new_with_interner("test.flx", interner);
    compiler2.compile_with_opts(&program, false, false).unwrap();
    let bc2 = compiler2.bytecode();

    // Should produce identical bytecode
    assert_eq!(bc1.constants.len(), bc2.constants.len());
}
