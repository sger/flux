//! The tests in `tests/type_inference/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod boundary_contract_tests;
mod constrained_type_params_integration;
mod imported_constructor_types_tests;
mod imported_ctor_payload_forwarding_tests;
mod match_some_pattern_tests;
mod non_zero_type_tests;
mod pattern_validation;
mod perf_type_infer;
mod sendable_tests;
mod static_type_validation_tests;
mod static_typing_closure;
mod static_typing_contract_tests;
mod type_infer_guards;
mod type_inference_tests;
mod type_semantics_matrix_tests;
mod type_semantics_runtime_matrix_tests;
mod typeclass_baseline_tests;
