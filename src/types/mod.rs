//! Internal type representations and substitution utilities used by the
//! Hindley-Milner-style inference engine.
//!
//! This module is intentionally separate from:
//! - surface syntax types (`TypeExpr`)
//! - runtime contract/checking types (`RuntimeType`)
pub mod class_defaulting;
pub mod class_dispatch;
pub mod class_env;
pub mod class_id;
pub mod class_solver;
pub mod infer_effect_row;
pub mod infer_type;
pub mod kind;
pub mod module_interface;
pub mod scheme;
pub mod type_constructor;
pub mod type_env;
pub mod type_subst;
pub mod unify;
pub mod unify_error;

/// A fresh identifier for unification variables during inference.
pub type TypeVarId = u32;
