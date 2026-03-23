#![cfg(feature = "core_to_llvm")]

use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    core_to_llvm::{CoreToLlvmError, compile_program_with_interner, render_module},
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

#[test]
fn lowers_single_function_with_entry_allocas() {
    let src = r#"
fn main(x) {
    let y = x
    y
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @main(i64 %arg0)"));
    assert!(rendered.contains("%slot.0 = alloca i64, align 8"));
    assert!(rendered.contains("store i64 %arg0, ptr %slot.0, align 8"));
    assert!(rendered.contains("load i64, ptr %slot.0, align 8"));
}

#[test]
fn lowers_factorial_with_control_flow_and_recursion() {
    let src = r#"
fn factorial(n, acc) {
    if n == 0 { acc } else { factorial(n - 1, n * acc) }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @factorial(i64 %arg0, i64 %arg1)"));
    assert!(rendered.contains("call fastcc i64 @flux_isub"));
    assert!(rendered.contains("call fastcc i64 @flux_imul"));
    assert!(rendered.contains("call fastcc i64 @factorial("));
    assert!(rendered.contains("br i1 %case.lit."));
    assert!(rendered.contains("phi i64"));
}

#[test]
fn lowers_fibonacci_with_recursive_calls() {
    let src = r#"
fn fibonacci(n) {
    if n < 2 { n } else { fibonacci(n - 1) + fibonacci(n - 2) }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);

    assert!(rendered.contains("define internal fastcc i64 @fibonacci(i64 %arg0)"));
    assert!(rendered.matches("call fastcc i64 @fibonacci").count() >= 2);
    assert!(rendered.contains("call fastcc i64 @flux_iadd"));
    assert!(rendered.contains("call fastcc i64 @flux_isub"));
}

#[test]
fn lowers_lambda_in_expression_via_closure_dispatch() {
    let src = r#"
fn main(x) {
    let f = fn (y) { y }
    f(x)
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let rendered = render_module(&module);
    assert!(rendered.contains("call fastcc i64 @flux_make_closure("));
    assert!(rendered.contains("call fastcc i64 @flux_call_closure("));
    assert!(rendered.contains(".lambda."));
}

#[test]
fn rejects_effects_and_adts() {
    let src = r#"
effect Console {
    print: String -> Unit
}

fn main() {
    perform Console.print("hi")
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let err =
        compile_program_with_interner(&core, Some(&interner)).expect_err("should reject effects");
    assert!(matches!(
        err,
        CoreToLlvmError::Unsupported {
            feature: "effects",
            ..
        }
    ));
}

#[test]
fn emitted_factorial_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let src = r#"
fn factorial(n, acc) {
    if n == 0 { acc } else { factorial(n - 1, n * acc) }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");
    let ll = render_module(&module);
    let path = std::env::temp_dir().join(format!(
        "core_to_llvm_phase3_{}.ll",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after unix epoch")
            .as_nanos()
    ));
    fs::write(&path, ll).expect("write ll");
    let output = Command::new("opt")
        .arg("--disable-output")
        .arg("--verify")
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
