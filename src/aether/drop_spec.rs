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

use crate::shared::opt_bisect;
use std::collections::BTreeSet;

use crate::core::{CoreBinder, CoreBinderId, CoreVarRef};
use crate::diagnostics::position::Span;

use super::{
    AetherAlt as CoreAlt, AetherExpr, AetherHandler,
    analysis::{pat_field_binder_ids, use_counts_aether},
};

type CoreExpr = AetherExpr;

/// Run drop specialization on a Core IR expression.
pub fn specialize_drops(expr: CoreExpr) -> CoreExpr {
    transform(expr)
}

/// Run drop specialization on a backend-only Aether expression.
pub fn specialize_drops_aether(expr: CoreExpr) -> CoreExpr {
    specialize_drops(expr)
}

fn transform(expr: CoreExpr) -> CoreExpr {
    match expr {
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => {
            let scrutinee = Box::new(transform(*scrutinee));

            // Extract scrutinee binder for specialization.
            let scrut_binder = match scrutinee.as_ref() {
                CoreExpr::Var { var, .. } => var.binder.map(|id| CoreBinder::new(id, var.name)),
                _ => None,
            };

            let alts = alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = transform(alt.rhs);
                    alt.guard = alt.guard.map(transform);

                    // Try drop specialization on this alt's RHS
                    if let Some(binder) = &scrut_binder {
                        // Only the cell's own fields: a nested field is held
                        // by an inner cell, which a unique outer cell does
                        // not hand over (audit L1).
                        let field_binders: Vec<CoreBinderId> = pat_field_binder_ids(&alt.pat)
                            .into_iter()
                            .flatten()
                            .flatten()
                            .collect();
                        if !field_binders.is_empty() {
                            alt.rhs = try_specialize(*binder, &field_binders, alt.rhs, span);
                            let split = matches!(
                                &alt.rhs,
                                CoreExpr::DropSpecialized { scrutinee, .. }
                                    if scrutinee.binder == Some(binder.id)
                            );
                            if !split {
                                alt.rhs = split_below_root(alt.rhs, *binder, &field_binders, span);
                            }
                        }
                    }

                    alt
                })
                .collect();

            CoreExpr::Case {
                scrutinee,
                alts,
                join_ty,
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
        CoreExpr::LetRecGroup {
            bindings,
            body,
            span,
        } => CoreExpr::LetRecGroup {
            bindings: bindings
                .into_iter()
                .map(|(var, rhs)| (var, Box::new(transform(*rhs))))
                .collect(),
            body: Box::new(transform(*body)),
            span,
        },
        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body: Box::new(transform(*body)),
            span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(transform(*func)),
            args: args.into_iter().map(transform).collect(),
            span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            guarded_borrowed_args,
            owned_after,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(transform(*func)),
            args: args.into_iter().map(transform).collect(),
            arg_modes,
            guarded_borrowed_args,
            owned_after,
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
            owned_after,
            span,
        } => CoreExpr::Perform {
            effect,
            operation,
            args: args.into_iter().map(transform).collect(),
            owned_after,
            span,
        },
        CoreExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(transform(*body)),
            effect,
            parameter: parameter.map(|p| Box::new(transform(*p))),
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
        CoreExpr::MemberAccess {
            object,
            member,
            span,
        } => CoreExpr::MemberAccess {
            object: Box::new(transform(*object)),
            member,
            span,
        },
        CoreExpr::TupleField {
            object,
            index,
            span,
        } => CoreExpr::TupleField {
            object: Box::new(transform(*object)),
            index,
            span,
        },
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => expr,
    }
}

/// An arm that also gives its cell away cannot split at its root, but a path
/// that only releases the cell can: each run of copies and releases that
/// ends in a release of it, after a copy of one of its fields, is split
/// where it is. A run is taken whole, so the release pairs with every field
/// copy above it.
fn split_below_root(
    expr: CoreExpr,
    cell: CoreBinder,
    fields: &[CoreBinderId],
    span: Span,
) -> CoreExpr {
    let below = |expr: CoreExpr| split_below_root(expr, cell, fields, span);
    match expr {
        CoreExpr::Dup { .. } | CoreExpr::Drop { .. } => {
            let mut links = Vec::new();
            let mut rest = expr;
            loop {
                match rest {
                    CoreExpr::Dup { var, body, span } => {
                        links.push((true, var, span));
                        rest = *body;
                    }
                    CoreExpr::Drop { var, body, span } => {
                        links.push((false, var, span));
                        rest = *body;
                    }
                    other => {
                        rest = other;
                        break;
                    }
                }
            }
            let released_after_copy = links.iter().enumerate().any(|(at, (copy, var, _))| {
                !copy
                    && var.binder == Some(cell.id)
                    && links[..at].iter().any(|(copy, var, _)| {
                        *copy && var.binder.is_some_and(|id| fields.contains(&id))
                    })
            });
            let chain = links
                .into_iter()
                .rev()
                .fold(below(rest), |body, (copy, var, span)| {
                    let body = Box::new(body);
                    if copy {
                        CoreExpr::Dup { var, body, span }
                    } else {
                        CoreExpr::Drop { var, body, span }
                    }
                });
            if released_after_copy {
                try_specialize(cell, fields, chain, span)
            } else {
                chain
            }
        }
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var,
            rhs,
            body: Box::new(below(*body)),
            span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee,
            alts: alts
                .into_iter()
                .map(|mut alt| {
                    alt.rhs = below(alt.rhs);
                    alt
                })
                .collect(),
            join_ty,
            span,
        },
        // Anything else holds no run that ends this path, or is a function
        // of its own. A release left unsplit keeps its plain form.
        other => other,
    }
}

/// Try to apply drop specialization to a Case alt RHS.
/// Extracts a validated candidate skeleton and builds unique/shared branches.
fn try_specialize(
    scrutinee: CoreBinder,
    field_binders: &[CoreBinderId],
    rhs: CoreExpr,
    span: Span,
) -> CoreExpr {
    let Ok(candidate) = extract_candidate(scrutinee, field_binders, rhs.clone()) else {
        return rhs;
    };
    let unique_body = rebuild_candidate(&candidate, DropSpecMode::Unique);
    let shared_body = rebuild_candidate(&candidate, DropSpecMode::Shared);

    // A bisected run past its limit (0190 §13) keeps the plain drop. The
    // reuse later put on the unique path is part of this rewrite, not one of
    // its own: that path has no drop of the scrutinee left to fall back on.
    if format!("{unique_body:?}") == format!("{shared_body:?}")
        || !opt_bisect::allow("drop_spec", || {
            "split a drop into unique and shared paths".to_string()
        })
    {
        return rhs;
    }

    CoreExpr::DropSpecialized {
        scrutinee: CoreVarRef::resolved(&candidate.scrutinee),
        unique_body: Box::new(unique_body),
        shared_body: Box::new(shared_body),
        span,
    }
}

#[derive(Debug, Clone)]
struct DropSpecCandidate {
    scrutinee: CoreBinder,
    duped_fields: Vec<CoreBinderId>,
    expr: CoreExpr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropSpecFailureReason {
    NoScrutineeDrop,
    NoFieldDup,
    EffectfulBoundary,
    ScrutineeEscapes,
    UnsupportedBranchShape,
    AlreadySpecialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropScanState {
    BeforeDrop,
    AfterDrop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropSpecMode {
    Unique,
    Shared,
}

#[derive(Debug, Default)]
struct CandidateAccumulator {
    duped_fields: BTreeSet<CoreBinderId>,
}

fn extract_candidate(
    scrutinee: CoreBinder,
    field_binders: &[CoreBinderId],
    rhs: CoreExpr,
) -> Result<DropSpecCandidate, DropSpecFailureReason> {
    let mut acc = CandidateAccumulator::default();
    let saw_drop = validate_skeleton(
        &rhs,
        scrutinee.id,
        field_binders,
        DropScanState::BeforeDrop,
        &mut Vec::new(),
        &mut acc,
    )?;
    if !saw_drop {
        return Err(DropSpecFailureReason::NoScrutineeDrop);
    }
    if acc.duped_fields.is_empty() {
        return Err(DropSpecFailureReason::NoFieldDup);
    }
    Ok(DropSpecCandidate {
        scrutinee,
        duped_fields: acc.duped_fields.into_iter().collect(),
        expr: rhs,
    })
}

/// `run` holds the binders copied in the run of dups and drops just above
/// `expr`: the copies a release of the scrutinee at its end is paired with.
fn validate_skeleton(
    expr: &CoreExpr,
    scrutinee_id: CoreBinderId,
    field_binders: &[CoreBinderId],
    state: DropScanState,
    run: &mut Vec<CoreBinderId>,
    acc: &mut CandidateAccumulator,
) -> Result<bool, DropSpecFailureReason> {
    if state == DropScanState::AfterDrop {
        validate_after_drop_body(expr, scrutinee_id)?;
        return Ok(true);
    }

    match expr {
        CoreExpr::Let { rhs, body, .. } => {
            if !is_safe_wrapper_rhs(rhs) {
                return Err(DropSpecFailureReason::EffectfulBoundary);
            }
            if uses_binder(rhs, scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            validate_skeleton(
                body,
                scrutinee_id,
                field_binders,
                DropScanState::BeforeDrop,
                &mut Vec::new(),
                acc,
            )
        }
        CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => Err(DropSpecFailureReason::EffectfulBoundary),
        CoreExpr::Dup { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            run.extend(var.binder);
            validate_skeleton(
                body,
                scrutinee_id,
                field_binders,
                DropScanState::BeforeDrop,
                run,
                acc,
            )
        }
        CoreExpr::Drop { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                if state == DropScanState::AfterDrop {
                    return Err(DropSpecFailureReason::AlreadySpecialized);
                }
                acc.duped_fields.extend(
                    run.iter()
                        .filter(|binder| field_binders.contains(binder))
                        .copied(),
                );
                validate_skeleton(
                    body,
                    scrutinee_id,
                    field_binders,
                    DropScanState::AfterDrop,
                    &mut Vec::new(),
                    acc,
                )
                .map(|_| true)
            } else {
                validate_skeleton(
                    body,
                    scrutinee_id,
                    field_binders,
                    DropScanState::BeforeDrop,
                    run,
                    acc,
                )
            }
        }
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            if uses_binder(scrutinee, scrutinee_id) {
                return Err(DropSpecFailureReason::UnsupportedBranchShape);
            }
            let mut any_branch_has_drop = false;
            for alt in alts {
                if alt
                    .guard
                    .as_ref()
                    .is_some_and(|guard| uses_binder(guard, scrutinee_id))
                {
                    return Err(DropSpecFailureReason::UnsupportedBranchShape);
                }
                let mut branch_acc = CandidateAccumulator::default();
                let branch_has_drop = validate_skeleton(
                    &alt.rhs,
                    scrutinee_id,
                    field_binders,
                    state,
                    &mut Vec::new(),
                    &mut branch_acc,
                )?;
                merge_candidate_acc(acc, branch_acc);
                any_branch_has_drop |= branch_has_drop;
            }
            Ok(any_branch_has_drop)
        }
        CoreExpr::DropSpecialized { .. } => Err(DropSpecFailureReason::AlreadySpecialized),
        other => {
            if uses_binder(other, scrutinee_id) {
                Err(DropSpecFailureReason::ScrutineeEscapes)
            } else {
                Ok(false)
            }
        }
    }
}

fn validate_after_drop_body(
    expr: &CoreExpr,
    scrutinee_id: CoreBinderId,
) -> Result<(), DropSpecFailureReason> {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {
            if uses_binder(expr, scrutinee_id) {
                Err(DropSpecFailureReason::ScrutineeEscapes)
            } else {
                Ok(())
            }
        }
        // A copy below the release belongs to a later use: the cell's
        // reference is handed over only to a copy made above it.
        CoreExpr::Dup { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            validate_after_drop_body(body, scrutinee_id)
        }
        CoreExpr::Drop { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::AlreadySpecialized);
            }
            validate_after_drop_body(body, scrutinee_id)
        }
        CoreExpr::Let { rhs, body, .. } => {
            if !is_safe_wrapper_rhs(rhs) {
                return Err(DropSpecFailureReason::EffectfulBoundary);
            }
            if uses_binder(rhs, scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            validate_after_drop_body(rhs, scrutinee_id)?;
            validate_after_drop_body(body, scrutinee_id)
        }
        CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. } => Err(DropSpecFailureReason::EffectfulBoundary),
        CoreExpr::Case {
            scrutinee, alts, ..
        } => {
            if uses_binder(scrutinee, scrutinee_id) {
                return Err(DropSpecFailureReason::UnsupportedBranchShape);
            }
            for alt in alts {
                if alt
                    .guard
                    .as_ref()
                    .is_some_and(|guard| uses_binder(guard, scrutinee_id))
                {
                    return Err(DropSpecFailureReason::UnsupportedBranchShape);
                }
                validate_after_drop_body(&alt.rhs, scrutinee_id)?;
            }
            Ok(())
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            validate_after_drop_body(func, scrutinee_id)?;
            for arg in args {
                validate_after_drop_body(arg, scrutinee_id)?;
            }
            Ok(())
        }
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                validate_after_drop_body(field, scrutinee_id)?;
            }
            Ok(())
        }
        CoreExpr::PrimOp { args, .. } => {
            for arg in args {
                validate_after_drop_body(arg, scrutinee_id)?;
            }
            Ok(())
        }
        CoreExpr::Return { value, .. } => validate_after_drop_body(value, scrutinee_id),
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            validate_after_drop_body(object, scrutinee_id)
        }
        CoreExpr::Lam { .. } | CoreExpr::DropSpecialized { .. } => {
            Err(DropSpecFailureReason::EffectfulBoundary)
        }
    }
}

fn rebuild_candidate(candidate: &DropSpecCandidate, mode: DropSpecMode) -> CoreExpr {
    rewrite_mode(
        &candidate.expr,
        mode,
        candidate.scrutinee.id,
        &candidate.duped_fields,
    )
}

fn rewrite_mode(
    expr: &CoreExpr,
    mode: DropSpecMode,
    scrutinee_id: CoreBinderId,
    duped_fields: &[CoreBinderId],
) -> CoreExpr {
    match expr {
        CoreExpr::Dup { var, body, span } => {
            let is_field_dup = var.binder.is_some_and(|id| duped_fields.contains(&id))
                && pairs_with_release(body, var.binder, scrutinee_id);
            let inner = rewrite_mode(body, mode, scrutinee_id, duped_fields);
            if is_field_dup && mode == DropSpecMode::Unique {
                inner
            } else {
                CoreExpr::Dup {
                    var: *var,
                    body: Box::new(inner),
                    span: *span,
                }
            }
        }
        CoreExpr::Drop { var, body, span } => {
            if var.binder == Some(scrutinee_id) && mode == DropSpecMode::Unique {
                rewrite_mode(body, mode, scrutinee_id, duped_fields)
            } else {
                CoreExpr::Drop {
                    var: *var,
                    body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
                    span: *span,
                }
            }
        }
        CoreExpr::Let {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::Let {
            var: *var,
            rhs: Box::new(rewrite_mode(rhs, mode, scrutinee_id, duped_fields)),
            body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
            span: *span,
        },
        CoreExpr::LetRec {
            var,
            rhs,
            body,
            span,
        } => CoreExpr::LetRec {
            var: *var,
            rhs: rhs.clone(),
            body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
            span: *span,
        },
        CoreExpr::Lam {
            params,
            param_types,
            result_ty,
            body,
            span,
        } => CoreExpr::Lam {
            params: params.clone(),
            param_types: param_types.clone(),
            result_ty: result_ty.clone(),
            body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
            span: *span,
        },
        CoreExpr::App { func, args, span } => CoreExpr::App {
            func: Box::new(rewrite_mode(func, mode, scrutinee_id, duped_fields)),
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            span: *span,
        },
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            guarded_borrowed_args,
            owned_after,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(rewrite_mode(func, mode, scrutinee_id, duped_fields)),
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            arg_modes: arg_modes.clone(),
            guarded_borrowed_args: guarded_borrowed_args.clone(),
            owned_after: owned_after.clone(),
            span: *span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
            join_ty,
            span,
        } => CoreExpr::Case {
            scrutinee: scrutinee.clone(),
            alts: alts
                .iter()
                .map(|alt| CoreAlt {
                    pat: alt.pat.clone(),
                    guard: alt.guard.clone(),
                    rhs: rewrite_mode(&alt.rhs, mode, scrutinee_id, duped_fields),
                    span: alt.span,
                })
                .collect(),
            join_ty: join_ty.clone(),
            span: *span,
        },
        CoreExpr::Con { tag, fields, span } => CoreExpr::Con {
            tag: tag.clone(),
            fields: fields
                .iter()
                .map(|field| rewrite_mode(field, mode, scrutinee_id, duped_fields))
                .collect(),
            span: *span,
        },
        CoreExpr::PrimOp { op, args, span } => CoreExpr::PrimOp {
            op: *op,
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            span: *span,
        },
        CoreExpr::Return { value, span } => CoreExpr::Return {
            value: Box::new(rewrite_mode(value, mode, scrutinee_id, duped_fields)),
            span: *span,
        },
        CoreExpr::Perform {
            effect,
            operation,
            args,
            owned_after,
            span,
        } => CoreExpr::Perform {
            effect: *effect,
            operation: *operation,
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            owned_after: owned_after.clone(),
            span: *span,
        },
        CoreExpr::Handle {
            body,
            effect,
            parameter,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
            effect: *effect,
            parameter: parameter
                .as_ref()
                .map(|p| Box::new(rewrite_mode(p, mode, scrutinee_id, duped_fields))),
            handlers: handlers
                .iter()
                .map(|handler| AetherHandler {
                    operation: handler.operation,
                    params: handler.params.clone(),
                    param_types: handler.param_types.clone(),
                    state: handler.state,
                    state_ty: handler.state_ty.clone(),
                    resume: handler.resume,
                    resume_ty: handler.resume_ty.clone(),
                    body: rewrite_mode(&handler.body, mode, scrutinee_id, duped_fields),
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
                .map(|field| rewrite_mode(field, mode, scrutinee_id, duped_fields))
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
            unique_body: Box::new(rewrite_mode(unique_body, mode, scrutinee_id, duped_fields)),
            shared_body: Box::new(rewrite_mode(shared_body, mode, scrutinee_id, duped_fields)),
            span: *span,
        },
        _ => expr.clone(),
    }
}

/// Whether a `Dup` of `field` with this `body` is the copy a release of the
/// scrutinee pairs with: the last copy of the field in the run of dups and
/// drops that ends in the release. A unique cell hands over one reference per
/// field, so only that copy is redundant there; any other is a later use's
/// own (audit L1).
fn pairs_with_release(
    body: &CoreExpr,
    field: Option<CoreBinderId>,
    scrutinee_id: CoreBinderId,
) -> bool {
    match body {
        CoreExpr::Dup { var, body, .. } => {
            var.binder != field && pairs_with_release(body, field, scrutinee_id)
        }
        CoreExpr::Drop { var, body, .. } => {
            var.binder == Some(scrutinee_id) || pairs_with_release(body, field, scrutinee_id)
        }
        _ => false,
    }
}

fn uses_binder(expr: &CoreExpr, binder_id: CoreBinderId) -> bool {
    use_counts_aether(expr).contains_key(&binder_id)
}

fn merge_candidate_acc(acc: &mut CandidateAccumulator, branch_acc: CandidateAccumulator) {
    acc.duped_fields.extend(branch_acc.duped_fields);
}

fn is_safe_wrapper_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::LetRec { .. }
        | CoreExpr::LetRecGroup { .. }
        | CoreExpr::DropSpecialized { .. }
        | CoreExpr::Lam { .. } => false,
        CoreExpr::Var { .. }
        | CoreExpr::Lit(_, _)
        | CoreExpr::Con { .. }
        | CoreExpr::Reuse { .. }
        | CoreExpr::PrimOp { .. }
        | CoreExpr::App { .. }
        | CoreExpr::AetherCall { .. }
        | CoreExpr::Let { .. }
        | CoreExpr::Case { .. }
        | CoreExpr::Return { .. }
        | CoreExpr::Dup { .. }
        | CoreExpr::Drop { .. }
        | CoreExpr::MemberAccess { .. }
        | CoreExpr::TupleField { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use crate::aether::{AetherAlt as CoreAlt, AetherExpr as CoreExpr};
    use crate::core::{CoreBinder, CoreBinderId, CoreLit, CorePat, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;
    use std::borrow::Borrow;

    use super::{DropSpecFailureReason, specialize_drops};

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var<B: Borrow<CoreBinder>>(binder: B) -> CoreExpr {
        CoreExpr::Var {
            var: crate::core::CoreVarRef::resolved(binder.borrow()),
            span: Span::default(),
        }
    }

    fn count_matching<F>(expr: &CoreExpr, predicate: &F) -> usize
    where
        F: Fn(&CoreExpr) -> bool,
    {
        let here = usize::from(predicate(expr));
        match expr {
            CoreExpr::Lam { body, .. } => here + count_matching(body, predicate),
            CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
                here + count_matching(func, predicate)
                    + args
                        .iter()
                        .map(|arg| count_matching(arg, predicate))
                        .sum::<usize>()
            }
            CoreExpr::Let { rhs, body, .. } | CoreExpr::LetRec { rhs, body, .. } => {
                here + count_matching(rhs, predicate) + count_matching(body, predicate)
            }
            CoreExpr::LetRecGroup { bindings, body, .. } => {
                here + bindings
                    .iter()
                    .map(|(_, rhs)| count_matching(rhs, predicate))
                    .sum::<usize>()
                    + count_matching(body, predicate)
            }
            CoreExpr::Case {
                scrutinee, alts, ..
            } => {
                here + count_matching(scrutinee, predicate)
                    + alts
                        .iter()
                        .map(|alt| {
                            count_matching(&alt.rhs, predicate)
                                + alt
                                    .guard
                                    .as_ref()
                                    .map(|guard| count_matching(guard, predicate))
                                    .unwrap_or(0)
                        })
                        .sum::<usize>()
            }
            CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
                here + fields
                    .iter()
                    .map(|field| count_matching(field, predicate))
                    .sum::<usize>()
            }
            CoreExpr::PrimOp { args, .. } | CoreExpr::Perform { args, .. } => {
                here + args
                    .iter()
                    .map(|arg| count_matching(arg, predicate))
                    .sum::<usize>()
            }
            CoreExpr::Return { value, .. } => here + count_matching(value, predicate),
            CoreExpr::Handle {
                body,
                parameter,
                handlers,
                ..
            } => {
                here + parameter
                    .as_ref()
                    .map_or(0, |parameter| count_matching(parameter, predicate))
                    + count_matching(body, predicate)
                    + handlers
                        .iter()
                        .map(|handler| count_matching(&handler.body, predicate))
                        .sum::<usize>()
            }
            CoreExpr::Dup { body, .. } | CoreExpr::Drop { body, .. } => {
                here + count_matching(body, predicate)
            }
            CoreExpr::DropSpecialized {
                unique_body,
                shared_body,
                ..
            } => {
                here + count_matching(unique_body, predicate)
                    + count_matching(shared_body, predicate)
            }
            CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
                here + count_matching(object, predicate)
            }
            CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => here,
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
                        var: crate::core::CoreVarRef::resolved(&h),
                        body: Box::new(CoreExpr::Drop {
                            var: crate::core::CoreVarRef::resolved(&xs),
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
            join_ty: None,
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

    /// A copy below the release is a later use's own copy: the cell's
    /// reference to the field is only handed over to a copy made above it.
    #[test]
    fn a_copy_after_the_release_is_not_paired() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Drop {
                    var: crate::core::CoreVarRef::resolved(&xs),
                    body: Box::new(CoreExpr::Con {
                        tag: CoreTag::Cons,
                        fields: vec![
                            CoreExpr::Dup {
                                var: crate::core::CoreVarRef::resolved(&h),
                                body: Box::new(var(h)),
                                span: Span::default(),
                            },
                            var(t),
                        ],
                        span: Span::default(),
                    }),
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        assert!(!matches!(
            specialized,
            CoreExpr::Case { ref alts, .. }
                if matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. })
        ));
    }

    #[test]
    fn specializes_branchy_case_body() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let keep = binder(&mut interner, 2, "keep");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");

        let rhs = CoreExpr::Dup {
            var: crate::core::CoreVarRef::resolved(&h),
            body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(&xs),
                body: Box::new(CoreExpr::Case {
                    scrutinee: Box::new(var(keep)),
                    alts: vec![
                        CoreAlt {
                            pat: CorePat::Lit(CoreLit::Bool(true)),
                            guard: None,
                            rhs: CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(t)],
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(t)],
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                    ],
                    join_ty: None,
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };
        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs,
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        assert!(matches!(
            specialized,
            CoreExpr::Case { ref alts, .. }
                if matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. })
        ));
    }

    #[test]
    fn specializes_when_only_one_branch_has_scrutinee_drop() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let copy = binder(&mut interner, 2, "copy");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");

        let rhs = CoreExpr::Case {
            scrutinee: Box::new(var(copy)),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: CoreExpr::Dup {
                        var: crate::core::CoreVarRef::resolved(&h),
                        body: Box::new(CoreExpr::Drop {
                            var: crate::core::CoreVarRef::resolved(&xs),
                            body: Box::new(CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(t)],
                                span: Span::default(),
                            }),
                            span: Span::default(),
                        }),
                        span: Span::default(),
                    },
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: CoreExpr::Con {
                        tag: CoreTag::Cons,
                        fields: vec![var(h), var(t)],
                        span: Span::default(),
                    },
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs,
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        match specialized {
            CoreExpr::Case { alts, .. } => match &alts[0].rhs {
                CoreExpr::DropSpecialized {
                    unique_body,
                    shared_body,
                    ..
                } => {
                    let unique_drops = count_matching(
                        unique_body,
                        &|expr| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(xs.id)),
                    );
                    let shared_drops = count_matching(
                        shared_body,
                        &|expr| matches!(expr, CoreExpr::Drop { var, .. } if var.binder == Some(xs.id)),
                    );
                    assert_eq!(unique_drops, 0);
                    assert_eq!(shared_drops, 1);
                }
                other => panic!("expected DropSpecialized, got {other:?}"),
            },
            other => panic!("expected case, got {other:?}"),
        }
    }

    #[test]
    fn specializes_through_nested_let_after_drop() {
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
                rhs: CoreExpr::Dup {
                    var: crate::core::CoreVarRef::resolved(&h),
                    body: Box::new(CoreExpr::Drop {
                        var: crate::core::CoreVarRef::resolved(&xs),
                        body: Box::new(CoreExpr::Let {
                            var: tmp,
                            rhs: Box::new(CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(t)],
                                span: Span::default(),
                            }),
                            body: Box::new(CoreExpr::Con {
                                tag: CoreTag::Cons,
                                fields: vec![var(h), var(tmp)],
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
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        assert!(matches!(
            specialized,
            CoreExpr::Case { ref alts, .. }
                if matches!(alts[0].rhs, CoreExpr::DropSpecialized { .. })
        ));
    }

    #[test]
    fn branch_local_dups_are_tracked_per_branch() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let choose = binder(&mut interner, 2, "choose");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Case {
                    scrutinee: Box::new(var(choose)),
                    alts: vec![
                        CoreAlt {
                            pat: CorePat::Lit(CoreLit::Bool(true)),
                            guard: None,
                            rhs: CoreExpr::Dup {
                                var: crate::core::CoreVarRef::resolved(&h),
                                body: Box::new(CoreExpr::Drop {
                                    var: crate::core::CoreVarRef::resolved(&xs),
                                    body: Box::new(CoreExpr::Con {
                                        tag: CoreTag::Cons,
                                        fields: vec![var(h), var(t)],
                                        span: Span::default(),
                                    }),
                                    span: Span::default(),
                                }),
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: CoreExpr::Dup {
                                var: crate::core::CoreVarRef::resolved(&t),
                                body: Box::new(CoreExpr::Drop {
                                    var: crate::core::CoreVarRef::resolved(&xs),
                                    body: Box::new(CoreExpr::Con {
                                        tag: CoreTag::Cons,
                                        fields: vec![var(h), var(t)],
                                        span: Span::default(),
                                    }),
                                    span: Span::default(),
                                }),
                                span: Span::default(),
                            },
                            span: Span::default(),
                        },
                    ],
                    join_ty: None,
                    span: Span::default(),
                },
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        match specialized {
            CoreExpr::Case { alts, .. } => match &alts[0].rhs {
                CoreExpr::DropSpecialized {
                    unique_body,
                    shared_body,
                    ..
                } => {
                    let unique_h_dups = count_matching(
                        unique_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(h.id)),
                    );
                    let unique_t_dups = count_matching(
                        unique_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(t.id)),
                    );
                    let shared_h_dups = count_matching(
                        shared_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(h.id)),
                    );
                    let shared_t_dups = count_matching(
                        shared_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(t.id)),
                    );
                    assert_eq!(unique_h_dups, 0);
                    assert_eq!(unique_t_dups, 0);
                    assert_eq!(shared_h_dups, 1);
                    assert_eq!(shared_t_dups, 1);
                }
                other => panic!("expected DropSpecialized, got {other:?}"),
            },
            other => panic!("expected case, got {other:?}"),
        }
    }

    #[test]
    fn rejects_effectful_boundary_inside_candidate() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let rhs = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(&xs),
            body: Box::new(CoreExpr::Perform {
                effect: crate::syntax::builtin_effects::io_effect_symbol(&mut interner),
                operation: interner.intern("print"),
                args: vec![var(h), var(t)],
                owned_after: Vec::new(),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let mut acc = super::CandidateAccumulator::default();
        let result = super::validate_skeleton(
            &rhs,
            xs.id,
            &[h.id, t.id],
            super::DropScanState::BeforeDrop,
            &mut Vec::new(),
            &mut acc,
        );
        assert_eq!(result, Err(DropSpecFailureReason::EffectfulBoundary));
    }

    #[test]
    fn rejects_scrutinee_escape_outside_drop_site() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let rhs = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(&xs),
            body: Box::new(CoreExpr::Con {
                tag: CoreTag::Cons,
                fields: vec![var(xs), var(t)],
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let mut acc = super::CandidateAccumulator::default();
        let result = super::validate_skeleton(
            &rhs,
            xs.id,
            &[h.id, t.id],
            super::DropScanState::BeforeDrop,
            &mut Vec::new(),
            &mut acc,
        );
        assert_eq!(result, Err(DropSpecFailureReason::ScrutineeEscapes));
    }

    #[test]
    fn rejects_when_only_non_field_dups_are_present() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let f = binder(&mut interner, 4, "f");

        let rhs = CoreExpr::Dup {
            var: crate::core::CoreVarRef::resolved(&f),
            body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(&xs),
                body: Box::new(CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![var(h), var(t)],
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let result = super::extract_candidate(xs, &[h.id, t.id], rhs.clone());
        assert_eq!(result.err(), Some(DropSpecFailureReason::NoFieldDup));
        let specialized = specialize_drops(rhs.clone());
        assert_eq!(format!("{specialized:?}"), format!("{rhs:?}"));
    }

    #[test]
    fn rejects_duplicate_scrutinee_drop_after_specialization_point() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let rhs = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(&xs),
            body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(&xs),
                body: Box::new(CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![
                        CoreExpr::Dup {
                            var: crate::core::CoreVarRef::resolved(&h),
                            body: Box::new(var(h)),
                            span: Span::default(),
                        },
                        var(t),
                    ],
                    span: Span::default(),
                }),
                span: Span::default(),
            }),
            span: Span::default(),
        };

        let mut acc = super::CandidateAccumulator::default();
        let result = super::validate_skeleton(
            &rhs,
            xs.id,
            &[h.id, t.id],
            super::DropScanState::BeforeDrop,
            &mut Vec::new(),
            &mut acc,
        );
        assert_eq!(result, Err(DropSpecFailureReason::AlreadySpecialized));
    }

    #[test]
    fn unique_branch_removes_only_field_dups_and_keeps_non_field_dups() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let f = binder(&mut interner, 4, "f");

        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: CorePat::Con {
                    tag: CoreTag::Cons,
                    fields: vec![CorePat::Var(h), CorePat::Var(t)],
                },
                guard: None,
                rhs: CoreExpr::Dup {
                    var: crate::core::CoreVarRef::resolved(&f),
                    body: Box::new(CoreExpr::Dup {
                        var: crate::core::CoreVarRef::resolved(&h),
                        body: Box::new(CoreExpr::Drop {
                            var: crate::core::CoreVarRef::resolved(&xs),
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
            join_ty: None,
            span: Span::default(),
        };

        let specialized = specialize_drops(expr);
        match specialized {
            CoreExpr::Case { alts, .. } => match &alts[0].rhs {
                CoreExpr::DropSpecialized {
                    unique_body,
                    shared_body,
                    ..
                } => {
                    let unique_field_dups = count_matching(
                        unique_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(h.id)),
                    );
                    let unique_non_field_dups = count_matching(
                        unique_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(f.id)),
                    );
                    let shared_field_dups = count_matching(
                        shared_body,
                        &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(h.id)),
                    );
                    assert_eq!(unique_field_dups, 0);
                    assert_eq!(unique_non_field_dups, 1);
                    assert_eq!(shared_field_dups, 1);
                }
                other => panic!("expected DropSpecialized, got {other:?}"),
            },
            other => panic!("expected case, got {other:?}"),
        }
    }

    fn dup_in<B: Borrow<CoreBinder>>(b: B, body: CoreExpr) -> CoreExpr {
        CoreExpr::Dup {
            var: crate::core::CoreVarRef::resolved(b.borrow()),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn drop_in<B: Borrow<CoreBinder>>(b: B, body: CoreExpr) -> CoreExpr {
        CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(b.borrow()),
            body: Box::new(body),
            span: Span::default(),
        }
    }

    fn cons_of(fields: Vec<CoreExpr>) -> CoreExpr {
        CoreExpr::Con {
            tag: CoreTag::Cons,
            fields,
            span: Span::default(),
        }
    }

    fn cons_pat(h: CorePat, t: CorePat) -> CorePat {
        CorePat::Con {
            tag: CoreTag::Cons,
            fields: vec![h, t],
        }
    }

    /// The unique and shared bodies drop specialization makes of
    /// `case xs of pat -> rhs`.
    fn specialized_paths(xs: CoreBinder, pat: CorePat, rhs: CoreExpr) -> (CoreExpr, CoreExpr) {
        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat,
                guard: None,
                rhs,
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };
        match specialize_drops(expr) {
            CoreExpr::Case { mut alts, .. } => match alts.remove(0).rhs {
                CoreExpr::DropSpecialized {
                    unique_body,
                    shared_body,
                    ..
                } => (*unique_body, *shared_body),
                other => panic!("expected DropSpecialized, got {other:?}"),
            },
            other => panic!("expected case, got {other:?}"),
        }
    }

    fn dups_of(expr: &CoreExpr, b: CoreBinder) -> usize {
        count_matching(
            expr,
            &|expr| matches!(expr, CoreExpr::Dup { var, .. } if var.binder == Some(b.id)),
        )
    }

    #[test]
    fn a_shared_path_keeps_its_scrutinee_release_after_the_field_copies() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let rhs = dup_in(h, dup_in(t, drop_in(xs, cons_of(vec![var(h), var(t)]))));
        let (_, shared) = specialized_paths(xs, cons_pat(CorePat::Var(h), CorePat::Var(t)), rhs);
        let CoreExpr::Dup {
            var: first, body, ..
        } = &shared
        else {
            panic!("expected first copy: {shared:?}");
        };
        assert_eq!(first.binder, Some(h.id));
        let CoreExpr::Dup {
            var: second, body, ..
        } = body.as_ref()
        else {
            panic!("expected second copy: {shared:?}");
        };
        assert_eq!(second.binder, Some(t.id));
        assert!(
            matches!(body.as_ref(), CoreExpr::Drop { var, .. } if var.binder == Some(xs.id)),
            "{shared:?}"
        );
    }

    /// Audit L1: `[h | t] -> [h | [h | t]]` stores `h` twice. A unique cell
    /// hands over one reference to it, so the second store keeps its copy.
    #[test]
    fn unique_path_keeps_a_second_copy_of_a_field() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let rhs = dup_in(
            h,
            dup_in(
                t,
                drop_in(
                    xs,
                    cons_of(vec![dup_in(h, var(h)), cons_of(vec![var(h), var(t)])]),
                ),
            ),
        );
        let (unique, shared) =
            specialized_paths(xs, cons_pat(CorePat::Var(h), CorePat::Var(t)), rhs);
        assert_eq!(
            (dups_of(&unique, h), dups_of(&unique, t)),
            (1, 0),
            "{unique:?}"
        );
        assert_eq!(
            (dups_of(&shared, h), dups_of(&shared, t)),
            (2, 1),
            "{shared:?}"
        );
    }

    /// Audit L1: a nested field is held by an inner cell, which the unique
    /// outer cell does not hand over, so its copy stays.
    #[test]
    fn unique_path_keeps_the_copies_of_nested_fields() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let h2 = binder(&mut interner, 3, "h2");
        let t2 = binder(&mut interner, 4, "t2");

        let pat = cons_pat(
            CorePat::Var(h),
            cons_pat(CorePat::Var(h2), CorePat::Var(t2)),
        );
        let rhs = dup_in(
            h,
            dup_in(
                h2,
                dup_in(
                    t2,
                    drop_in(xs, cons_of(vec![var(h), cons_of(vec![var(h2), var(t2)])])),
                ),
            ),
        );
        let (unique, _) = specialized_paths(xs, pat, rhs);
        assert_eq!(
            (
                dups_of(&unique, h),
                dups_of(&unique, h2),
                dups_of(&unique, t2)
            ),
            (0, 1, 1),
            "{unique:?}"
        );
    }

    /// Of two copies of a field above the release, only the last is the one
    /// the unique cell's reference stands in for.
    #[test]
    fn unique_path_removes_only_the_last_copy_in_the_run() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");

        let rhs = dup_in(
            h,
            dup_in(
                h,
                drop_in(xs, cons_of(vec![var(h), cons_of(vec![var(h), var(t)])])),
            ),
        );
        let (unique, shared) =
            specialized_paths(xs, cons_pat(CorePat::Var(h), CorePat::Var(t)), rhs);
        assert_eq!(dups_of(&unique, h), 1, "{unique:?}");
        assert_eq!(dups_of(&shared, h), 2, "{shared:?}");
    }

    /// `Cons(h, t) -> if c { dup t; drop xs; Cons(h, t) } else { xs }`: the
    /// arm gives `xs` away on one path, so it cannot split at its root, but
    /// the path that only releases `xs` splits where the release is.
    #[test]
    fn a_release_below_the_arm_root_is_split_where_it_is() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let h = binder(&mut interner, 2, "h");
        let t = binder(&mut interner, 3, "t");
        let released = dup_in(t, drop_in(xs, cons_of(vec![var(h), var(t)])));
        let branch = CoreExpr::Case {
            scrutinee: Box::new(CoreExpr::Lit(CoreLit::Bool(true), Span::default())),
            alts: vec![
                CoreAlt {
                    pat: CorePat::Lit(CoreLit::Bool(true)),
                    guard: None,
                    rhs: released,
                    span: Span::default(),
                },
                CoreAlt {
                    pat: CorePat::Wildcard,
                    guard: None,
                    rhs: var(xs),
                    span: Span::default(),
                },
            ],
            join_ty: None,
            span: Span::default(),
        };
        let expr = CoreExpr::Case {
            scrutinee: Box::new(var(xs)),
            alts: vec![CoreAlt {
                pat: cons_pat(CorePat::Var(h), CorePat::Var(t)),
                guard: None,
                rhs: branch,
                span: Span::default(),
            }],
            join_ty: None,
            span: Span::default(),
        };
        let mut splits = Vec::new();
        collect_splits(&specialize_drops(expr), xs, &mut splits);
        let [(unique, shared)] = splits.as_slice() else {
            panic!("expected one split of xs: {splits:?}");
        };
        assert_eq!(dups_of(unique, t), 0, "{unique:?}");
        assert_eq!(dups_of(shared, t), 1, "{shared:?}");
    }

    fn collect_splits(expr: &CoreExpr, xs: CoreBinder, splits: &mut Vec<(CoreExpr, CoreExpr)>) {
        match expr {
            CoreExpr::DropSpecialized {
                scrutinee,
                unique_body,
                shared_body,
                ..
            } if scrutinee.binder == Some(xs.id) => {
                splits.push((unique_body.as_ref().clone(), shared_body.as_ref().clone()));
            }
            CoreExpr::Case { alts, .. } => {
                for alt in alts {
                    collect_splits(&alt.rhs, xs, splits);
                }
            }
            _ => {}
        }
    }
}
