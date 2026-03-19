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

/// Count how many times `var` appears in an **owned** (consuming) position.
///
/// A use is "owned" if it stores, returns, or captures the value.
/// A use is "borrowed" if it only reads (PrimOp operand, Case scrutinee,
/// function position of App).
///
/// Returns the number of owned uses. If 0, all uses are borrowed and no
/// Dup is needed regardless of total use count.
pub fn owned_use_count(var: CoreBinderId, expr: &CoreExpr) -> usize {
    count_owned(var, expr)
}

fn count_owned(var: CoreBinderId, expr: &CoreExpr) -> usize {
    match expr {
        // Atoms: a bare Var reference is not in any specific position.
        // It only appears as a subexpression of something else after ANF.
        // If it appears bare (e.g., as the body of a Let), it's a return → owned.
        CoreExpr::Var { var: ref_var, .. } => {
            if ref_var.binder == Some(var) { 1 } else { 0 }
        }
        CoreExpr::Lit(_, _) => 0,

        // PrimOp operands are READ-ONLY (borrowed).
        // If var appears as a PrimOp arg, it's borrowed → count 0.
        CoreExpr::PrimOp { args, .. } => {
            // Args are trivial after ANF (Var or Lit).
            // Each arg that matches var is a borrowed use → not counted.
            args.iter().map(|a| count_owned_skip_direct(var, a)).sum::<usize>()
        }

        // Case scrutinee is READ-ONLY (borrowed). Pattern RHS is normal.
        CoreExpr::Case { scrutinee, alts, .. } => {
            let scrut = count_owned_skip_direct(var, scrutinee);
            let alts_owned: usize = alts.iter().map(|alt| {
                if pat_binds(var, &alt.pat) {
                    0 // shadowed by pattern
                } else {
                    let rhs = count_owned(var, &alt.rhs);
                    let guard = alt.guard.as_ref().map_or(0, |g| count_owned(var, g));
                    rhs + guard
                }
            }).sum();
            scrut + alts_owned
        }

        // App: function position is borrowed, arguments are OWNED (conservative).
        CoreExpr::App { func, args, .. } => {
            let func_owned = count_owned_skip_direct(var, func);
            let args_owned: usize = args.iter().map(|a| count_owned(var, a)).sum();
            func_owned + args_owned
        }

        // Con fields are OWNED (stored in data structure).
        CoreExpr::Con { fields, .. } => {
            fields.iter().map(|f| count_owned(var, f)).sum()
        }

        // Return value is OWNED (escapes scope).
        CoreExpr::Return { value, .. } => count_owned(var, value),

        // Lam: if var is captured (free in body), it's OWNED.
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) {
                0 // shadowed
            } else {
                // Any free occurrence inside a Lam is a capture → owned
                count_owned_in_capture(var, body)
            }
        }

        // Let: recurse into rhs and body with proper scoping
        CoreExpr::Let { var: binding, rhs, body, .. } => {
            let rhs_owned = count_owned(var, rhs);
            if binding.id == var {
                rhs_owned // shadowed in body
            } else {
                rhs_owned + count_owned(var, body)
            }
        }

        CoreExpr::LetRec { var: binding, rhs, body, .. } => {
            if binding.id == var {
                0 // shadowed in both
            } else {
                count_owned(var, rhs) + count_owned(var, body)
            }
        }

        // Perform args are OWNED (continuation capture boundary).
        CoreExpr::Perform { args, .. } => {
            args.iter().map(|a| count_owned(var, a)).sum()
        }

        // Handle: body is normal context, handler bodies have their own scope.
        CoreExpr::Handle { body, handlers, .. } => {
            let body_owned = count_owned(var, body);
            let handlers_owned: usize = handlers.iter().map(|h| {
                if h.resume.id == var || h.params.iter().any(|p| p.id == var) {
                    0
                } else {
                    count_owned(var, &h.body)
                }
            }).sum();
            body_owned + handlers_owned
        }

        // Dup/Drop: transparent
        CoreExpr::Dup { var: dup_var, body, .. } => {
            let dup_use = if dup_var.binder == Some(var) { 1 } else { 0 };
            dup_use + count_owned(var, body)
        }
        CoreExpr::Drop { body, .. } => count_owned(var, body),
    }
}

/// Count owned uses, but if the expression is a direct Var reference to `var`,
/// return 0 (the variable is in a borrowed position — the caller handles this).
fn count_owned_skip_direct(var: CoreBinderId, expr: &CoreExpr) -> usize {
    match expr {
        CoreExpr::Var { .. } => 0, // Direct reference in borrowed position — skip
        _ => count_owned(var, expr),
    }
}

/// Inside a Lam body, every free occurrence of var is a capture → owned.
fn count_owned_in_capture(var: CoreBinderId, expr: &CoreExpr) -> usize {
    total_occurrences(var, expr)
}

/// Count total free occurrences of `var` in `expr` (regardless of position).
fn total_occurrences(var: CoreBinderId, expr: &CoreExpr) -> usize {
    match expr {
        CoreExpr::Var { var: ref_var, .. } => {
            if ref_var.binder == Some(var) { 1 } else { 0 }
        }
        CoreExpr::Lit(_, _) => 0,
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) { 0 } else { total_occurrences(var, body) }
        }
        CoreExpr::App { func, args, .. } => {
            total_occurrences(var, func) + args.iter().map(|a| total_occurrences(var, a)).sum::<usize>()
        }
        CoreExpr::Let { var: b, rhs, body, .. } => {
            let r = total_occurrences(var, rhs);
            if b.id == var { r } else { r + total_occurrences(var, body) }
        }
        CoreExpr::LetRec { var: b, rhs, body, .. } => {
            if b.id == var { 0 } else { total_occurrences(var, rhs) + total_occurrences(var, body) }
        }
        CoreExpr::Case { scrutinee, alts, .. } => {
            total_occurrences(var, scrutinee) + alts.iter().map(|alt| {
                if pat_binds(var, &alt.pat) { 0 }
                else { total_occurrences(var, &alt.rhs) + alt.guard.as_ref().map_or(0, |g| total_occurrences(var, g)) }
            }).sum::<usize>()
        }
        CoreExpr::Con { fields, .. } => fields.iter().map(|f| total_occurrences(var, f)).sum(),
        CoreExpr::PrimOp { args, .. } => args.iter().map(|a| total_occurrences(var, a)).sum(),
        CoreExpr::Return { value, .. } => total_occurrences(var, value),
        CoreExpr::Perform { args, .. } => args.iter().map(|a| total_occurrences(var, a)).sum(),
        CoreExpr::Handle { body, handlers, .. } => {
            total_occurrences(var, body) + handlers.iter().map(|h| {
                if h.resume.id == var || h.params.iter().any(|p| p.id == var) { 0 }
                else { total_occurrences(var, &h.body) }
            }).sum::<usize>()
        }
        CoreExpr::Dup { var: v, body, .. } => {
            (if v.binder == Some(var) { 1 } else { 0 }) + total_occurrences(var, body)
        }
        CoreExpr::Drop { body, .. } => total_occurrences(var, body),
    }
}

/// Check if a pattern binds the given variable (shadows it).
fn pat_binds(var: CoreBinderId, pat: &CorePat) -> bool {
    match pat {
        CorePat::Var(b) => b.id == var,
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => fields.iter().any(|f| pat_binds(var, f)),
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => false,
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
