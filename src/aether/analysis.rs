//! Aether use-count analysis for dup/drop insertion.
//!
//! Computes how many times each bound variable is referenced within a Core IR
//! expression. This information drives the dup/drop insertion rules:
//! - 0 uses → insert Drop
//! - 1 use  → ownership transfer (no dup/drop)
//! - N uses → insert (N-1) Dups

use std::collections::{HashMap, HashSet};

use crate::core::{CoreBinderId, CoreExpr, CorePat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueDemand {
    Ignore,
    Borrowed,
    Owned,
}

/// Environment tracked by the reverse Aether planner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AetherEnv {
    pub live: HashSet<CoreBinderId>,
    pub owned: HashSet<CoreBinderId>,
    pub borrowed: HashSet<CoreBinderId>,
}

impl AetherEnv {
    pub fn mark_owned(&mut self, binder: CoreBinderId) {
        self.live.insert(binder);
        self.owned.insert(binder);
        self.borrowed.remove(&binder);
    }

    pub fn mark_borrowed(&mut self, binder: CoreBinderId) {
        self.live.insert(binder);
        if !self.owned.contains(&binder) {
            self.borrowed.insert(binder);
        }
    }

    pub fn remove(&mut self, binder: CoreBinderId) {
        self.live.remove(&binder);
        self.owned.remove(&binder);
        self.borrowed.remove(&binder);
    }

    pub fn remove_all<I>(&mut self, binders: I)
    where
        I: IntoIterator<Item = CoreBinderId>,
    {
        for binder in binders {
            self.remove(binder);
        }
    }

    pub fn is_live(&self, binder: CoreBinderId) -> bool {
        self.live.contains(&binder)
    }

    pub fn is_owned(&self, binder: CoreBinderId) -> bool {
        self.owned.contains(&binder)
    }

    pub fn is_borrowed(&self, binder: CoreBinderId) -> bool {
        self.borrowed.contains(&binder) && !self.owned.contains(&binder)
    }

    pub fn union_from(&mut self, other: &Self) {
        self.live.extend(other.live.iter().copied());
        self.owned.extend(other.owned.iter().copied());
        for binder in other.borrowed.iter().copied() {
            self.live.insert(binder);
            if !self.owned.contains(&binder) {
                self.borrowed.insert(binder);
            }
        }
        self.borrowed.retain(|binder| !self.owned.contains(binder));
    }
}

#[derive(Debug, Clone)]
pub struct AetherPlan {
    pub expr: CoreExpr,
    pub env_before: AetherEnv,
}

pub fn join_branch_envs(branches: &[AetherEnv]) -> AetherEnv {
    let mut joined = AetherEnv::default();
    for branch in branches {
        joined.union_from(branch);
    }
    joined
}

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
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            count_uses(func, counts);
            for a in args {
                count_uses(a, counts);
            }
        }
        CoreExpr::Let { var, rhs, body, .. } => {
            count_uses(rhs, counts);
            let mut body_counts = HashMap::new();
            count_uses(body, &mut body_counts);
            body_counts.remove(&var.id);
            merge_counts(counts, &body_counts);
        }
        CoreExpr::LetRec { var, rhs, body, .. } => {
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
        CoreExpr::Handle { body, handlers, .. } => {
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
        // Reuse: token is a use + recurse into fields (same as Con).
        CoreExpr::Reuse { token, fields, .. } => {
            if let Some(id) = token.binder {
                *counts.entry(id).or_insert(0) += 1;
            }
            for f in fields {
                count_uses(f, counts);
            }
        }
        // DropSpecialized: count scrutinee var + take max of both branches
        // (like Case arms — only one branch executes at runtime).
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            if let Some(id) = scrutinee.binder {
                *counts.entry(id).or_insert(0) += 1;
            }
            let mut unique_counts = HashMap::new();
            count_uses(unique_body, &mut unique_counts);
            let mut shared_counts = HashMap::new();
            count_uses(shared_body, &mut shared_counts);
            // Merge taking max (only one branch runs).
            let all_keys: std::collections::HashSet<_> = unique_counts
                .keys()
                .chain(shared_counts.keys())
                .copied()
                .collect();
            for id in all_keys {
                let u = unique_counts.get(&id).copied().unwrap_or(0);
                let s = shared_counts.get(&id).copied().unwrap_or(0);
                *counts.entry(id).or_insert(0) += u.max(s);
            }
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
    count_owned_inner(var, expr, None)
}

/// Like `owned_use_count`, but consults a `BorrowRegistry` to determine
/// if App arguments at borrowed parameter positions should be counted as
/// borrowed (not owned). This eliminates unnecessary Rc::clone at call sites.
pub fn owned_use_count_with_registry(
    var: CoreBinderId,
    expr: &CoreExpr,
    registry: &super::borrow_infer::BorrowRegistry,
) -> usize {
    count_owned_inner(var, expr, Some(registry))
}

fn count_owned_inner(
    var: CoreBinderId,
    expr: &CoreExpr,
    registry: Option<&super::borrow_infer::BorrowRegistry>,
) -> usize {
    match expr {
        // Atoms: a bare Var reference is not in any specific position.
        // It only appears as a subexpression of something else after ANF.
        // If it appears bare (e.g., as the body of a Let), it's a return → owned.
        CoreExpr::Var { var: ref_var, .. } => {
            if ref_var.binder == Some(var) {
                1
            } else {
                0
            }
        }
        CoreExpr::Lit(_, _) => 0,

        // PrimOp operands are READ-ONLY (borrowed).
        // If var appears as a PrimOp arg, it's borrowed → count 0.
        CoreExpr::PrimOp { args, .. } => {
            // Args are trivial after ANF (Var or Lit).
            // Each arg that matches var is a borrowed use → not counted.
            args.iter()
                .map(|a| count_owned_skip_direct(var, a, registry))
                .sum::<usize>()
        }

        // Case scrutinee is READ-ONLY (borrowed). Pattern RHS is normal.
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            let scrut = count_owned_skip_direct(var, scrutinee, registry);
            let alts_owned: usize = alts
                .iter()
                .map(|alt| {
                    if pat_binds(var, &alt.pat) {
                        0 // shadowed by pattern
                    } else {
                        let rhs = count_owned_inner(var, &alt.rhs, registry);
                        let guard = alt
                            .guard
                            .as_ref()
                            .map_or(0, |g| count_owned_inner(var, g, registry));
                        rhs + guard
                    }
                })
                .sum();
            scrut + alts_owned
        }

        // App: function position is borrowed. Arguments are owned UNLESS the
        // callee declares the corresponding parameter as Borrowed (via registry).
        CoreExpr::App { func, args, .. } => {
            let func_owned = count_owned_skip_direct(var, func, registry);

            let resolved_callee = registry.and_then(|reg| match func.as_ref() {
                CoreExpr::Var {
                    var: callee_var, ..
                } => Some(reg.resolve_var_ref(callee_var)),
                _ => None,
            });

            let args_owned: usize = args
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let param_borrowed = resolved_callee
                        .zip(registry)
                        .is_some_and(|(callee, reg)| reg.is_borrowed(callee, i));

                    if param_borrowed {
                        // Callee borrows this param — skip as borrowed use
                        count_owned_skip_direct(var, a, registry)
                    } else {
                        // Callee owns this param — count as owned
                        count_owned_inner(var, a, registry)
                    }
                })
                .sum();
            func_owned + args_owned
        }
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            ..
        } => {
            let func_owned = count_owned_skip_direct(var, func, registry);
            let args_owned: usize = args
                .iter()
                .zip(arg_modes.iter())
                .map(|(arg, mode)| match mode {
                    super::borrow_infer::BorrowMode::Borrowed => {
                        count_owned_skip_direct(var, arg, registry)
                    }
                    super::borrow_infer::BorrowMode::Owned => count_owned_inner(var, arg, registry),
                })
                .sum();
            func_owned + args_owned
        }

        // Con fields are OWNED (stored in data structure).
        CoreExpr::Con { fields, .. } => fields
            .iter()
            .map(|f| count_owned_inner(var, f, registry))
            .sum(),

        // Return value is OWNED (escapes scope).
        CoreExpr::Return { value, .. } => count_owned_inner(var, value, registry),

        // Lam: captured values are owned only when the closure body consumes
        // them. Read-only captures can stay borrowed.
        CoreExpr::Lam { params, body, .. } => {
            if params.iter().any(|p| p.id == var) {
                0 // shadowed
            } else {
                count_owned_in_capture(var, body, registry)
            }
        }

        // Let: recurse into rhs and body with proper scoping
        CoreExpr::Let {
            var: binding,
            rhs,
            body,
            ..
        } => {
            let rhs_owned = count_owned_inner(var, rhs, registry);
            if binding.id == var {
                rhs_owned // shadowed in body
            } else {
                rhs_owned + count_owned_inner(var, body, registry)
            }
        }

        CoreExpr::LetRec {
            var: binding,
            rhs,
            body,
            ..
        } => {
            if binding.id == var {
                0 // shadowed in both
            } else {
                count_owned_inner(var, rhs, registry) + count_owned_inner(var, body, registry)
            }
        }

        // Perform args are OWNED (continuation capture boundary).
        CoreExpr::Perform { args, .. } => args
            .iter()
            .map(|a| count_owned_inner(var, a, registry))
            .sum(),

        // Handle: body is normal context, handler bodies have their own scope.
        CoreExpr::Handle { body, handlers, .. } => {
            let body_owned = count_owned_inner(var, body, registry);
            let handlers_owned: usize = handlers
                .iter()
                .map(|h| {
                    if h.resume.id == var || h.params.iter().any(|p| p.id == var) {
                        0
                    } else {
                        count_owned_inner(var, &h.body, registry)
                    }
                })
                .sum();
            body_owned + handlers_owned
        }

        // Dup/Drop: transparent
        CoreExpr::Dup {
            var: dup_var, body, ..
        } => {
            let dup_use = if dup_var.binder == Some(var) { 1 } else { 0 };
            dup_use + count_owned_inner(var, body, registry)
        }
        CoreExpr::Drop { body, .. } => count_owned_inner(var, body, registry),
        // Reuse: token is owned (reuse consumes the token), fields are owned (stored).
        CoreExpr::Reuse { token, fields, .. } => {
            let token_use = if token.binder == Some(var) { 1 } else { 0 };
            token_use
                + fields
                    .iter()
                    .map(|f| count_owned_inner(var, f, registry))
                    .sum::<usize>()
        }
        // DropSpecialized: scrutinee is borrowed (tested for uniqueness),
        // branches are normal context — take max (only one runs).
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            ..
        } => {
            let scrut = if scrutinee.binder == Some(var) { 0 } else { 0 }; // borrowed
            let _ = scrut;
            let u = count_owned_inner(var, unique_body, registry);
            let s = count_owned_inner(var, shared_body, registry);
            u.max(s)
        }
    }
}

/// Count owned uses, but if the expression is a direct Var reference to `var`,
/// return 0 (the variable is in a borrowed position — the caller handles this).
fn count_owned_skip_direct(
    var: CoreBinderId,
    expr: &CoreExpr,
    registry: Option<&super::borrow_infer::BorrowRegistry>,
) -> usize {
    match expr {
        CoreExpr::Var { .. } => 0, // Direct reference in borrowed position — skip
        _ => count_owned_inner(var, expr, registry),
    }
}

/// Inside a Lam body, only consuming free occurrences of `var` force the
/// capture to be owned. Borrow-only captures can remain borrowed.
fn count_owned_in_capture(
    var: CoreBinderId,
    expr: &CoreExpr,
    registry: Option<&super::borrow_infer::BorrowRegistry>,
) -> usize {
    count_owned_inner(var, expr, registry)
}

/// Check if a pattern binds the given variable (shadows it).
fn pat_binds(var: CoreBinderId, pat: &CorePat) -> bool {
    match pat {
        CorePat::Var(b) => b.id == var,
        CorePat::Con { fields, .. } | CorePat::Tuple(fields) => {
            fields.iter().any(|f| pat_binds(var, f))
        }
        CorePat::Lit(_) | CorePat::Wildcard | CorePat::EmptyList => false,
    }
}

fn merge_counts(target: &mut HashMap<CoreBinderId, usize>, source: &HashMap<CoreBinderId, usize>) {
    for (&id, &count) in source {
        *target.entry(id).or_insert(0) += count;
    }
}
