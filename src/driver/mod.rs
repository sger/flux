//! Driver entrypoint and internal orchestration modules.
//!
//! The driver layer owns CLI-selected execution policy, pipeline orchestration, backend dispatch,
//! command helpers, and user-facing reporting. It should not introduce new compiler IR stages or
//! backend-agnostic semantic behavior; that work belongs in the compiler pipeline itself.

pub mod backend;
pub mod backend_policy;
pub mod command;
pub mod flags;
pub(crate) mod frontend;
pub mod mode;
pub(crate) mod module_compile;
pub mod pipeline;
pub(crate) mod reporting;
pub(crate) mod run_program;
pub(crate) mod run_tests;
pub mod session;
pub(crate) mod shared;
pub(crate) mod support;
#[cfg(test)]
pub(crate) mod test_support;

pub use mode::{AetherDumpMode, CoreDumpMode, DiagnosticOutputFormat, RunMode};
