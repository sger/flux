//! The tests in `tests/core_ir/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod backend_representation_runtime_tests;
mod core_type_contract_matrix_tests;
mod ir_pipeline_tests;
mod primop_compiler_lowering_tests;
mod primop_effect_summary_tests;
