//! Aether Phase 9: Dup/Drop fusion (Perceus Section 2.3).
//!
//! After drop specialization creates unique/shared branches, adjacent
//! `Dup(x); Drop(x)` and `Drop(x); Dup(x)` pairs can be cancelled.
//! This is what eliminates RC operations on the unique (fast) path.
//!
//! Example transformation:
//! ```text
//! -- Before fusion (unique branch after drop spec):
//! Dup(x); Drop(x); free(xs); body     -- dup then drop cancel
//!
//! -- After fusion:
//! free(xs); body                       -- zero RC ops!
//! ```

use crate::core::{CoreExpr, CoreVarRef};

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

fn fuse(expr: CoreExpr) -> CoreExpr {
    match expr {
        // Pattern 1: Dup(x, Drop(x, body)) → body
        CoreExpr::Dup { var, body, span } => {
            let body = fuse(*body);
            if let CoreExpr::Drop {
                var: drop_var,
                body: inner,
                ..
            } = &body
            {
                if same_var(&var, drop_var) {
                    return *inner.clone();
                }
            }
            CoreExpr::Dup {
                var,
                body: Box::new(body),
                span,
            }
        }

        // Pattern 2: Drop(x, Dup(x, body)) → body
        CoreExpr::Drop { var, body, span } => {
            let body = fuse(*body);
            if let CoreExpr::Dup {
                var: dup_var,
                body: inner,
                ..
            } = &body
            {
                if same_var(&var, dup_var) {
                    return *inner.clone();
                }
            }
            CoreExpr::Drop {
                var,
                body: Box::new(body),
                span,
            }
        }

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
