use std::collections::HashMap;

use flux::compiler::Compiler;
use flux::cfg::validate_ir;
use flux::core::to_ir::lower_core_to_ir;
use flux::core::{display::CoreDisplayMode, lower_ast::lower_program_ast};
use flux::diagnostics::render_diagnostics;
use flux::syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser};
use flux::types::infer_type::InferType;

fn parse(input: &str) -> (flux::syntax::program::Program, Interner) {
    let mut parser = Parser::new(Lexer::new(input));
    let program = parser.parse_program();
    assert!(
        parser.errors.is_empty(),
        "parser errors: {:?}",
        parser.errors
    );
    let interner = parser.take_interner();
    (program, interner)
}

fn compile_err_code(input: &str) -> String {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    let err = compiler
        .compile(&program)
        .expect_err("expected compile error");
    err.first()
        .map(|d| d.code().unwrap_or("").to_string())
        .unwrap_or_default()
}

fn compile_ok_static_typing(input: &str) {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
}

fn dump_core_debug(input: &str) -> String {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner);
    compiler
        .compile(&program)
        .unwrap_or_else(|diags| panic!("{}", render_diagnostics(&diags, Some(input), None)));
    compiler
        .dump_core_with_opts(&program, false, CoreDisplayMode::Debug)
        .expect("dump_core_with_opts should succeed")
}

fn parse_and_infer(
    input: &str,
) -> (
    flux::syntax::program::Program,
    HashMap<ExprId, InferType>,
    Interner,
) {
    let (program, interner) = parse(input);
    let mut compiler = Compiler::new_with_interner("<unknown>", interner.clone());
    let hm_expr_types = compiler.infer_expr_types_for_program(&program);
    (program, hm_expr_types, interner)
}

#[test]
fn dynamic_top_type_escape_hatch_is_rejected_in_source_annotations() {
    let dynamic_top = ["A", "n", "y"].concat();
    let src = format!("fn id(x: {dynamic_top}) -> Int {{ 0 }}");
    let code = compile_err_code(&src);
    assert_eq!(code, "E423");
}

#[test]
fn static_typing_accepts_polymorphic_identity_without_dynamic_fallback() {
    compile_ok_static_typing("fn id(x) { x }");
}

#[test]
fn debug_core_dump_shows_explicit_type_residue_without_dynamic() {
    let core = dump_core_debug(
        r#"
fn id<T>(x: T) -> T { x }

fn main() {
    let n = id(1)
    let s = id("hi")
    n
}
"#,
    );

    assert!(
        core.contains("letrec id : "),
        "expected debug Core dump to preserve explicit type-variable residue, got:\n{core}"
    );
    assert!(
        core.contains("letrec id : a ="),
        "expected canonical debug Core dump to render stable quantified names, got:\n{core}"
    );
    assert!(
        !core.contains("Dynamic"),
        "debug Core dump should not regress to semantic Dynamic placeholders, got:\n{core}"
    );
}

#[test]
fn core_to_cfg_lowering_preserves_static_rep_contract() {
    let (program, hm_expr_types, _interner) = parse_and_infer(
        r#"
fn choose(x: Int) -> Int {
    if x > 0 { 1 } else { 0 }
}

fn main() {
    choose(5)
}
"#,
    );
    let core = lower_program_ast(&program, &hm_expr_types);
    let ir = lower_core_to_ir(&core);
    validate_ir(&ir).expect("maintained Core->CFG path should preserve rep/semantic contract");
}
