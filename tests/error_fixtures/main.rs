//! The tests in `tests/error_fixtures/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod compiler_error_fixtures_snapshots;
mod parser_error_fixtures_snapshots;
mod runtime_error_fixtures_snapshots;
