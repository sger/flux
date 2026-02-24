//! Internal type representations and substitution utilities used by the
//! Hindley-Milner-style inference engine.
//!
//! This module is intentionally separate from:
//! - surface syntax types (`TypeExpr`)
//! - runtime contract/checking types (`RuntimeType`)
pub mod infer_type;
pub mod type_constructor;
pub mod type_subst;

/// A fresh identifier for unification variables during inference.
pub type TypeVarId = u32;
