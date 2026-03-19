//! Aether Phase 5: Dup/Drop insertion pass.
//!
//! After ANF normalization, all compound operands are let-bound. This pass walks
//! the let-chain structure and inserts explicit `Dup` (Rc::clone) and `Drop`
//! (early release) nodes based on variable use counts.
//!
//! Rules:
//! - 0 uses in body → insert `Drop(x)` at start of body
//! - 1 use in body  → ownership transfer (no dup/drop needed)
//! - N uses (N>1)   → insert `(N-1)` Dups wrapping body
//!
//! For Case expressions, unused pattern-bound variables get `Drop` at the start
//! of the alternative RHS. Variables used in some arms but not others get Dup
//! before the Case and Drop in the non-using arms.

use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::{pat_binders, use_counts};

/// Insert Dup/Drop annotations into a Core IR expression.
///
/// This is the main entry point for Aether Phase 5.
pub fn insert_dup_drop(expr: CoreExpr) -> CoreExpr {
    transform(expr)
}

fn transform(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = Box::new(transform(*rhs));
            let body = transform(*body);

            // Count uses of `var` in body
            let body_counts = use_counts(&body);
            let uses = body_counts.get(&var.id).copied().unwrap_or(0);

            let body = match uses {
                0 => {
                    // Variable unused in body — drop it immediately
                    wrap_drop(var, body, span)
                }
                1 => {
                    // Exactly one use — ownership transferred, no dup/drop
                    body
                }
                n => {
                    // N uses — insert (N-1) Dups wrapping the body
                    wrap_dups(var, body, span, n - 1)
                }
            };

            CoreExpr::Let {
                var,
                rhs,
                body: Box::new(body),
                span,
            }
        }

        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => {
            let rhs = Box::new(transform(*rhs));
            let body = transform(*body);

            // For LetRec, count uses in body only (self-reference in rhs is
            // handled by the closure mechanism, not by dup/drop).
            let body_counts = use_counts(&body);
            let uses = body_counts.get(&var.id).copied().unwrap_or(0);

            let body = match uses {
                0 => wrap_drop(var, body, span),
                1 => body,
                n => wrap_dups(var, body, span, n - 1),
            };

            CoreExpr::LetRec {
                var,
                rhs,
                body: Box::new(body),
                span,
            }
        }

        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee = Box::new(transform(*scrutinee));
            let alts = alts
                .into_iter()
                .map(|alt| {
                    let rhs = transform(alt.rhs);
                    // Drop unused pattern-bound variables at start of RHS
                    let rhs = drop_unused_pat_vars(&alt.pat, rhs, span);
                    CoreAlt {
                        pat: alt.pat,
                        guard: alt.guard.map(transform),
                        rhs,
                        span: alt.span,
                    }
                })
                .collect();

            CoreExpr::Case {
                scrutinee,
                alts,
                span,
            }
        }

        CoreExpr::Lam {
            params,
            body,
            span,
        } => {
            let body = Box::new(transform(*body));

            // Drop unused parameters at start of body
            let mut result_body = *body;
            for param in params.iter().rev() {
                let body_counts = use_counts(&result_body);
                let uses = body_counts.get(&param.id).copied().unwrap_or(0);
                if uses == 0 {
                    result_body = wrap_drop(*param, result_body, span);
                }
            }

            CoreExpr::Lam {
                params,
                body: Box::new(result_body),
                span,
            }
        }

        // Transparent recursion for other forms
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(transform(*func)),
            args: args.into_iter().map(transform).collect(),
            span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag,
            fields: fields.into_iter().map(transform).collect(),
            span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op,
            args: args.into_iter().map(transform).collect(),
            span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(transform(*value)),
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
            args: args.into_iter().map(transform).collect(),
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(transform(*body)),
            effect,
            handlers: handlers
                .into_iter()
                .map(|h| crate::core::CoreHandler {
                    operation: h.operation,
                    params: h.params,
                    resume: h.resume,
                    body: transform(h.body),
                    span: h.span,
                })
                .collect(),
            span,
        },

        // Atoms — nothing to insert
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,

        // Dup/Drop already present (idempotent)
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var,
            body: Box::new(transform(*body)),
            span,
        },
        CoreExpr::Drop { var, body, span } => CoreExpr::Drop {
            var,
            body: Box::new(transform(*body)),
            span,
        },
    }
}

/// Wrap `body` with a `Drop` node for the given binder.
fn wrap_drop(binder: CoreBinder, body: CoreExpr, span: Span) -> CoreExpr {
    CoreExpr::Drop {
        var: CoreVarRef::resolved(binder),
        body: Box::new(body),
        span,
    }
}

/// Wrap `body` with `n` `Dup` nodes for the given binder.
fn wrap_dups(binder: CoreBinder, body: CoreExpr, span: Span, n: usize) -> CoreExpr {
    let mut result = body;
    for _ in 0..n {
        result = CoreExpr::Dup {
            var: CoreVarRef::resolved(binder),
            body: Box::new(result),
            span,
        };
    }
    result
}

/// Drop pattern-bound variables that are unused in the RHS.
fn drop_unused_pat_vars(pat: &crate::core::CorePat, rhs: CoreExpr, span: Span) -> CoreExpr {
    let binders = pat_binders(pat);
    if binders.is_empty() {
        return rhs;
    }

    let rhs_counts = use_counts(&rhs);
    let mut result = rhs;

    for &binder_id in binders.iter().rev() {
        let uses = rhs_counts.get(&binder_id).copied().unwrap_or(0);
        if uses == 0 {
            // We need a CoreBinder to create the Drop, but we only have the ID.
            // Reconstruct from the pattern.
            if let Some(binder) = find_binder_in_pat(pat, binder_id) {
                result = CoreExpr::Drop {
                    var: CoreVarRef::resolved(binder),
                    body: Box::new(result),
                    span,
                };
            }
        }
    }

    result
}

/// Find a CoreBinder in a pattern by its ID.
fn find_binder_in_pat(
    pat: &crate::core::CorePat,
    target: CoreBinderId,
) -> Option<CoreBinder> {
    match pat {
        crate::core::CorePat::Var(binder) => {
            if binder.id == target {
                Some(*binder)
            } else {
                None
            }
        }
        crate::core::CorePat::Con { fields, .. } | crate::core::CorePat::Tuple(fields) => {
            for f in fields {
                if let Some(b) = find_binder_in_pat(f, target) {
                    return Some(b);
                }
            }
            None
        }
        crate::core::CorePat::Lit(_)
        | crate::core::CorePat::Wildcard
        | crate::core::CorePat::EmptyList => None,
    }
}
