//! The tests in `tests/diagnostics/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod diagnostic_render_tests;
mod diagnostic_span_render_tests;
mod diagnostics_aggregator_tests;
mod diagnostics_rendering_extended_tests;
mod diagnostics_rustdoc_guard;
mod error_codes_registry_tests;
#[path = "../snapshots/diagnostics/mod.rs"]
mod snapshot_diagnostics;
mod unified_diagnostics_tests;
