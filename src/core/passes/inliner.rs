/// Inlining pass with occurrence analysis.
///
/// Counts how many times each `Let`/`LetRec` binder is referenced, then
/// inlines bindings that are:
/// - Dead (0 references) — eliminated entirely
/// - Used once outside a lambda — always inlined (no duplication)
/// - Small (body ≤ threshold nodes) — inlined even if multiply referenced
///
/// Recursive (`LetRec`) bindings are never inlined (would create infinite expansion).
use crate::core::{CoreBinderId, CoreExpr, CorePat};

use super::helpers::{can_discard, expr_size, is_pure, map_children, subst};

/// Maximum node count of an RHS to inline when the binder is used more than once.
const INLINE_THRESHOLD: usize = 10;

/// Inline let-bindings guided by occurrence analysis.
///
/// This subsumes `inline_trivial_lets` — it inlines all trivial bindings
/// (literals, variables) and additionally inlines small or single-use bindings.
pub fn inline_lets(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = inline_lets(*rhs);
            let body = inline_lets(*body);

            let count = count_occurrences(var.id, &body);
            let preserves_call_boundary =
                matches!(rhs, CoreExpr::Var { .. } | CoreExpr::Lam { .. })
                    && occurs_as_callee(var.id, &body);

            if count == 0 && can_discard(&rhs) {
                // Dead binding — drop it. Proposal 0161 Phase 3: `CanFail`
                // primops (`10 / n`, `arr[i]`) are also droppable here because
                // the binding was unused, so the failure was never observed.
                body
            } else if count == 0 {
                // Dead but has observable effect — keep to preserve side effects.
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            } else if count == 1
                && !occurs_under_lambda(var.id, &body)
                && is_pure(&rhs)
                && !preserves_call_boundary
            {
                // Used exactly once, not under a lambda, and pure — safe to
                // inline regardless of size (no code duplication, no work
                // duplication, no effect reordering).
                inline_lets(subst(body, var.id, &rhs))
            } else if expr_size(&rhs) <= INLINE_THRESHOLD
                && is_pure(&rhs)
                && !preserves_call_boundary
            {
                // Small pure RHS — inline even if used multiple times.
                inline_lets(subst(body, var.id, &rhs))
            } else {
                CoreExpr::Let {
                    var,
                    rhs: Box::new(rhs),
                    body: Box::new(body),
                    span,
                }
            }
        }
        // Never inline recursive bindings.
        CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Lam { .. }
        | CoreExpr::App { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Con { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::MemberAccess { .. }
        | CoreExpr::TupleField { .. } => map_children(expr, inline_lets),
        other => other,
    }
}

// ── Occurrence counting ───────────────────────────────────────────────────────

/// Count the number of free occurrences of `var` in `expr`.
pub(super) fn count_occurrences(var: CoreBinderId, expr: &CoreExpr) -> usize {
    match expr {
        CoreExpr::Var { var: ref_var, .. } => {
            if ref_var.binder == Some(var) {
                1
            } else {
                0
            }
        }
        CoreExpr::Lit(_, _) => 0,
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) {
                0 // shadowed
            } else {
                count_occurrences(var, body)
            }
        }
        CoreExpr::App { func, args, .. } => {
            count_occurrences(var, func)
                + args
                    .iter()
                    .map(|a| count_occurrences(var, a))
                    .sum::<usize>()
        }
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            ..
        } => {
            let rhs_count = count_occurrences(var, rhs);
            if binding.id == var {
                rhs_count // shadowed in body
            } else {
                rhs_count + count_occurrences(var, body)
            }
        }
        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            ..
        } => {
            if binding.id == var {
                0 // shadowed in both rhs and body
            } else {
                count_occurrences(var, rhs) + count_occurrences(var, body)
            }
        }
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            if bindings.iter().any(|(b, _)| b.id == var) {
                0 // shadowed by one of the group binders
            } else {
                bindings
                    .iter()
                    .map(|(_, rhs)| count_occurrences(var, rhs))
                    .sum::<usize>()
                    + count_occurrences(var, body)
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            count_occurrences(var, scrutinee)
                + alts
                    .iter()
                    .map(|alt| {
                        if pat_binds_var(&alt.pat, var) {
                            0
                        } else {
                            alt.guard.as_ref().map_or(0, |g| count_occurrences(var, g))
                                + count_occurrences(var, &alt.rhs)
                        }
                    })
                    .sum::<usize>()
        }
        CoreExpr::Con { fields, .. } => fields.iter().map(|f| count_occurrences(var, f)).sum(),
        CoreExpr::PrimOp { args, .. } => args.iter().map(|a| count_occurrences(var, a)).sum(),
        CoreExpr::Return { value, .. } => count_occurrences(var, value),
        CoreExpr::Perform { args, .. } => args.iter().map(|a| count_occurrences(var, a)).sum(),
        CoreExpr::Handle { body, handlers, .. } => {
            count_occurrences(var, body)
                + handlers
                    .iter()
                    .map(|h| {
                        if h.resume.id == var || h.params.iter().any(|p| p.id == var) {
                            0
                        } else {
                            count_occurrences(var, &h.body)
                        }
                    })
                    .sum::<usize>()
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            count_occurrences(var, object)
        }
    }
}

/// Returns true if `var` occurs free under at least one lambda in `expr`.
///
/// If so, inlining a single-use binding could still duplicate work
/// (the lambda may execute multiple times).
pub(super) fn occurs_under_lambda(var: CoreBinderId, expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) {
                false // shadowed
            } else {
                // Every occurrence inside the lambda body counts as "under lambda".
                count_occurrences(var, body) > 0
            }
        }
        CoreExpr::App { func, args, .. } => {
            occurs_under_lambda(var, func) || args.iter().any(|a| occurs_under_lambda(var, a))
        }
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            ..
        } => occurs_under_lambda(var, rhs) || (binding.id != var && occurs_under_lambda(var, body)),
        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            ..
        } => binding.id != var && (occurs_under_lambda(var, rhs) || occurs_under_lambda(var, body)),
        CoreExpr::LetRecGroup { bindings, body, .. } => {
            !bindings.iter().any(|(b, _)| b.id == var)
                && (bindings
                    .iter()
                    .any(|(_, rhs)| occurs_under_lambda(var, rhs))
                    || occurs_under_lambda(var, body))
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            occurs_under_lambda(var, scrutinee)
                || alts.iter().any(|alt| {
                    !pat_binds_var(&alt.pat, var)
                        && (alt
                            .guard
                            .as_ref()
                            .is_some_and(|g| occurs_under_lambda(var, g))
                            || occurs_under_lambda(var, &alt.rhs))
                })
        }
        CoreExpr::Con { fields, .. } => fields.iter().any(|f| occurs_under_lambda(var, f)),
        CoreExpr::PrimOp { args, .. } => args.iter().any(|a| occurs_under_lambda(var, a)),
        CoreExpr::Return { value, .. } => occurs_under_lambda(var, value),
        CoreExpr::Perform { args, .. } => args.iter().any(|a| occurs_under_lambda(var, a)),
        CoreExpr::Handle { body, handlers, .. } => {
            occurs_under_lambda(var, body)
                || handlers.iter().any(|h| {
                    h.resume.id != var
                        && !h.params.iter().any(|p| p.id == var)
                        && occurs_under_lambda(var, &h.body)
                })
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            occurs_under_lambda(var, object)
        }
    }
}

pub(super) fn occurs_as_callee(var: CoreBinderId, expr: &CoreExpr) -> bool {
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
            if bindings.iter().any(|(b, _)| b.id == var) {
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
                    !pat_binds_var(&alt.pat, var)
                        && (alt.guard.as_ref().is_some_and(|g| occurs_as_callee(var, g))
                            || occurs_as_callee(var, &alt.rhs))
                })
        }
        CoreExpr::Con { fields, .. } => fields.iter().any(|f| occurs_as_callee(var, f)),
        CoreExpr::PrimOp { args, .. } => args.iter().any(|a| occurs_as_callee(var, a)),
        CoreExpr::Return { value, .. } => occurs_as_callee(var, value),
        CoreExpr::Perform { args, .. } => args.iter().any(|a| occurs_as_callee(var, a)),
        CoreExpr::Handle { body, handlers, .. } => {
            occurs_as_callee(var, body)
                || handlers.iter().any(|h| {
                    h.resume.id != var
                        && !h.params.iter().any(|p| p.id == var)
                        && occurs_as_callee(var, &h.body)
                })
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            occurs_as_callee(var, object)
        }
    }
}

// ── Effect analysis ───────────────────────────────────────────────────────────

// ── Pattern helpers ───────────────────────────────────────────────────────────

pub(super) fn pat_binds_var(pat: &CorePat, var: CoreBinderId) -> bool {
    match pat {
        CorePat::Var(binder) => binder.id == var,
        CorePat::Con { fields, .. } => fields.iter().any(|f| pat_binds_var(f, var)),
        CorePat::Tuple(fields) => fields.iter().any(|f| pat_binds_var(f, var)),
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => false,
    }
}
