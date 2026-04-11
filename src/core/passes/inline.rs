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

use super::helpers::{is_trivially_pure, map_children, subst};

pub fn inline_trivial_lets(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let { var, rhs, body, .. } => {
            let rhs = inline_trivial_lets(*rhs);
            let body = inline_trivial_lets(*body);
            let preserves_call_boundary =
                matches!(rhs, CoreExpr::Var { .. } | CoreExpr::Lam { .. })
                    && occurs_as_callee(var.id, &body);
            if is_trivially_pure(&rhs) && !preserves_call_boundary {
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
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => map_children(expr, inline_trivial_lets),
        other => other,
    }
}

fn occurs_as_callee(var: crate::core::CoreBinderId, expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) {
                false
            } else {
                occurs_as_callee(var, body)
            }
        }
        CoreExpr::App { func, args, .. } => {
            matches!(func.as_ref(), CoreExpr::Var { var: ref_var, .. } if ref_var.binder == Some(var))
                || occurs_as_callee(var, func)
                || args.iter().any(|arg| occurs_as_callee(var, arg))
        }
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            ..
        } => {
            occurs_as_callee(var, rhs)
                || if binding.id == var {
                    false
                } else {
                    occurs_as_callee(var, body)
                }
        }
        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            ..
        } => {
            if binding.id == var {
                false
            } else {
                occurs_as_callee(var, rhs) || occurs_as_callee(var, body)
            }
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            if bindings.iter().any(|(binder, _)| binder.id == var) {
                false
            } else {
                bindings.iter().any(|(_, rhs)| occurs_as_callee(var, rhs))
                    || occurs_as_callee(var, body)
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            occurs_as_callee(var, scrutinee)
                || alts.iter().any(|alt| {
                    let shadowed = pattern_binds(var, &alt.pat);
                    if shadowed {
                        return false;
                    }
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| occurs_as_callee(var, guard))
                        || occurs_as_callee(var, &alt.rhs)
                })
        }
        CoreExpr::Con { fields, .. } | CoreExpr::PrimOp { args: fields, .. } => {
            fields.iter().any(|field| occurs_as_callee(var, field))
        }
        CoreExpr::Return { value, .. } => occurs_as_callee(var, value),
        CoreExpr::Perform { args, .. } => args.iter().any(|arg| occurs_as_callee(var, arg)),
        CoreExpr::Handle { body, handlers, .. } => {
            occurs_as_callee(var, body)
                || handlers.iter().any(|handler| {
                    if handler.params.iter().any(|p| p.id == var) || handler.resume.id == var {
                        false
                    } else {
                        occurs_as_callee(var, &handler.body)
                    }
                })
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            occurs_as_callee(var, object)
        }
    }
}

fn pattern_binds(var: crate::core::CoreBinderId, pat: &crate::core::CorePat) -> bool {
    match pat {
        crate::core::CorePat::Var(binder) => binder.id == var,
        crate::core::CorePat::Con { fields, .. } | crate::core::CorePat::Tuple(fields) => {
            fields.iter().any(|field| pattern_binds(var, field))
        }
        _ => false,
    }
}
