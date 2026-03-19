//! Aether — Flux's compile-time reference counting optimization.
//!
//! Implements Perceus-style dup/drop insertion for Core IR, enabling:
//! - Phase 5: explicit `Dup` (Rc::clone) and `Drop` (early release) in Core IR
//! - Phase 6 (future): borrowing analysis to elide dup/drop for read-only params
//! - Phase 7 (future): reuse tokens for zero-allocation functional updates
//!
//! The pass runs as the final Core IR transformation (after ANF normalization).
//! Existing passes (1-7) never see Dup/Drop nodes.

pub mod analysis;
pub mod insert;

use crate::core::CoreExpr;

/// Run the Aether dup/drop insertion pass on a Core IR expression.
///
/// This is the public entry point called from `run_core_passes` when the
/// `aether-rc` feature is enabled.
pub fn run_aether_pass(expr: CoreExpr) -> CoreExpr {
    insert::insert_dup_drop(expr)
}
