//! The tests in `tests/parser/`, built as one binary: each file is a module.

#[path = "../support/mod.rs"]
mod support;

mod linter_tests;
mod list_literal_syntax_tests;
mod parser_recovery;
mod parser_tests;
#[path = "../snapshots/formatter/mod.rs"]
mod snapshot_formatter;
#[path = "../snapshots/parser/mod.rs"]
mod snapshot_parser;
mod span_tests;
