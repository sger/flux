#![cfg(feature = "native")]

use std::collections::HashMap;

use flux::{
    bytecode::compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    core_to_llvm::{compile_program_with_interner, render_module},
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
fn snapshot_for_adt_list_and_tuple_lowering() {
    let src = r#"
data Wrap {
    Wrap(Int),
    Empty,
}

fn fold(xs) {
    match xs {
        [] -> 0,
        [x | rest] -> x + fold(rest),
    }
}

fn main() {
    let value = (fold([1, 2]), Wrap(3))
    match value {
        (left, Wrap(right)) -> left + right,
        (_, Empty) -> 0,
    }
}
"#;
    let (core, interner) = parse_and_lower_core(src);
    let module = compile_program_with_interner(&core, Some(&interner)).expect("lower to llvm");

    insta::with_settings!({
        snapshot_path => "snapshots/core_to_llvm",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("adt_list_tuple_module", render_module(&module));
    });
}
