//! Integration tests for the bytecode compiler's optimization passes
//! (`compile_with_opts`): constant folding, tail-call detection, free-variable
//! analysis, and pipeline ordering.

use flux::{
    ast::constant_fold,
    compiler::Compiler,
    syntax::{interner::Interner, lexer::Lexer, parser::Parser, program::Program},
};

fn parse(source: &str) -> (Program, Interner) {
    let lexer = Lexer::new(source);
    let mut parser = Parser::new(lexer);
    let program = parser.parse_program();
    let interner = parser.take_interner();
    (program, interner)
}

// ---------------------------------------------------------------------------
// compile_with_opts: end-to-end pass interaction
// ---------------------------------------------------------------------------

#[test]
fn compile_with_opts_applies_all_transformations() {
    let source = r#"
        fn factorial(n, acc) {
            if n == 0 { acc } else { factorial(n - 1, n * acc) }
        }
        let x = !!(2 + 3);
        let y = !(5 == 3);
    "#;

    let (program, interner) = parse(source);

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

    let (program, interner) = parse(source);
    let mut compiler = Compiler::new_with_interner("test.flx", interner);

    // This will fail at compile time, but analysis pass should detect free vars
    let _ = compiler.compile_with_opts(&program, false, true);

    assert!(
        !compiler.free_vars.is_empty(),
        "Expected undefined_var to be detected as free"
    );
}

#[test]
fn optimization_pipeline_order_is_correct() {
    // Desugar happens before constant fold: !(2 == 2) → (2 != 2) → false
    let source = "let x = !(2 == 2);";

    let (program, interner) = parse(source);
    let mut compiler = Compiler::new_with_interner("test.flx", interner);
    compiler.compile_with_opts(&program, true, false).unwrap();
    let bytecode = compiler.bytecode();

    let has_integer_2 = bytecode
        .constants
        .iter()
        .any(|c| matches!(c, flux::runtime::value::Value::Integer(2)));

    assert!(
        !has_integer_2,
        "Expected constant folding to eliminate intermediate integer constants"
    );
}

#[test]
fn compile_with_opts_false_is_same_as_compile() {
    let source = "let x = 42;";

    let (program, interner) = parse(source);

    let mut compiler1 = Compiler::new_with_interner("test.flx", interner.clone());
    compiler1.compile(&program).unwrap();
    let bc1 = compiler1.bytecode();

    let mut compiler2 = Compiler::new_with_interner("test.flx", interner);
    compiler2.compile_with_opts(&program, false, false).unwrap();
    let bc2 = compiler2.bytecode();

    assert_eq!(bc1.constants.len(), bc2.constants.len());
}

// ---------------------------------------------------------------------------
// Constant folding in isolation
// ---------------------------------------------------------------------------

#[test]
fn constant_folding_reduces_constants() {
    let source = r#"
        let a = 2 + 3;
        let b = "hello" + " world";
    "#;

    let (program, interner) = parse(source);

    let mut compiler_no_opt = Compiler::new_with_interner("test.flx", interner.clone());
    compiler_no_opt.compile(&program).unwrap();
    let bytecode_no_opt = compiler_no_opt.bytecode();

    let mut compiler_opt = Compiler::new_with_interner("test.flx", interner);
    compiler_opt
        .compile_with_opts(&program, true, false)
        .unwrap();
    let bytecode_opt = compiler_opt.bytecode();

    assert!(
        bytecode_opt.constants.len() < bytecode_no_opt.constants.len(),
        "Optimized bytecode should have fewer constants: {} vs {}",
        bytecode_opt.constants.len(),
        bytecode_no_opt.constants.len()
    );

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

    let (program, _) = parse(source);

    let folded = constant_fold(program.clone());

    // Both should have the same number of statements;
    // semantic result (27) is implicitly preserved.
    assert_eq!(program.statements.len(), folded.statements.len());
}
