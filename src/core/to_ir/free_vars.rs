use std::collections::HashSet;

use crate::core::{CoreBinderId, CoreExpr, CorePat};

/// Collect all free (unbound) variables in a `CoreExpr`.
pub fn collect_free_vars_core(expr: &CoreExpr) -> HashSet<CoreBinderId> {
    let mut free = HashSet::new();
    free_vars_rec(expr, &mut HashSet::new(), &mut free);
    free
}

pub(super) fn free_vars_rec(
    expr: &CoreExpr,
    bound: &mut HashSet<CoreBinderId>,
    free: &mut HashSet<CoreBinderId>,
) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(binder) = var.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .copied()
                .collect();
            free_vars_rec(body, bound, free);
            for p in new_params {
                bound.remove(&p.id);
            }
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            free_vars_rec(func, bound, free);
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            free_vars_rec(rhs, bound, free);
            let is_new = bound.insert(var.id);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(&var.id);
            }
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
            let is_new = bound.insert(var.id);
            free_vars_rec(rhs, bound, free);
            free_vars_rec(body, bound, free);
            if is_new {
                bound.remove(&var.id);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            free_vars_rec(scrutinee, bound, free);
            for alt in alts {
                let mut alt_bound = HashSet::new();
                collect_pat_binders(&alt.pat, &mut alt_bound);
                let new_binders: Vec<_> = alt_bound
                    .iter()
                    .filter(|b| bound.insert(**b))
                    .copied()
                    .collect();
                if let Some(guard) = &alt.guard {
                    free_vars_rec(guard, bound, free);
                }
                free_vars_rec(&alt.rhs, bound, free);
                for b in new_binders {
                    bound.remove(&b);
                }
            }
        }
        CoreExpr::Con { fields, .. } => {
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::Return { value, .. } => free_vars_rec(value, bound, free),
        CoreExpr::PrimOp { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Perform { args, .. } => {
            for a in args {
                free_vars_rec(a, bound, free);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            free_vars_rec(body, bound, free);
            for h in handlers {
                let mut new_binders = Vec::new();
                if bound.insert(h.resume.id) {
                    new_binders.push(h.resume.id);
                }
                for p in &h.params {
                    if bound.insert(p.id) {
                        new_binders.push(p.id);
                    }
                }
                free_vars_rec(&h.body, bound, free);
                for b in new_binders {
                    bound.remove(&b);
                }
            }
        }
        CoreExpr::Dup { var, body, .. } | CoreExpr::Drop { var, body, .. } => {
            if let Some(binder) = var.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
            free_vars_rec(body, bound, free);
        }
        CoreExpr::Reuse { token, fields, .. } => {
            if let Some(binder) = token.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
            for f in fields {
                free_vars_rec(f, bound, free);
            }
        }
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            if let Some(binder) = scrutinee.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
            free_vars_rec(unique_body, bound, free);
            free_vars_rec(shared_body, bound, free);
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            free_vars_rec(object, bound, free);
        }
    }
}

pub(super) fn collect_pat_binders(pat: &CorePat, out: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            out.insert(binder.id);
        }
        CorePat::Con { fields, .. } => {
            for f in fields {
                collect_pat_binders(f, out);
            }
        }
        CorePat::Tuple(fields) => {
            for f in fields {
                collect_pat_binders(f, out);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}
