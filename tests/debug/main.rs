//! The tests in `tests/debug/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod debug_info_tests;
mod native_codeview_tests;
mod native_debug_heap_tests;
mod native_debug_tests;
mod parity_debug_tests;
mod vm_debug_tests;
