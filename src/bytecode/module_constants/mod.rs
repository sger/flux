//! Module constant evaluation for Flux.
//!
//! This module provides compile-time evaluation of module-level `let` bindings.

pub mod analysis;
pub mod compile;
pub mod dependency;
pub mod error;
pub mod eval;

pub use analysis::{ModuleConstantAnalysis, analyze_module_constants};
pub use compile::{ConstCompileError, compile_module_constants};
pub use dependency::{find_constant_refs, topological_sort_constants};
pub use error::ConstEvalError;
pub use eval::eval_const_expr;
