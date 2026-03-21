//! Aether Phase 9: Dup/Drop fusion (Perceus Section 2.3).
//!
//! After drop specialization creates unique/shared branches, matching
//! `Dup(x)` / `Drop(x)` pairs can be cancelled — even when non-adjacent.
//!
//! The spine-based algorithm collects all leading Dup/Drop operations on
//! a spine, cancels matching pairs regardless of order, then re-wraps
//! the remaining operations. This handles patterns like:
//!
//! ```text
//! Dup(h); Dup(t); Drop(h); Drop(t); free(xs); body
//!   → free(xs); body                    // all pairs cancelled
//!
//! Dup(h); Dup(f); Drop(h); body
//!   → Dup(f); body                      // h cancelled, f kept
//! ```

use crate::core::{CoreExpr, CoreVarRef};
use crate::diagnostics::position::Span;

/// Run dup/drop fusion on a Core IR expression.
pub fn fuse_dup_drop(expr: CoreExpr) -> CoreExpr {
    // Run multiple passes until no more fusions occur
    let mut result = expr;
    loop {
        let fused = fuse(result.clone());
        if exprs_equal(&fused, &result) {
            return fused;
        }
        result = fused;
    }
}

/// Represents a collected Dup or Drop on a spine.
#[derive(Clone)]
enum RcOp {
    Dup(CoreVarRef, Span),
    Drop(CoreVarRef, Span),
}

fn fuse(expr: CoreExpr) -> CoreExpr {
    match expr {
        // When we hit a Dup or Drop, collect the entire spine of RC ops
        // and fuse matching pairs before re-wrapping.
        CoreExpr::Dup { .. } | CoreExpr::Drop { .. } => fuse_spine(expr),

        // Recurse into all other forms
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(fuse(*rhs)),
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(fuse(*rhs)),
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(fuse(*body)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(fuse(*func)),
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(fuse(*func)),
            args: args.into_iter().map(fuse).collect(),
            arg_modes,
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(fuse(*scrutinee)),
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = fuse(alt.rhs);
                    alt.guard = alt.guard.map(fuse);
                    alt
                })
                .collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(fuse(*value)),
            span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(fuse).collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(fuse(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|mut h| {
                    h.body = fuse(h.body);
                    h
                })
                .collect(),
            span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields.into_iter().map(fuse).collect(),
            field_mask,
            span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee,
            unique_body: Box::new(fuse(*unique_body)),
            shared_body: Box::new(fuse(*shared_body)),
            span,
        },
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
    }
}

/// Collect all consecutive Dup/Drop operations on a spine, cancel matching
/// pairs, then re-wrap the remaining operations around the fused body.
///
/// This handles non-adjacent pairs like:
///   `Dup(a); Dup(b); Drop(a); Drop(c); body`
///   → `Dup(b); Drop(c); body`     (a cancelled)
fn fuse_spine(expr: CoreExpr) -> CoreExpr {
    let mut ops: Vec<RcOp> = Vec::new();
    let mut cursor = expr;

    // Collect all leading Dup/Drop nodes
    loop {
        match cursor {
            CoreExpr::Dup { var, body, span } => {
                ops.push(RcOp::Dup(var, span));
                cursor = *body;
            }
            CoreExpr::Drop { var, body, span } => {
                ops.push(RcOp::Drop(var, span));
                cursor = *body;
            }
            _ => break,
        }
    }

    // Cancel matching Dup/Drop pairs.
    // For each Drop, find a matching Dup and remove both.
    let mut cancelled = vec![false; ops.len()];
    for i in 0..ops.len() {
        if cancelled[i] {
            continue;
        }
        if let RcOp::Drop(ref drop_var, _) = ops[i] {
            // Find a matching Dup earlier in the spine
            for j in 0..i {
                if cancelled[j] {
                    continue;
                }
                if let RcOp::Dup(ref dup_var, _) = ops[j]
                    && same_var(dup_var, drop_var)
                {
                    cancelled[i] = true;
                    cancelled[j] = true;
                    break;
                }
            }
        }
    }

    // Also cancel Drop then Dup (reverse order)
    for i in 0..ops.len() {
        if cancelled[i] {
            continue;
        }
        if let RcOp::Dup(ref dup_var, _) = ops[i] {
            for j in 0..i {
                if cancelled[j] {
                    continue;
                }
                if let RcOp::Drop(ref drop_var, _) = ops[j]
                    && same_var(dup_var, drop_var)
                {
                    cancelled[i] = true;
                    cancelled[j] = true;
                    break;
                }
            }
        }
    }

    // Recurse into the remaining body
    let body = fuse(cursor);

    // Re-wrap surviving operations around the body (preserve original order)
    let mut result = body;
    for (idx, op) in ops.into_iter().enumerate().rev() {
        if cancelled[idx] {
            continue;
        }
        match op {
            RcOp::Dup(var, span) => {
                result = CoreExpr::Dup {
                    var,
                    body: Box::new(result),
                    span,
                };
            }
            RcOp::Drop(var, span) => {
                result = CoreExpr::Drop {
                    var,
                    body: Box::new(result),
                    span,
                };
            }
        }
    }

    result
}

fn same_var(a: &CoreVarRef, b: &CoreVarRef) -> bool {
    match (a.binder, b.binder) {
        (Some(a_id), Some(b_id)) => a_id == b_id,
        _ => false,
    }
}

/// Quick structural equality check (conservative — may return false for equal exprs).
fn exprs_equal(a: &CoreExpr, b: &CoreExpr) -> bool {
    // Use debug representation for comparison — not perfect but sufficient
    // for detecting when fusion made no changes.
    format!("{:?}", a) == format!("{:?}", b)
}
