/// Trivial let inlining pass.
///
/// Inlines `let x = rhs; body` when `rhs` is a literal or variable.
///
/// This is constant propagation + copy propagation at the Core IR level.
/// It complements `elim_dead_let`: that pass removes unused pure bindings;
/// this pass substitutes trivial values so downstream passes (COKC, dead-let)
/// can see through them.
///
/// Examples:
/// ```text
/// let x = 5; x + x          →  5 + 5
/// let x = y; some_fn(x)     →  some_fn(y)
/// ```
use crate::core::CoreExpr;

use super::helpers::{is_pure, map_children, subst};

pub fn inline_trivial_lets(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let { var, rhs, body, .. } => {
            let rhs = inline_trivial_lets(*rhs);
            let body = inline_trivial_lets(*body);
            if is_pure(&rhs) {
                // Substitute and continue — may unlock further inlining.
                inline_trivial_lets(subst(body, var.id, &rhs))
            } else {
                // Keep the binding; rhs has side-effects or is non-trivial.
                let span = rhs.span();
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
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, inline_trivial_lets),
        other => other,
    }
}
