//! The tests in `tests/lexer/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod interner_span_tests;
mod lexer_tests;
mod perf_parse_ops;
#[path = "../snapshots/lexer/mod.rs"]
mod snapshot_lexer;
mod token_tests;
mod token_type_tests;
