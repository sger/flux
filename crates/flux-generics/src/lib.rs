//! Generics: binding groups, quantification, evidence, and the dictionary
//! translation.
//!
//! This crate exists so the boundary around that subject is checked by the
//! compiler rather than by review. See
//! `docs/proposals/0186_generics_foundations.md`.
//!
//! It may not depend on the compiler crate. A backend receives a program whose
//! evidence is already explicit and has no say in it, so nothing here reaches
//! into lowering, bytecode or LLVM — and the crate graph is what enforces that,
//! after six hand-maintained resolution sites proved that review does not.

pub mod scc;

pub use scc::strongly_connected_components;
