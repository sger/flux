//! The tests in `tests/integration/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod cache_invalidation_tests;
mod compiler_optimization_tests;
mod compiler_rules_tests;
mod cross_module_function_tests;
mod cross_module_named_fields_tests;
mod flow_async_surface_tests;
mod flow_effects_spec_drift;
mod flow_prelude_module_tests;
mod flow_task_tests;
mod layout_guard;
mod module_graph_tests;
mod module_linking_integration_tests;
mod module_local_shadowing_tests;
mod module_scoped_classes_tests;
mod mutual_recursion;
mod named_field_class_dispatch_tests;
mod operators;
mod qualified_class_method_dispatch_tests;
mod regression_snapshots;
mod stdlib_discovery_tests;
mod symbol_table_tests;
mod test_runner_cli;
mod tuple_projection_predicate_tests;
mod type_alias_effect_row_tests;
