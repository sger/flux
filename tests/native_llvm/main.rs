//! The tests in `tests/native_llvm/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod bitwise_primop_native_tests;
mod effect_multi_shot_tests;
mod llvm_adt;
mod llvm_closures;
mod llvm_codegen;
mod llvm_snapshots;
mod llvm_type_class;
mod math_primop_native_tests;
mod native_async_indirect_tests;
mod native_async_sleep_tests;
#[cfg(feature = "llvm")]
mod native_constructor_tag_tests;
mod native_cross_module_effect_tests;
mod native_http_client_tests;
mod native_http_server_tests;
mod native_json_tests;
mod native_panic_trace_tests;
mod native_resolution_tests;
mod native_runtime_config_tests;
mod native_runtime_error_spans;
mod native_stream_tests;
mod native_typeclass_tests;
mod native_work_stealing_tests;
mod parity_corpus_tests;
mod safe_arithmetic_tests;
mod stackless_drop_native;
