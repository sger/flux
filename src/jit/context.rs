//! Re-export from `crate::runtime::native_context`.
//!
//! The execution context and tagged-value types live in `src/runtime/` so they
//! can be shared between the Cranelift JIT and LLVM backends without feature
//! coupling. This module re-exports everything for backward compatibility.

pub use crate::runtime::native_context::*;
