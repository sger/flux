use std::collections::HashSet;

use crate::aether::AetherExpr;
use crate::core::{CoreBinderId, CorePat};

pub fn collect_free_vars_aether(expr: &AetherExpr) -> HashSet<CoreBinderId> {
    let mut free = HashSet::new();
    free_vars_rec_aether(expr, &mut HashSet::new(), &mut free);
    free
}

pub fn free_vars_rec_aether(
    expr: &AetherExpr,
    bound: &mut HashSet<CoreBinderId>,
    free: &mut HashSet<CoreBinderId>,
) {
    match expr {
        AetherExpr::Var { var, .. } => {
            if let Some(binder) = var.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
        }
        AetherExpr::Lit(_, _) => {}
        AetherExpr::Lam { params, body, .. } => {
            let new_params: Vec<_> = params
                .iter()
                .filter(|p| bound.insert(p.id))
                .copied()
                .collect();
            free_vars_rec_aether(body, bound, free);
            for p in new_params {
                bound.remove(&p.id);
            }
        }
        AetherExpr::App { func, args, .. } | AetherExpr::AetherCall { func, args, .. } => {
            free_vars_rec_aether(func, bound, free);
            for arg in args {
                free_vars_rec_aether(arg, bound, free);
            }
        }
        AetherExpr::Let { var, rhs, body, .. } | AetherExpr::LetRec { var, rhs, body, .. } => {
            if matches!(expr, AetherExpr::LetRec { .. }) {
                let is_new = bound.insert(var.id);
                free_vars_rec_aether(rhs, bound, free);
                free_vars_rec_aether(body, bound, free);
                if is_new {
                    bound.remove(&var.id);
                }
            } else {
                free_vars_rec_aether(rhs, bound, free);
                let is_new = bound.insert(var.id);
                free_vars_rec_aether(body, bound, free);
                if is_new {
                    bound.remove(&var.id);
                }
            }
        }
        AetherExpr::LetRecGroup { bindings, body, .. } => {
            let new_binders: Vec<_> = bindings
                .iter()
                .filter(|(var, _)| bound.insert(var.id))
                .map(|(var, _)| var.id)
                .collect();
            for (_, rhs) in bindings {
                free_vars_rec_aether(rhs, bound, free);
            }
            free_vars_rec_aether(body, bound, free);
            for id in new_binders {
                bound.remove(&id);
            }
        }
        AetherExpr::Case {
            scrutinee, alts, ..
        } => {
            free_vars_rec_aether(scrutinee, bound, free);
            for alt in alts {
                let mut alt_bound = HashSet::new();
                collect_pat_binders(&alt.pat, &mut alt_bound);
                let new_binders: Vec<_> = alt_bound
                    .iter()
                    .filter(|binder| bound.insert(**binder))
                    .copied()
                    .collect();
                if let Some(guard) = &alt.guard {
                    free_vars_rec_aether(guard, bound, free);
                }
                free_vars_rec_aether(&alt.rhs, bound, free);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        AetherExpr::Con { fields, .. }
        | AetherExpr::PrimOp { args: fields, .. }
        | AetherExpr::Perform { args: fields, .. } => {
            for field in fields {
                free_vars_rec_aether(field, bound, free);
            }
        }
        AetherExpr::MemberAccess { object, .. } | AetherExpr::TupleField { object, .. } => {
            free_vars_rec_aether(object, bound, free);
        }
        AetherExpr::Return { value, .. } => free_vars_rec_aether(value, bound, free),
        AetherExpr::Handle { body, handlers, .. } => {
            free_vars_rec_aether(body, bound, free);
            for handler in handlers {
                let mut new_binders = Vec::new();
                if bound.insert(handler.resume.id) {
                    new_binders.push(handler.resume.id);
                }
                for p in &handler.params {
                    if bound.insert(p.id) {
                        new_binders.push(p.id);
                    }
                }
                free_vars_rec_aether(&handler.body, bound, free);
                for binder in new_binders {
                    bound.remove(&binder);
                }
            }
        }
        AetherExpr::Dup { var, body, .. } | AetherExpr::Drop { var, body, .. } => {
            if let Some(binder) = var.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
            free_vars_rec_aether(body, bound, free);
        }
        AetherExpr::Reuse { token, fields, .. } => {
            if let Some(binder) = token.binder
                && !bound.contains(&binder)
            {
                free.insert(binder);
            }
            for field in fields {
                free_vars_rec_aether(field, bound, free);
            }
        }
        AetherExpr::DropSpecialized {
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
            free_vars_rec_aether(unique_body, bound, free);
            free_vars_rec_aether(shared_body, bound, free);
        }
    }
}

pub fn collect_pat_binders(pat: &CorePat, out: &mut HashSet<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => {
            out.insert(binder.id);
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for field in fields {
                collect_pat_binders(field, out);
            }
        }
        CorePat::Wildcard | CorePat::Lit(_) | CorePat::EmptyList => {}
    }
}
