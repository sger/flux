//! Aether Phase 8: Drop specialization (Perceus Section 2.3).
//!
//! After dup/drop insertion, Case alt RHS bodies often contain:
//!   `Dup(field1); Dup(field2); Drop(scrutinee); body`
//!
//! Drop specialization inlines the `Drop(scrutinee)` for the known constructor,
//! splitting execution into two paths:
//!
//! - **Unique** (Rc::strong_count == 1): the extracted fields are already owned,
//!   so the dups are unnecessary. Just free the scrutinee's shell.
//!   After dup/drop fusion: zero RC operations on the fast path.
//!
//! - **Shared** (Rc::strong_count > 1): dup the fields (they're shared),
//!   decrement the scrutinee's refcount (don't free recursively).
//!
//! The result is `DropSpecialized { scrutinee, unique_body, shared_body }`.

use crate::core::{CoreBinder, CoreBinderId, CoreExpr, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::pat_binders;

/// Run drop specialization on a Core IR expression.
pub fn specialize_drops(expr: CoreExpr) -> CoreExpr {
    transform(expr)
}

fn transform(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => {
            let scrutinee = Box::new(transform(*scrutinee));

            // Extract scrutinee binder for specialization.
            let scrut_binder = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => {
                    var.binder.map(|id| CoreBinder { id, name: var.name })
                }
                _ => None,
            };

            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = transform(alt.rhs);
                    alt.guard = alt.guard.map(transform);

                    // Try drop specialization on this alt's RHS
                    if let Some(binder) = scrut_binder {
                        let field_binders = pat_binders(&alt.pat);
                        if !field_binders.is_empty() {
                            alt.rhs =
                                try_specialize(binder, &field_binders, alt.rhs, span);
                        }
                    }

                    alt
                })
                .collect();

            CoreExpr::Case {
                scrutinee,
                alts,
                span,
            }
        }

        // Recurse into all other expression forms
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs: Box::new(transform(*rhs)),
            body: Box::new(transform(*body)),
            span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var,
            rhs: Box::new(transform(*rhs)),
            body: Box::new(transform(*body)),
            span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params,
            body: Box::new(transform(*body)),
            span,
        },
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
                .map(|mut h| {
                    h.body = transform(h.body);
                    h
                })
                .collect(),
            span,
        },
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
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token,
            tag,
            fields: fields.into_iter().map(transform).collect(),
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
            unique_body: Box::new(transform(*unique_body)),
            shared_body: Box::new(transform(*shared_body)),
            span,
        },
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
    }
}

/// Try to apply drop specialization to a Case alt RHS.
///
/// Looks for the pattern: `Dup(field)*; Drop(scrutinee); rest`
/// and splits into unique/shared branches.
fn try_specialize(
    scrutinee: CoreBinder,
    field_binders: &[CoreBinderId],
    rhs: CoreExpr,
    span: Span,
) -> CoreExpr {
    // Collect dups and find the scrutinee drop in the RHS spine.
    let mut duped_fields = Vec::new();
    let mut cursor = &rhs;
    let mut depth = 0;

    // Walk through Dup nodes, collecting which field binders are duped
    loop {
        match cursor {
            CoreExpr::Dup { var, body, .. } => {
                if let Some(binder_id) = var.binder {
                    if field_binders.contains(&binder_id) {
                        duped_fields.push(binder_id);
                    }
                }
                cursor = body;
                depth += 1;
            }
            _ => break,
        }
    }

    // Now cursor should be at Drop(scrutinee) if the pattern matches
    let drop_matches = match cursor {
        CoreExpr::Drop { var, .. } => {
            var.binder == Some(scrutinee.id)
        }
        _ => false,
    };

    if !drop_matches || duped_fields.is_empty() {
        // Pattern doesn't match — no specialization possible
        return rhs;
    }

    // Build the UNIQUE branch:
    // Strip dups for fields that were duped, keep the rest.
    // Don't include the Drop(scrutinee) — the shell is freed implicitly
    // (Rc::try_unwrap succeeded, so the shell memory is reclaimed).
    let unique_body = strip_field_dups(&rhs, &duped_fields, scrutinee.id, depth);

    // Build the SHARED branch:
    // Keep all dups. Replace Drop(scrutinee) with just the body after drop
    // (the scrutinee refcount was already decremented by the DropSpecialized check).
    let shared_body = replace_scrutinee_drop(&rhs, scrutinee.id);

    CoreExpr::DropSpecialized {
        scrutinee: CoreVarRef::resolved(scrutinee),
        unique_body: Box::new(unique_body),
        shared_body: Box::new(shared_body),
        span,
    }
}

/// Strip Dup nodes for field binders and the Drop(scrutinee) from the RHS.
/// This is the unique path — fields are already owned, no dups needed.
fn strip_field_dups(
    expr: &CoreExpr,
    duped_fields: &[CoreBinderId],
    scrutinee_id: CoreBinderId,
    _depth: usize,
) -> CoreExpr {
    match expr {
        CoreExpr::Dup { var, body, span } => {
            let is_field_dup = var
                .binder
                .is_some_and(|id| duped_fields.contains(&id));
            let inner = strip_field_dups(body, duped_fields, scrutinee_id, _depth);
            if is_field_dup {
                // Strip this dup — field is already owned on unique path
                inner
            } else {
                // Keep non-field dups (e.g., dup(f) for a closure)
                CoreExpr::Dup {
                    var: *var,
                    body: Box::new(inner),
                    span: *span,
                }
            }
        }
        CoreExpr::Drop { var, body, span } => {
            if var.binder == Some(scrutinee_id) {
                // Strip the scrutinee drop — on unique path, shell is freed
                // by the DropSpecialized runtime check
                body.as_ref().clone()
            } else {
                CoreExpr::Drop {
                    var: *var,
                    body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id, _depth)),
                    span: *span,
                }
            }
        }
        _ => expr.clone(),
    }
}

/// Replace Drop(scrutinee) with just its body in the shared branch.
/// On the shared path, the DropSpecialized runtime already decremented
/// the refcount, so we skip the explicit drop.
fn replace_scrutinee_drop(expr: &CoreExpr, scrutinee_id: CoreBinderId) -> CoreExpr {
    match expr {
        CoreExpr::Dup { var, body, span } => CoreExpr::Dup {
            var: *var,
            body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
            span: *span,
        },
        CoreExpr::Drop { var, body, span } => {
            if var.binder == Some(scrutinee_id) {
                // Replace: on shared path, refcount was already decremented
                body.as_ref().clone()
            } else {
                CoreExpr::Drop {
                    var: *var,
                    body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
                    span: *span,
                }
            }
        }
        _ => expr.clone(),
    }
}
