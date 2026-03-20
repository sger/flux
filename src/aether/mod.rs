//! Aether — Flux's compile-time reference counting optimization.
//!
//! Implements Perceus-style dup/drop insertion for Core IR, enabling:
//! - Phase 5: explicit `Dup` (Rc::clone) and `Drop` (early release) in Core IR
//! - Phase 6: borrowing analysis to elide dup/drop for read-only params
//! - Phase 7: reuse tokens for zero-allocation functional updates
//!
//! The pass runs as the final Core IR transformation (after ANF normalization).
//! Existing passes (1-7) never see Dup/Drop nodes.

pub mod analysis;
pub mod drop_spec;
pub mod fusion;
pub mod insert;
pub mod reuse;
pub mod verify;

use crate::core::CoreExpr;

/// Statistics collected from an Aether-transformed Core IR expression.
#[derive(Debug, Clone, Default)]
pub struct AetherStats {
    pub dups: usize,
    pub drops: usize,
    pub reuses: usize,
    pub drop_specs: usize,
}

impl std::fmt::Display for AetherStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dups: {}  Drops: {}  Reuses: {}  DropSpecs: {}",
            self.dups, self.drops, self.reuses, self.drop_specs
        )
    }
}

/// Walk a Core IR expression and count Dup/Drop/Reuse nodes.
pub fn collect_stats(expr: &CoreExpr) -> AetherStats {
    let mut stats = AetherStats::default();
    count_nodes(expr, &mut stats);
    stats
}

fn count_nodes(expr: &CoreExpr, stats: &mut AetherStats) {
    match expr {
        CoreExpr::Dup { body, .. } => {
            stats.dups += 1;
            count_nodes(body, stats);
        }
        CoreExpr::Drop { body, .. } => {
            stats.drops += 1;
            count_nodes(body, stats);
        }
        CoreExpr::Reuse { fields, .. } => {
            stats.reuses += 1;
            for f in fields {
                count_nodes(f, stats);
            }
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { body, .. } => count_nodes(body, stats),
        CoreExpr::App { func, args, .. } => {
            count_nodes(func, stats);
            for a in args {
                count_nodes(a, stats);
            }
        }
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            count_nodes(rhs, stats);
            count_nodes(body, stats);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            count_nodes(scrutinee, stats);
            for alt in alts {
                count_nodes(&alt.rhs, stats);
                if let Some(g) = &alt.guard {
                    count_nodes(g, stats);
                }
            }
        }
        CoreExpr::Con { fields, .. } => {
            for f in fields {
                count_nodes(f, stats);
            }
        }
        CoreExpr::PrimOp { args, .. } => {
            for a in args {
                count_nodes(a, stats);
            }
        }
        CoreExpr::Return { value, .. } => count_nodes(value, stats),
        CoreExpr::Perform { args, .. } => {
            for a in args {
                count_nodes(a, stats);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            count_nodes(body, stats);
            for h in handlers {
                count_nodes(&h.body, stats);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            stats.drop_specs += 1;
            count_nodes(unique_body, stats);
            count_nodes(shared_body, stats);
        }
    }
}

/// Run the full Aether optimization pipeline on a Core IR expression.
///
/// Pipeline order:
/// 1. Dup/drop insertion (Phase 5) — insert explicit Rc operations
/// 2. Drop specialization (Phase 8) — split into unique/shared paths
/// 3. Dup/drop fusion (Phase 9) — cancel adjacent dup/drop pairs
/// 4. Reuse token insertion (Phase 7) — reuse allocations on the fast path
///
/// This is the public entry point called from `run_core_passes`.
pub fn run_aether_pass(expr: CoreExpr) -> CoreExpr {
    let expr = insert::insert_dup_drop(expr);
    let expr = drop_spec::specialize_drops(expr);
    let expr = fusion::fuse_dup_drop(expr);
    reuse::insert_reuse(expr)
}
