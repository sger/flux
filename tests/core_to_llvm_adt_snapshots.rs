#![cfg(feature = "native")]

use std::collections::HashMap;

use flux::{
    bytecode::compiler::Compiler,
    core::{lower_ast::lower_program_ast, passes::run_core_passes_with_interner},
    core_to_llvm::render_module,
    lir,
    syntax::{expression::ExprId, interner::Interner, lexer::Lexer, parser::Parser},
    types::infer_type::InferType,
};

fn parse_and_lower_to_llvm_module(src: &str) -> (flux::core_to_llvm::LlvmModule, Interner) {
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
    let lir_program = lir::lower::lower_program_with_interner(&core, Some(&interner), None);
    let module = lir::emit_llvm::emit_llvm_module(&lir_program);
    (module, interner)
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
    let (module, _interner) = parse_and_lower_to_llvm_module(src);

    insta::with_settings!({
        snapshot_path => "snapshots/core_to_llvm",
        prepend_module_to_snapshot => false,
    }, {
        insta::assert_snapshot!("adt_list_tuple_module", render_module(&module));
    });
}
