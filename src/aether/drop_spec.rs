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

use std::collections::BTreeSet;

use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreVarRef};
use crate::diagnostics::position::Span;

use super::analysis::{pat_binders, use_counts};

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
                CoreExpr::Var { var, .. } => var.binder.map(|id| CoreBinder::new(id, var.name)),
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
                            alt.rhs = try_specialize(binder, &field_binders, alt.rhs, span);
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
        CoreExpr::AetherCall {
            func,
            args,
            arg_modes,
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(transform(*func)),
            args: args.into_iter().map(transform).collect(),
            arg_modes,
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

    if format!("{unique_body:?}") == format!("{shared_body:?}") {
        return rhs;
    }

    CoreExpr::DropSpecialized {
        scrutinee: CoreVarRef::resolved(candidate.scrutinee),
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

fn validate_skeleton(
    expr: &CoreExpr,
    scrutinee_id: CoreBinderId,
    field_binders: &[CoreBinderId],
    state: DropScanState,
    acc: &mut CandidateAccumulator,
) -> Result<bool, DropSpecFailureReason> {
    if state == DropScanState::AfterDrop {
        validate_after_drop_body(expr, scrutinee_id, field_binders, acc)?;
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
                acc,
            )
        }
        CoreExpr::LetRec { .. } | CoreExpr::Perform { .. } | CoreExpr::Handle { .. } => {
            Err(DropSpecFailureReason::EffectfulBoundary)
        }
        CoreExpr::Dup { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            if let Some(binder_id) = var.binder
                && field_binders.contains(&binder_id)
            {
                acc.duped_fields.insert(binder_id);
            }
            validate_skeleton(
                body,
                scrutinee_id,
                field_binders,
                DropScanState::BeforeDrop,
                acc,
            )
        }
        CoreExpr::Drop { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                if state == DropScanState::AfterDrop {
                    return Err(DropSpecFailureReason::AlreadySpecialized);
                }
                validate_skeleton(
                    body,
                    scrutinee_id,
                    field_binders,
                    DropScanState::AfterDrop,
                    acc,
                )
                .map(|_| true)
            } else {
                validate_skeleton(
                    body,
                    scrutinee_id,
                    field_binders,
                    DropScanState::BeforeDrop,
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
    field_binders: &[CoreBinderId],
    acc: &mut CandidateAccumulator,
) -> Result<(), DropSpecFailureReason> {
    match expr {
        CoreExpr::Var { .. } | CoreExpr::Lit(_, _) => {
            if uses_binder(expr, scrutinee_id) {
                Err(DropSpecFailureReason::ScrutineeEscapes)
            } else {
                Ok(())
            }
        }
        CoreExpr::Dup { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            if let Some(binder_id) = var.binder
                && field_binders.contains(&binder_id)
            {
                acc.duped_fields.insert(binder_id);
            }
            validate_after_drop_body(body, scrutinee_id, field_binders, acc)
        }
        CoreExpr::Drop { var, body, .. } => {
            if var.binder == Some(scrutinee_id) {
                return Err(DropSpecFailureReason::AlreadySpecialized);
            }
            validate_after_drop_body(body, scrutinee_id, field_binders, acc)
        }
        CoreExpr::Let { rhs, body, .. } => {
            if !is_safe_wrapper_rhs(rhs) {
                return Err(DropSpecFailureReason::EffectfulBoundary);
            }
            if uses_binder(rhs, scrutinee_id) {
                return Err(DropSpecFailureReason::ScrutineeEscapes);
            }
            validate_after_drop_body(rhs, scrutinee_id, field_binders, acc)?;
            validate_after_drop_body(body, scrutinee_id, field_binders, acc)
        }
        CoreExpr::LetRec { .. } | CoreExpr::Perform { .. } | CoreExpr::Handle { .. } => {
            Err(DropSpecFailureReason::EffectfulBoundary)
        }
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
                let mut branch_acc = CandidateAccumulator::default();
                validate_after_drop_body(&alt.rhs, scrutinee_id, field_binders, &mut branch_acc)?;
                merge_candidate_acc(acc, branch_acc);
            }
            Ok(())
        }
        CoreExpr::App { func, args, .. } | CoreExpr::AetherCall { func, args, .. } => {
            validate_after_drop_body(func, scrutinee_id, field_binders, acc)?;
            for arg in args {
                validate_after_drop_body(arg, scrutinee_id, field_binders, acc)?;
            }
            Ok(())
        }
        CoreExpr::Con { fields, .. } | CoreExpr::Reuse { fields, .. } => {
            for field in fields {
                validate_after_drop_body(field, scrutinee_id, field_binders, acc)?;
            }
            Ok(())
        }
        CoreExpr::PrimOp { args, .. } => {
            for arg in args {
                validate_after_drop_body(arg, scrutinee_id, field_binders, acc)?;
            }
            Ok(())
        }
        CoreExpr::Return { value, .. } => {
            validate_after_drop_body(value, scrutinee_id, field_binders, acc)
        }
        CoreExpr::MemberAccess { object, .. } | CoreExpr::TupleField { object, .. } => {
            validate_after_drop_body(object, scrutinee_id, field_binders, acc)
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
            let is_field_dup = var.binder.is_some_and(|id| duped_fields.contains(&id));
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
            if var.binder == Some(scrutinee_id) {
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
        CoreExpr::Lam { params, body, span } => CoreExpr::Lam {
            params: params.clone(),
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
            span,
        } => CoreExpr::AetherCall {
            func: Box::new(rewrite_mode(func, mode, scrutinee_id, duped_fields)),
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            arg_modes: arg_modes.clone(),
            span: *span,
        },
        CoreExpr::Case {
            scrutinee,
            alts,
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
            span,
        } => CoreExpr::Perform {
            effect: *effect,
            operation: *operation,
            args: args
                .iter()
                .map(|arg| rewrite_mode(arg, mode, scrutinee_id, duped_fields))
                .collect(),
            span: *span,
        },
        CoreExpr::Handle {
            body,
            effect,
            handlers,
            span,
        } => CoreExpr::Handle {
            body: Box::new(rewrite_mode(body, mode, scrutinee_id, duped_fields)),
            effect: *effect,
            handlers: handlers
                .iter()
                .map(|handler| crate::core::CoreHandler {
                    operation: handler.operation,
                    params: handler.params.clone(),
                    resume: handler.resume,
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

fn uses_binder(expr: &CoreExpr, binder_id: CoreBinderId) -> bool {
    use_counts(expr).contains_key(&binder_id)
}

fn merge_candidate_acc(acc: &mut CandidateAccumulator, branch_acc: CandidateAccumulator) {
    acc.duped_fields.extend(branch_acc.duped_fields);
}

fn is_safe_wrapper_rhs(expr: &CoreExpr) -> bool {
    match expr {
        CoreExpr::Perform { .. }
        | CoreExpr::Handle { .. }
        | CoreExpr::LetRec { .. }
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
    use crate::core::{CoreAlt, CoreBinder, CoreBinderId, CoreExpr, CoreLit, CorePat, CoreTag};
    use crate::diagnostics::position::Span;
    use crate::syntax::interner::Interner;

    use super::{DropSpecFailureReason, specialize_drops};

    fn binder(interner: &mut Interner, raw: u32, name: &str) -> CoreBinder {
        CoreBinder::new(CoreBinderId(raw), interner.intern(name))
    }

    fn var(binder: CoreBinder) -> CoreExpr {
        CoreExpr::Var {
            var: crate::core::CoreVarRef::resolved(binder),
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
            CoreExpr::Handle { body, handlers, .. } => {
                here + count_matching(body, predicate)
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

    #[test]
    fn specializes_when_field_dup_follows_scrutinee_drop() {
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
                    var: crate::core::CoreVarRef::resolved(xs),
                    body: Box::new(CoreExpr::Con {
                        tag: CoreTag::Cons,
                        fields: vec![
                            CoreExpr::Dup {
                                var: crate::core::CoreVarRef::resolved(h),
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
    fn specializes_branchy_case_body() {
        let mut interner = Interner::new();
        let xs = binder(&mut interner, 1, "xs");
        let keep = binder(&mut interner, 2, "keep");
        let h = binder(&mut interner, 3, "h");
        let t = binder(&mut interner, 4, "t");

        let rhs = CoreExpr::Drop {
            var: crate::core::CoreVarRef::resolved(xs),
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
                            fields: vec![
                                CoreExpr::Dup {
                                    var: crate::core::CoreVarRef::resolved(h),
                                    body: Box::new(var(h)),
                                    span: Span::default(),
                                },
                                var(t),
                            ],
                            span: Span::default(),
                        },
                        span: Span::default(),
                    },
                ],
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
                    assert_eq!(shared_drops, 0);
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
                rhs: CoreExpr::Drop {
                    var: crate::core::CoreVarRef::resolved(xs),
                    body: Box::new(CoreExpr::Let {
                        var: tmp,
                        rhs: Box::new(CoreExpr::Con {
                            tag: CoreTag::Cons,
                            fields: vec![var(h), var(t)],
                            span: Span::default(),
                        }),
                        body: Box::new(CoreExpr::Con {
                            tag: CoreTag::Cons,
                            fields: vec![
                                CoreExpr::Dup {
                                    var: crate::core::CoreVarRef::resolved(h),
                                    body: Box::new(var(h)),
                                    span: Span::default(),
                                },
                                var(tmp),
                            ],
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
                            },
                            span: Span::default(),
                        },
                        CoreAlt {
                            pat: CorePat::Wildcard,
                            guard: None,
                            rhs: CoreExpr::Dup {
                                var: crate::core::CoreVarRef::resolved(t),
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
                            },
                            span: Span::default(),
                        },
                    ],
                    span: Span::default(),
                },
                span: Span::default(),
            }],
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
            var: crate::core::CoreVarRef::resolved(xs),
            body: Box::new(CoreExpr::Perform {
                effect: interner.intern("IO"),
                operation: interner.intern("print"),
                args: vec![var(h), var(t)],
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
            var: crate::core::CoreVarRef::resolved(xs),
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
            var: crate::core::CoreVarRef::resolved(f),
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
            var: crate::core::CoreVarRef::resolved(xs),
            body: Box::new(CoreExpr::Drop {
                var: crate::core::CoreVarRef::resolved(xs),
                body: Box::new(CoreExpr::Con {
                    tag: CoreTag::Cons,
                    fields: vec![
                        CoreExpr::Dup {
                            var: crate::core::CoreVarRef::resolved(h),
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
                    var: crate::core::CoreVarRef::resolved(f),
                    body: Box::new(CoreExpr::Drop {
                        var: crate::core::CoreVarRef::resolved(xs),
                        body: Box::new(CoreExpr::Con {
                            tag: CoreTag::Cons,
                            fields: vec![
                                CoreExpr::Dup {
                                    var: crate::core::CoreVarRef::resolved(h),
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
                },
                span: Span::default(),
            }],
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
}
