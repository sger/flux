pub mod aether;
pub mod ast;
pub mod bytecode;
pub mod cfg;
pub mod cli;
pub mod compiler;
pub mod core;
// Diagnostics are their own crate: they depend only on `flux-source`, and
// keeping them out of the compiler crate is what will let the type layer
// report without depending on the compiler. Re-exported so
// `crate::diagnostics::…` resolves unchanged.
pub use flux_diagnostics as diagnostics;
pub mod driver;
pub mod generics_frontend;
pub mod lir;
#[cfg(feature = "llvm")]
pub mod llvm;
pub mod lsp_support;
pub mod parity;
#[cfg(feature = "repl")]
pub mod repl;
pub mod runtime;
pub mod shared;
pub mod shared_ir;
pub mod syntax;
pub mod types;
pub mod vm;
