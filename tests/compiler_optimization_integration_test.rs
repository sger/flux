use flux::{
    bytecode::compiler::Compiler,
    syntax::{lexer::Lexer, parser::Parser},
};

#[test]
fn compile_with_opts_applies_all_transformations() {
    let source = r#"
        fn factorial(n, acc) {
            if n == 0 { acc; } else { factorial(n - 1, n * acc); }
        }
        let x = !!(2 + 3);
        let y = !(5 == 3);
    "#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    // Compile WITHOUT optimization
    let mut compiler_no_opt = Compiler::new_with_interner("test.flx", interner.clone());
    compiler_no_opt
        .compile_with_opts(&program, false, false)
        .unwrap();
    let bytecode_no_opt = compiler_no_opt.bytecode();

    // Compile WITH optimization AND analysis
    let mut compiler_opt = Compiler::new_with_interner("test.flx", interner);
    compiler_opt
        .compile_with_opts(&program, true, true)
        .unwrap();
    let bytecode_opt = compiler_opt.bytecode();

    // 1. Constant folding should reduce constants
    assert!(
        bytecode_opt.constants.len() < bytecode_no_opt.constants.len(),
        "Expected fewer constants after optimization: {} vs {}",
        bytecode_opt.constants.len(),
        bytecode_no_opt.constants.len()
    );

    // 2. Tail calls should be detected
    assert!(
        !compiler_opt.tail_calls.is_empty(),
        "Expected tail calls to be detected"
    );
    assert_eq!(
        compiler_opt.tail_calls.len(),
        1,
        "Expected exactly 1 tail call (factorial)"
    );

    // 3. Free vars should be empty (all vars are defined)
    assert!(
        compiler_opt.free_vars.is_empty(),
        "Expected no free variables in this program"
    );

    // 4. Without optimization, analysis should be cleared
    assert!(
        compiler_no_opt.tail_calls.is_empty(),
        "Expected no tail call analysis without optimization"
    );
    assert!(
        compiler_no_opt.free_vars.is_empty(),
        "Expected no free var analysis without optimization"
    );
}

#[test]
fn compile_with_opts_detects_free_variables() {
    let source = r#"
        let x = undefined_var + 5;
    "#;

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let mut compiler = Compiler::new_with_interner("test.flx", interner);

    // This will fail at compile time, but analysis pass should detect free vars
    let _ = compiler.compile_with_opts(&program, false, true);

    // Free vars should be detected (even though compilation fails)
    assert!(
        !compiler.free_vars.is_empty(),
        "Expected undefined_var to be detected as free"
    );
}

#[test]
fn optimization_pipeline_order_is_correct() {
    // Test that desugar happens before constant fold
    // !(2 == 2) should desugar to (2 != 2) then fold to false
    let source = "let x = !(2 == 2);";

    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();

    let mut compiler = Compiler::new_with_interner("test.flx", interner);
    compiler.compile_with_opts(&program, true, false).unwrap();
    let bytecode = compiler.bytecode();

    // After desugaring to (2 != 2) and constant folding,
    // the result should be optimized (no constants for 2, 2, or comparison)
    // The bytecode should just be OpFalse

    // Check that we don't have the intermediate values (2, 2) as constants
    let has_integer_2 = bytecode
        .constants
        .iter()
        .any(|c| matches!(c, flux::runtime::value::Value::Integer(2)));

    assert!(
        !has_integer_2,
        "Expected constant folding to eliminate intermediate integer constants"
    );
}
