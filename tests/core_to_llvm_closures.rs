#![cfg(feature = "native")]

use std::{
    collections::HashMap,
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use flux::{
    bytecode::compiler::Compiler,
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
fn lowers_lambda_expression_and_indirect_call() {
    let rendered = compile_to_llvm_ir(
        r#"
fn main() {
    let f = \x -> x + 1
    f(10)
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
fn lowers_top_level_function_value_via_wrapper_closure() {
    let rendered = compile_to_llvm_ir(
        r#"
fn add1(x) { x + 1 }
fn apply(f, x) { f(x) }
fn main() { apply(add1, 10) }
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
fn lowers_returned_closure_chain() {
    let rendered = compile_to_llvm_ir(
        r#"
fn make_adder(n) {
    \x -> x + n
}

fn main() {
    make_adder(5)(10)
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
fn lowers_partial_application_via_closure() {
    let rendered = compile_to_llvm_ir(
        r#"
fn add(a, b) { a + b }
fn main() {
    let add5 = add(5)
    add5(10)
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
fn lowers_higher_order_map_over_list() {
    let rendered = compile_to_llvm_ir(
        r#"
fn map(f, xs) {
    match xs {
        [h | t] -> [f(h) | map(f, t)],
        _ -> xs,
    }
}
fn main() {
    map(\x -> x + 1, [1, 2, 3])
}
"#,
    );

    assert!(
        rendered.contains("flux_call_closure"),
        "expected closure call"
    );
    assert!(
        rendered.contains("flux_make_cons"),
        "expected cons construction"
    );
}

#[test]
fn emitted_closure_module_verifies_with_opt_when_available() {
    if Command::new("opt").arg("--version").output().is_err() {
        return;
    }

    let ll = compile_to_llvm_ir(
        r#"
fn make_adder(n) {
    \x -> x + n
}

fn main() {
    make_adder(5)(10)
}
"#,
    );
    let path = std::env::temp_dir().join(format!(
        "core_to_llvm_closures_{}.ll",
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
