#![cfg(feature = "llvm")]

use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    lir::{emit_llvm::emit_llvm_ir, lower::lower_program_with_interner},
    syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser},
    types::infer_type::InferType,
};

fn parse_and_lower_core(src: &str) -> (flux::core::CoreProgram, Interner) {
    let mut parser = Parser::new(Lexer::new(src));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    let mut compiler = Compiler::new_with_interner("<test>", interner.clone());
    let hm_expr_types: HashMap<ExprId, InferType> = compiler.infer_expr_types_for_program(&program);
    let mut core = lower_program_ast(&program, &hm_expr_types);
    run_core_passes_with_interner(&mut core, &interner, false).expect("core passes should succeed");
    (core, interner)
}

fn compile_to_llvm_ir(src: &str) -> String {
    let (core, interner) = parse_and_lower_core(src);
    let lir = lower_program_with_interner(&core, Some(&interner), None);
    emit_llvm_ir(&lir)
}

#[test]
fn lowers_single_function_with_entry() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main(x) {
    let y = x
    y
}
"#,
    );

    // `main` is renamed to `flux_main` with ccc calling convention.
    assert!(
        rendered.contains("@flux_main"),
        "expected flux_main in output"
    );
}

#[test]
fn lowers_factorial_with_control_flow_and_recursion() {
    let rendered = compile_to_llvm_ir(
        r#"
fn factorial(n, acc) {
    if n == 0 { acc } else { factorial(n - 1, n * acc) }
}
"#,
    );

    assert!(
        rendered.contains("@flux_factorial"),
        "expected flux_factorial in output"
    );
    assert!(
        rendered.contains("flux_isub") || rendered.contains("flux_sub"),
        "expected subtraction"
    );
    assert!(
        rendered.contains("flux_imul") || rendered.contains("flux_mul"),
        "expected multiplication"
    );
}

#[test]
fn lowers_fibonacci_with_recursive_calls() {
    let rendered = compile_to_llvm_ir(
        r#"
fn fibonacci(n) {
    if n < 2 { n } else { fibonacci(n - 1) + fibonacci(n - 2) }
}
"#,
    );

    assert!(
        rendered.contains("@flux_fibonacci"),
        "expected flux_fibonacci in output"
    );
    assert!(
        rendered.contains("flux_iadd") || rendered.contains("flux_add"),
        "expected addition"
    );
}

#[test]
fn lowers_lambda_in_expression_via_closure_dispatch() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main(x) {
    let f = fn (y) { y }
    f(x)
}
"#,
    );
    assert!(
        rendered.contains("flux_make_closure"),
        "expected closure creation"
    );
    assert!(
        rendered.contains("flux_call_closure"),
        "expected closure call"
    );
}

#[test]
fn lowers_float_math_primops_via_runtime_helpers() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main(x) {
    round(ceil(floor(log(exp(cos(sin(sqrt(x))))))))
}
"#,
    );

    assert!(rendered.contains("flux_sqrt"), "expected flux_sqrt helper");
    assert!(rendered.contains("flux_sin"), "expected flux_sin helper");
    assert!(rendered.contains("flux_cos"), "expected flux_cos helper");
    assert!(rendered.contains("flux_exp"), "expected flux_exp helper");
    assert!(rendered.contains("flux_log"), "expected flux_log helper");
    assert!(
        rendered.contains("flux_floor"),
        "expected flux_floor helper"
    );
    assert!(rendered.contains("flux_ceil"), "expected flux_ceil helper");
    assert!(
        rendered.contains("flux_round"),
        "expected flux_round helper"
    );
}

#[test]
fn lowers_bitwise_primops_to_integer_llvm_ops() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main(x, y) {
    bit_and(x, y) + bit_or(x, y) + bit_xor(x, y) + bit_shl(x, y) + bit_shr(x, y)
}
"#,
    );

    assert!(rendered.contains(" and i64 "), "expected integer and");
    assert!(rendered.contains(" or i64 "), "expected integer or");
    assert!(rendered.contains(" xor i64 "), "expected integer xor");
    assert!(rendered.contains(" shl i64 "), "expected integer shl");
    assert!(rendered.contains(" ashr i64 "), "expected integer ashr");
}

#[test]
fn emitted_factorial_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let ll = compile_to_llvm_ir(
        r#"
fn factorial(n, acc) {
    if n == 0 { acc } else { factorial(n - 1, n * acc) }
}
"#,
    );
    let path = std::env::temp_dir().join(format!(
        "llvm_codegen_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, &ll).expect("write ll");
    let output = Command::new("opt")
        .arg("--disable-output")
        .arg("-passes=verify")
        .arg(&path)
        .output()
        .expect("run opt");
    let _ = fs::remove_file(&path);
    assert!(
        output.status.success(),
        "opt verify failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
