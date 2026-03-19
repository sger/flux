//! Aether use-count analysis for dup/drop insertion.
//!
//! Computes how many times each bound variable is referenced within a Core IR
//! expression. This information drives the dup/drop insertion rules:
//! - 0 uses → insert Drop
//! - 1 use  → ownership transfer (no dup/drop)
//! - N uses → insert (N-1) Dups

use std::collections::HashMap;

use crate::core::{CoreBinderId, CoreExpr, CorePat};

/// Count free-variable occurrences in `expr`, respecting scoping.
///
/// Returns a map from binder ID to the number of times that variable appears
/// free (unbound) in the expression. Variables bound within the expression
/// (by Let, Lam, Case patterns, Handle params) are excluded from the count
/// for regions where they are in scope.
pub fn use_counts(expr: &CoreExpr) -> HashMap<CoreBinderId, usize> {
    let mut counts = HashMap::new();
    count_uses(expr, &mut counts);
    counts
}

fn count_uses(expr: &CoreExpr, counts: &mut HashMap<CoreBinderId, usize>) {
    match expr {
        CoreExpr::Var { var, .. } => {
            if let Some(id) = var.binder {
                *counts.entry(id).or_insert(0) += 1;
            }
        }
        CoreExpr::Lit(_, _) => {}
        CoreExpr::Lam { params, body, .. } => {
            let mut inner = HashMap::new();
            count_uses(body, &mut inner);
            // Remove params (they're bound, not free)
            for p in params {
                inner.remove(&p.id);
            }
            merge_counts(counts, &inner);
        }
        CoreExpr::App { func, args, .. } => {
            count_uses(func, counts);
            for a in args {
                count_uses(a, counts);
            }
        }
        CoreExpr::Let {
            var, rhs, body, ..
        } => {
            count_uses(rhs, counts);
            let mut body_counts = HashMap::new();
            count_uses(body, &mut body_counts);
            body_counts.remove(&var.id);
            merge_counts(counts, &body_counts);
        }
        CoreExpr::LetRec {
            var, rhs, body, ..
        } => {
            // var is in scope in both rhs and body
            let mut inner = HashMap::new();
            count_uses(rhs, &mut inner);
            count_uses(body, &mut inner);
            inner.remove(&var.id);
            merge_counts(counts, &inner);
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            count_uses(scrutinee, counts);
            for alt in alts {
                let mut alt_counts = HashMap::new();
                count_uses(&alt.rhs, &mut alt_counts);
                if let Some(g) = &alt.guard {
                    count_uses(g, &mut alt_counts);
                }
                // Remove pattern-bound variables
                remove_pat_bindings(&alt.pat, &mut alt_counts);
                merge_counts(counts, &alt_counts);
            }
        }
        CoreExpr::Con { fields, .. } => {
            for f in fields {
                count_uses(f, counts);
            }
        }
        CoreExpr::PrimOp { args, .. } => {
            for a in args {
                count_uses(a, counts);
            }
        }
        CoreExpr::Return { value, .. } => count_uses(value, counts),
        CoreExpr::Perform { args, .. } => {
            for a in args {
                count_uses(a, counts);
            }
        }
        CoreExpr::Handle {
            body, handlers, ..
        } => {
            count_uses(body, counts);
            for h in handlers {
                let mut h_counts = HashMap::new();
                count_uses(&h.body, &mut h_counts);
                h_counts.remove(&h.resume.id);
                for p in &h.params {
                    h_counts.remove(&p.id);
                }
                merge_counts(counts, &h_counts);
            }
        }
        // Dup/Drop are transparent for analysis (shouldn't exist pre-pass,
        // but handle them for correctness if the pass runs multiple times).
        CoreExpr::Dup { var, body, .. } => {
            if let Some(id) = var.binder {
                *counts.entry(id).or_insert(0) += 1;
            }
            count_uses(body, counts);
        }
        CoreExpr::Drop { body, .. } => {
            count_uses(body, counts);
        }
    }
}

/// Count uses within a single Case alternative's RHS (and guard),
/// excluding pattern-bound variables.
pub fn alt_use_counts(alt: &crate::core::CoreAlt) -> HashMap<CoreBinderId, usize> {
    let mut counts = HashMap::new();
    count_uses(&alt.rhs, &mut counts);
    if let Some(g) = &alt.guard {
        count_uses(g, &mut counts);
    }
    remove_pat_bindings(&alt.pat, &mut counts);
    counts
}

fn remove_pat_bindings(pat: &CorePat, counts: &mut HashMap<CoreBinderId, usize>) {
    match pat {
        CorePat::Var(binder) => {
            counts.remove(&binder.id);
        }
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for f in fields {
                remove_pat_bindings(f, counts);
            }
        }
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => {}
    }
}

/// Collect all binder IDs introduced by a pattern.
pub fn pat_binders(pat: &CorePat) -> Vec<CoreBinderId> {
    let mut out = Vec::new();
    collect_pat_binders(pat, &mut out);
    out
}

fn collect_pat_binders(pat: &CorePat, out: &mut Vec<CoreBinderId>) {
    match pat {
        CorePat::Var(binder) => out.push(binder.id),
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            for f in fields {
                collect_pat_binders(f, out);
            }
        }
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => {}
    }
}

fn merge_counts(
    target: &mut HashMap<CoreBinderId, usize>,
    source: &HashMap<CoreBinderId, usize>,
) {
    for (&id, &count) in source {
        *target.entry(id).or_insert(0) += count;
    }
}
