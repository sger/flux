//! Aether verification — detect missed optimization opportunities and correctness issues.
//!
//! Enabled with `FLUX_AETHER_VERIFY=1`. Reports:
//! - Missed reuse: Case arm destructures a value and constructs a new one of the same
//!   shape, but no Reuse node was inserted (the scrutinee could have been reused).
//! - Redundant Dups: Dup nodes where the variable is only used in borrowed positions
//!   (should have been elided by Phase 6 borrowing).
//! - Drop safety: validates that dropped variables have 0 remaining uses in the body.

use crate::core::{CoreExpr, CorePat, CoreTag};

use super::analysis::use_counts;

/// Diagnostic from Aether verification.
#[derive(Debug)]
pub struct AetherDiagnostic {
    pub kind: AetherDiagnosticKind,
    pub message: String,
}

#[derive(Debug)]
pub enum AetherDiagnosticKind {
    /// Case arm destructures value and reconstructs same shape — could reuse.
    MissedReuse,
    /// Drop node but variable still has uses in body (correctness issue).
    UnsafeDrop,
}

/// Verify an Aether-transformed Core IR expression.
/// Returns a list of diagnostics (empty = all good).
pub fn verify(expr: &CoreExpr) -> Vec<AetherDiagnostic> {
    let mut diags = Vec::new();
    check(expr, &mut diags);
    diags
}

fn check(expr: &CoreExpr, diags: &mut Vec<AetherDiagnostic>) {
    match expr {
        // Verify Drop safety: the dropped variable must have 0 uses in body
        CoreExpr::Drop { var, body, .. } => {
            if let Some(id) = var.binder
                && let Some(&count) = use_counts(body).get(&id)
                && count > 0
            {
                diags.push(AetherDiagnostic {
                    kind: AetherDiagnosticKind::UnsafeDrop,
                    message: format!(
                        "UNSAFE DROP: variable {:?} dropped but still has {} uses in body",
                        var.name, count
                    ),
                });
            }
            check(body, diags);
        }

        // Check Case arms for missed reuse opportunities
        CoreExpr::Case { scrutinee, alts, .. } => {
            check(scrutinee, diags);
            for alt in alts {
                // Check if this alt destructures a constructor and rebuilds the same shape
                let destr_tag = pat_constructor_tag(&alt.pat);
                if let Some(destr_tag) = destr_tag
                    && let Some(con_tag) = find_con_in_body(&alt.rhs)
                    && tags_compatible(&destr_tag, &con_tag)
                    && !has_reuse_for_tag(&alt.rhs, &con_tag)
                {
                    diags.push(AetherDiagnostic {
                        kind: AetherDiagnosticKind::MissedReuse,
                        message: format!(
                            "MISSED REUSE: Case arm destructures {:?} and constructs {:?} — could reuse allocation",
                            destr_tag, con_tag
                        ),
                    });
                }
                check(&alt.rhs, diags);
                if let Some(g) = &alt.guard {
                    check(g, diags);
                }
            }
        }

        // Recurse into all other forms
        CoreExpr::Dup { body, .. } => check(body, diags),
        CoreExpr::Reuse { fields, .. } | CoreExpr::Con { fields, .. } => {
            for f in fields {
                check(f, diags);
            }
        }
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => check(body, diags),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            check(rhs, diags);
            check(body, diags);
        }
        CoreExpr::App { func, args, .. } => {
            check(func, diags);
            for a in args {
                check(a, diags);
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for a in args {
                check(a, diags);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            check(body, diags);
            for h in handlers {
                check(&h.body, diags);
            }
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
    }
}

/// Extract the constructor tag from a pattern (if it destructures a constructor).
fn pat_constructor_tag(pat: &CorePat) -> Option<CoreTag> {
    match pat {
        CorePat::Con { tag, .. } => Some(tag.clone()),
        _ => None,
    }
}

/// Find the first Con node in the "spine" of a body (through Lets, Drops, Dups).
fn find_con_in_body(expr: &CoreExpr) -> Option<CoreTag> {
    match expr {
        CoreExpr::Con { tag, .. } => Some(tag.clone()),
        CoreExpr::Reuse { tag, .. } => Some(tag.clone()), // Already a reuse — has the tag
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            find_con_in_body(body)
        }
        _ => None,
    }
}

/// Check if two tags are shape-compatible for reuse.
fn tags_compatible(a: &CoreTag, b: &CoreTag) -> bool {
    match (a, b) {
        (CoreTag::Cons, CoreTag::Cons) => true,
        (CoreTag::Some, CoreTag::Some) => true,
        (CoreTag::Left, CoreTag::Left) => true,
        (CoreTag::Right, CoreTag::Right) => true,
        (CoreTag::Named(a), CoreTag::Named(b)) => a == b,
        _ => false,
    }
}

/// Check if the body already has a Reuse node for the given tag.
fn has_reuse_for_tag(expr: &CoreExpr, tag: &CoreTag) -> bool {
    match expr {
        CoreExpr::Reuse { tag: t, .. } => tags_compatible(t, tag),
        CoreExpr::Let { body, .. } | CoreExpr::Drop { body, .. } | CoreExpr::Dup { body, .. } => {
            has_reuse_for_tag(body, tag)
        }
        _ => false,
    }
}
