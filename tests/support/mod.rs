//! Helpers shared by the test binaries. Each binary declares this module once:
//!
//! ```rust
//! #[path = "../support/mod.rs"]
//! mod support;
//! use crate::support::flux_runner;
//! ```
// Each binary uses a different subset, so most helpers look unused from any
// one of them.
#![allow(dead_code, unused_imports)]

pub mod diagnostics_env;
pub mod examples_snapshot;
pub mod flux_runner;
pub mod primop_parity;
pub mod scratch;
pub mod semantic_core_dump;
pub mod semantic_infer;
pub mod semantic_runtime;
pub mod stdlib_fixture;
