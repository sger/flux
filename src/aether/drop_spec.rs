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
/// Looks for the pattern: a leading spine of `Dup(field)` and `Drop(scrutinee)`
/// operations in any order, followed by a body that rebuilds from the
/// destructured fields.
/// and splits into unique/shared branches.
fn try_specialize(
    scrutinee: CoreBinder,
    field_binders: &[CoreBinderId],
    rhs: CoreExpr,
    span: Span,
) -> CoreExpr {
    let mut duped_fields = Vec::new();
    collect_field_dups(&rhs, field_binders, &mut duped_fields);
    duped_fields.sort_unstable();
    duped_fields.dedup();
    let saw_scrutinee_drop = contains_scrutinee_drop(&rhs, scrutinee.id);

    if !saw_scrutinee_drop || duped_fields.is_empty() {
        // Pattern doesn't match — no specialization possible
        return rhs;
    }

    // Build the UNIQUE branch:
    // Strip dups for fields that were duped, keep the rest.
    // Don't include the Drop(scrutinee) — the shell is freed implicitly
    // (Rc::try_unwrap succeeded, so the shell memory is reclaimed).
    let unique_body = strip_field_dups(&rhs, &duped_fields, scrutinee.id);

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

fn collect_field_dups(
    expr: &CoreExpr,
    field_binders: &[CoreBinderId],
    duped_fields: &mut Vec<CoreBinderId>,
) {
    match expr {
        CoreExpr::Dup { var, body, .. } => {
            if let Some(binder_id) = var.binder
                && field_binders.contains(&binder_id)
            {
                duped_fields.push(binder_id);
            }
            collect_field_dups(body, field_binders, duped_fields);
        }
        CoreExpr::Drop { body, .. } => collect_field_dups(body, field_binders, duped_fields),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            collect_field_dups(rhs, field_binders, duped_fields);
            collect_field_dups(body, field_binders, duped_fields);
        }
        CoreExpr::Lam { body, .. } | CoreExpr::Return { value: body, .. } => {
            collect_field_dups(body, field_binders, duped_fields);
        }
        CoreExpr::App { func, args, .. } => {
            collect_field_dups(func, field_binders, duped_fields);
            for arg in args {
                collect_field_dups(arg, field_binders, duped_fields);
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            collect_field_dups(scrutinee, field_binders, duped_fields);
            for alt in alts {
                if let Some(guard) = &alt.guard {
                    collect_field_dups(guard, field_binders, duped_fields);
                }
                collect_field_dups(&alt.rhs, field_binders, duped_fields);
            }
        }
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                collect_field_dups(field, field_binders, duped_fields);
            }
        }
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
            for arg in args {
                collect_field_dups(arg, field_binders, duped_fields);
            }
        }
        CoreExpr::Handle { body, handlers, .. } => {
            collect_field_dups(body, field_binders, duped_fields);
            for handler in handlers {
                collect_field_dups(&handler.body, field_binders, duped_fields);
            }
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            collect_field_dups(unique_body, field_binders, duped_fields);
            collect_field_dups(shared_body, field_binders, duped_fields);
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {}
    }
}

fn contains_scrutinee_drop(expr: &CoreExpr, scrutinee_id: CoreBinderId) -> bool {
    match expr {
        CoreExpr::Drop { var, body, .. } => {
            var.binder == Some(scrutinee_id) || contains_scrutinee_drop(body, scrutinee_id)
        }
        CoreExpr::Dup { body, .. } | CoreExpr::Lam { body, .. } => {
            contains_scrutinee_drop(body, scrutinee_id)
        }
        CoreExpr::Return { value, .. } => contains_scrutinee_drop(value, scrutinee_id),
        CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
            contains_scrutinee_drop(rhs, scrutinee_id) || contains_scrutinee_drop(body, scrutinee_id)
        }
        CoreExpr::App { func, args, .. } => {
            contains_scrutinee_drop(func, scrutinee_id)
                || args
                    .iter()
                    .any(|arg| contains_scrutinee_drop(arg, scrutinee_id))
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            contains_scrutinee_drop(scrutinee, scrutinee_id)
                || alts.iter().any(|alt| {
                    alt.guard
                        .as_ref()
                        .is_some_and(|guard| contains_scrutinee_drop(guard, scrutinee_id))
                        || contains_scrutinee_drop(&alt.rhs, scrutinee_id)
                })
        }
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => fields
            .iter()
            .any(|field| contains_scrutinee_drop(field, scrutinee_id)),
        CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => args
            .iter()
            .any(|arg| contains_scrutinee_drop(arg, scrutinee_id)),
        CoreExpr::Handle { body, handlers, .. } => {
            contains_scrutinee_drop(body, scrutinee_id)
                || handlers
                    .iter()
                    .any(|handler| contains_scrutinee_drop(&handler.body, scrutinee_id))
        }
        CoreExpr::DropSpecialized {
            unique_body,
            shared_body,
            ..
        } => {
            contains_scrutinee_drop(unique_body, scrutinee_id)
                || contains_scrutinee_drop(shared_body, scrutinee_id)
        }
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => false,
    }
}

/// Strip Dup nodes for field binders and the Drop(scrutinee) from the RHS.
/// This is the unique path — fields are already owned, no dups needed.
fn strip_field_dups(expr: &CoreExpr, duped_fields: &[CoreBinderId], scrutinee_id: CoreBinderId) -> CoreExpr {
    match expr {
        CoreExpr::Dup { var, body, span } => {
            let is_field_dup = var
                .binder
                .is_some_and(|id| duped_fields.contains(&id));
            let inner = strip_field_dups(body, duped_fields, scrutinee_id);
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
                strip_field_dups(body, duped_fields, scrutinee_id)
            } else {
                CoreExpr::Drop {
                    var: *var,
                    body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id)),
                    span: *span,
                }
            }
        }
        CoreExpr::Let { var, rhs, body, span } => CoreExpr::Let {
            var: *var,
            rhs: Box::new(strip_field_dups(rhs, duped_fields, scrutinee_id)),
            body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id)),
            span: *span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var: *var,
            rhs: Box::new(strip_field_dups(rhs, duped_fields, scrutinee_id)),
            body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id)),
            span: *span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params: params.clone(),
            body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id)),
            span: *span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(strip_field_dups(func, duped_fields, scrutinee_id)),
            args: args
                .iter()
                .map(|arg| strip_field_dups(arg, duped_fields, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(strip_field_dups(scrutinee, duped_fields, scrutinee_id)),
            alts: alts
                .iter()
                .map(|alt| crate::core::CoreAlt {
                    pat: alt.pat.clone(),
                    guard: alt
                        .guard
                        .as_ref()
                        .map(|guard| strip_field_dups(guard, duped_fields, scrutinee_id)),
                    rhs: strip_field_dups(&alt.rhs, duped_fields, scrutinee_id),
                    span: alt.span,
                })
                .collect(),
            span: *span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag: tag.clone(),
            fields: fields
                .iter()
                .map(|field| strip_field_dups(field, duped_fields, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op: op.clone(),
            args: args
                .iter()
                .map(|arg| strip_field_dups(arg, duped_fields, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(strip_field_dups(value, duped_fields, scrutinee_id)),
            span: *span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect: *effect,
            operation: *operation,
            args: args
                .iter()
                .map(|arg| strip_field_dups(arg, duped_fields, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(strip_field_dups(body, duped_fields, scrutinee_id)),
            effect: *effect,
            handlers: handlers
                .iter()
                .map(|handler| crate::core::CoreHandler {
                    operation: handler.operation,
                    params: handler.params.clone(),
                    resume: handler.resume,
                    body: strip_field_dups(&handler.body, duped_fields, scrutinee_id),
                    span: handler.span,
                })
                .collect(),
            span: *span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token: *token,
            tag: tag.clone(),
            fields: fields
                .iter()
                .map(|field| strip_field_dups(field, duped_fields, scrutinee_id))
                .collect(),
            field_mask: *field_mask,
            span: *span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee: *scrutinee,
            unique_body: Box::new(strip_field_dups(unique_body, duped_fields, scrutinee_id)),
            shared_body: Box::new(strip_field_dups(shared_body, duped_fields, scrutinee_id)),
            span: *span,
        },
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
                replace_scrutinee_drop(body, scrutinee_id)
            } else {
                CoreExpr::Drop {
                    var: *var,
                    body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
                    span: *span,
                }
            }
        }
        CoreExpr::Let { var, rhs, body, span } => CoreExpr::Let {
            var: *var,
            rhs: Box::new(replace_scrutinee_drop(rhs, scrutinee_id)),
            body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
            span: *span,
        },
        CoreExpr::LetRec { var, rhs, body, span } => CoreExpr::LetRec {
            var: *var,
            rhs: Box::new(replace_scrutinee_drop(rhs, scrutinee_id)),
            body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
            span: *span,
        },
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params: params.clone(),
            body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
            span: *span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(replace_scrutinee_drop(func, scrutinee_id)),
            args: args
                .iter()
                .map(|arg| replace_scrutinee_drop(arg, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            span,
        } => CoreExpr::Case {
            scrutinee: Box::new(replace_scrutinee_drop(scrutinee, scrutinee_id)),
            alts: alts
                .iter()
                .map(|alt| crate::core::CoreAlt {
                    pat: alt.pat.clone(),
                    guard: alt
                        .guard
                        .as_ref()
                        .map(|guard| replace_scrutinee_drop(guard, scrutinee_id)),
                    rhs: replace_scrutinee_drop(&alt.rhs, scrutinee_id),
                    span: alt.span,
                })
                .collect(),
            span: *span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag: tag.clone(),
            fields: fields
                .iter()
                .map(|field| replace_scrutinee_drop(field, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op: op.clone(),
            args: args
                .iter()
                .map(|arg| replace_scrutinee_drop(arg, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(replace_scrutinee_drop(value, scrutinee_id)),
            span: *span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            span,
        } => CoreExpr::Perform {
            effect: *effect,
            operation: *operation,
            args: args
                .iter()
                .map(|arg| replace_scrutinee_drop(arg, scrutinee_id))
                .collect(),
            span: *span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(replace_scrutinee_drop(body, scrutinee_id)),
            effect: *effect,
            handlers: handlers
                .iter()
                .map(|handler| crate::core::CoreHandler {
                    operation: handler.operation,
                    params: handler.params.clone(),
                    resume: handler.resume,
                    body: replace_scrutinee_drop(&handler.body, scrutinee_id),
                    span: handler.span,
                })
                .collect(),
            span: *span,
        },
        CoreExpr::Reuse {
            token,
            tag,
            fields,
            field_mask,
            span,
        } => CoreExpr::Reuse {
            token: *token,
            tag: tag.clone(),
            fields: fields
                .iter()
                .map(|field| replace_scrutinee_drop(field, scrutinee_id))
                .collect(),
            field_mask: *field_mask,
            span: *span,
        },
        CoreExpr::DropSpecialized {
            scrutinee,
            unique_body,
            shared_body,
            span,
        } => CoreExpr::DropSpecialized {
            scrutinee: *scrutinee,
            unique_body: Box::new(replace_scrutinee_drop(unique_body, scrutinee_id)),
            shared_body: Box::new(replace_scrutinee_drop(shared_body, scrutinee_id)),
            span: *span,
        },
        _ => expr.clone(),
    }
}

#[cfg(test)]
mod tests {
    use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreLit, CorePat, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::specialize_drops;

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: crate::core::CoreVarRef::resolved(binder),
            span: Span::default(),
        }
    }

    #[test]
    fn specializes_through_pure_leading_let() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let tmp = binder(&mut interner, 4, "tmp");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Let {
                    var: tmp,
                    rhs: Box::new(CoreExpr::Lit(CoreLit::Int(0), Span::default())),
                    body: Box::new(CoreExpr::Dup {
                        var: crate::core::CoreVarRef::resolved(h),
                        body: Box::new(CoreExpr::Drop {
                            var: crate::core::CoreVarRef::resolved(xs),
                            body: Box::new(CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(t)],
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        let has_drop_spec = matches!(
            specialized,
            CoreExpr::Case { ref alts, .. }
                if matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. })
        );
        assert!(
            has_drop_spec,
            "expected DropSpecialized through a pure leading let"
        );
    }
}
