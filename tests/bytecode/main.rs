//! The tests in `tests/bytecode/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod cmp_jump_fusion_tests;
mod op_code_tests;
#[path = "../snapshots/bytecode/mod.rs"]
mod snapshot_bytecode;
mod superinstruction_tests;
