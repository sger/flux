//! Aether — Flux's compile-time reference counting optimization.
//!
//! Implements Perceus-style dup/drop insertion for Core IR, enabling:
//! - Phase 5: explicit `Dup` (Rc::clone) and `Drop` (early release) in Core IR
//! - Phase 6: borrowing analysis to elide dup/drop for read-only params
//! - Phase 7: reuse tokens for zero-allocation functional updates
//!
//! Aether contract:
//! - `Dup` / `Drop` operate only on resolved Core binders.
//! - `Reuse` may target only heap-allocating constructor tags and the reuse
//!   token must not appear free in any field expression.
//! - `DropSpecialized` must use a resolved scrutinee and both branches must
//!   remain valid Aether expressions that do not use the scrutinee value after
//!   specialization.
//!
//! The pass runs as the final Core IR transformation (after ANF normalization).
//! Existing passes (1-7) never see Dup/Drop nodes.

pub mod analysis;
pub mod borrow_infer;
pub mod check_fbip;
pub mod drop_spec;
pub mod fusion;
pub mod insert;
pub mod reuse;
pub mod verify;

use crate::core::{CoreExpr, CoreTag, CoreVarRef};
use crate::diagnostics::position::Span;

/// Statistics collected from an Aether-transformed Core IR expression.
#[derive(Debug, Clone, Default)]
pub struct AetherStats {
    pub dups: usize,
    pub drops: usize,
    pub reuses: usize,
    pub drop_specs: usize,
    /// Number of heap constructor allocations (Con nodes with heap tags).
    pub allocs: usize,
}

/// FBIP status auto-detected from Aether stats (Perceus Section 2.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FbipStatus {
    /// Zero unreused allocations — functional but fully in-place on the unique path.
    Fip,
    /// N unreused allocations — functional but partially in-place.
    Fbip(usize),
    /// No constructors in the function — FBIP classification not applicable.
    NotApplicable,
}

impl AetherStats {
    /// Total constructor sites (both fresh allocations and reused ones).
    pub fn total_constructors(&self) -> usize {
        self.allocs + self.reuses
    }

    /// Auto-detect FBIP status from allocation and reuse counts.
    /// - `fip`: all constructor sites are reused (zero fresh allocations)
    /// - `fbip(N)`: N fresh allocations (not reused)
    /// - `NotApplicable`: no constructor sites at all
    pub fn fbip_status(&self) -> FbipStatus {
        if self.total_constructors() == 0 {
            FbipStatus::NotApplicable
        } else if self.allocs == 0 {
            FbipStatus::Fip
        } else {
            FbipStatus::Fbip(self.allocs)
        }
    }
}

impl std::fmt::Display for AetherStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Dups: {}  Drops: {}  Reuses: {}  DropSpecs: {}",
            self.dups, self.drops, self.reuses, self.drop_specs
        )?;
        match self.fbip_status() {
            FbipStatus::Fip => write!(f, "  FBIP: fip"),
            FbipStatus::Fbip(n) => write!(f, "  FBIP: fbip({})", n),
            FbipStatus::NotApplicable => Ok(()),
        }
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
        CoreExpr::Con { tag, fields, .. } => {
            // Count heap-allocating constructors (not Nil/None which are value types)
            if is_heap_tag(tag) {
                stats.allocs += 1;
            }
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

/// Returns true for constructor tags that allocate on the heap.
/// Nil and None are value types (no heap allocation).
pub(crate) fn is_heap_tag(tag: &CoreTag) -> bool {
    match tag {
        CoreTag::Cons | CoreTag::Some | CoreTag::Left | CoreTag::Right | CoreTag::Named(_) => true,
        CoreTag::Nil | CoreTag::None => false,
    }
}

/// View an expression as a constructor allocation shape for Aether passes.
///
/// Named ADT constructors can still appear as external constructor applications
/// in Core; those require an expected tag from the enclosing pattern to
/// disambiguate them from ordinary external calls.
pub fn constructor_shape_for_tag<'a>(
    expr: &'a CoreExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [CoreExpr], Span)> {
    match expr {
        CoreExpr::Con { tag, fields, span } => Some((tag.clone(), fields.as_slice(), *span)),
        CoreExpr::App { func, args, span } => {
            constructor_app_shape_for_tag(func.as_ref(), args, *span, expected_tag)
        }
        _ => None,
    }
}

/// Consume an expression if it is constructor-shaped.
pub fn into_constructor_shape_for_tag(
    expr: CoreExpr,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<CoreExpr>, Span)> {
    match expr {
        CoreExpr::Con { tag, fields, span } => Some((tag, fields, span)),
        CoreExpr::App { func, args, span } => {
            into_constructor_app_shape_for_tag(*func, args, span, expected_tag)
        }
        _ => None,
    }
}

fn constructor_app_shape_for_tag<'a>(
    func: &'a CoreExpr,
    args: &'a [CoreExpr],
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, &'a [CoreExpr], Span)> {
    let CoreExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(var, expected_tag)?;
    Some((tag, args, span))
}

fn into_constructor_app_shape_for_tag(
    func: CoreExpr,
    args: Vec<CoreExpr>,
    span: Span,
    expected_tag: Option<&CoreTag>,
) -> Option<(CoreTag, Vec<CoreExpr>, Span)> {
    let CoreExpr::Var { var, .. } = func else {
        return None;
    };
    let tag = core_tag_from_constructor_var(&var, expected_tag)?;
    Some((tag, args, span))
}

fn core_tag_from_constructor_var(
    var: &CoreVarRef,
    expected_tag: Option<&CoreTag>,
) -> Option<CoreTag> {
    if var.binder.is_some() {
        return None;
    }
    match expected_tag {
        Some(CoreTag::Named(name)) if var.name == *name => Some(CoreTag::Named(*name)),
        _ => None,
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

/// Run the Aether pipeline with a borrow registry for cross-function optimization.
/// Arguments to borrowed parameters will skip Rc::clone.
pub fn run_aether_pass_with_registry(
    expr: CoreExpr,
    registry: &borrow_infer::BorrowRegistry,
) -> CoreExpr {
    let expr = insert::insert_dup_drop_with_registry(expr, registry);
    let expr = drop_spec::specialize_drops(expr);
    let expr = fusion::fuse_dup_drop(expr);
    reuse::insert_reuse(expr)
}
