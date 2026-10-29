//! Helpers shared by the test binaries. Each binary declares this module once:
//!
//! ```rust
//! #[path = "../support/mod.rs"]
//! mod support;
//! use crate::support::flux_runner;
//! ```
// Each binary uses a different subset, so most helpers look unused from any
// one of them.
#![allow(dead_code, unused_imports, unused_macros)]

pub mod diagnostics_env;
pub mod examples_snapshot;
pub mod flux_runner;
pub mod primop_parity;
pub mod scratch;
pub mod semantic_core_dump;
pub mod semantic_infer;
pub mod semantic_runtime;
pub mod stdlib_fixture;

/// `insta::assert_snapshot!` named after the test function alone. insta's
/// default name starts with the module path, which includes the test binary,
/// so moving a test to another binary would rename its snapshot.
macro_rules! assert_snapshot {
    ($value:expr) => {
        insta::with_settings!({ prepend_module_to_snapshot => false }, {
            insta::assert_snapshot!($value)
        })
    };
}
pub(crate) use assert_snapshot;
