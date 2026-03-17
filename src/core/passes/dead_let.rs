/// Dead let elimination pass.
///
/// Removes `Let { var, rhs, body }` where `var` does not appear free in `body`
/// and `rhs` is pure (a literal or variable — no observable effects).
use crate::core::CoreExpr;

use super::helpers::{appears_free, is_pure, map_children};

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
            if is_pure(&rhs) && !appears_free(var.id, &body) {
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
        | CoreExpr::Case { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. } => map_children(expr, elim_dead_let),
        other => other,
    }
}
