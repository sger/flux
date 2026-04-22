/// Dead let elimination pass.
///
/// Removes `Let { var, rhs, body }` where `var` does not appear free in `body`
/// and `rhs` has no observable side effect. Proposal 0161 Phase 3 widens this
/// from strict `is_pure` to `can_discard`: a `CanFail` rhs (e.g. `10 / n`,
/// `arr[i]`) is safe to drop because its failure was never observed — the
/// binding is unused, so the whole expression is never evaluated.
use crate::core::CoreExpr;

use super::helpers::{appears_free, can_discard, map_children};

pub fn elim_dead_let(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = elim_dead_let(*rhs);
            let body = elim_dead_let(*body);
            if can_discard(&rhs) && !appears_free(var.id, &body) {
                body
            } else {
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            }
        }
        CoreExpr::Lam { .. }
        | CoreExpr::App { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. } => map_children(expr, elim_dead_let),
        other => other,
    }
}
