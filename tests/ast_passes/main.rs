//! The tests in `tests/ast_passes/`, built as one binary: each file is a module.

mod complexity_tests;
mod constant_fold_tests;
mod desugar_tests;
mod fold_smoke;
mod free_vars_tests;
mod rename_tests;
mod tail_call_tests;
mod tail_position_tests;
mod visit_smoke;
