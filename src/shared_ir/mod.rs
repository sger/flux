//! Shared IR identity types.
//!
//! `shared_ir` is intentionally minimal now. The mixed program/container role
//! has been retired from production code; only the ID types remain shared
//! between backend and transitional layers.

pub mod ids;

pub use ids::{AdtId, BlockId, EffectId, FunctionId, GlobalId, IrVar, LiteralId};
