#![allow(clippy::result_large_err, clippy::too_many_arguments)]

use std::collections::HashSet;

use crate::{
    diagnostics::position::Span,
    syntax::Identifier,
    types::{
        TypeVarId,
        infer_effect_row::InferEffectRow,
        infer_type::InferType,
        type_constructor::TypeConstructor,
        type_subst::TypeSubst,
        unify_error::{UnifyError, UnifyErrorDetail},
    },
};

/// Unify two types, returning the minimal substitution that makes them equal.
///
/// The substitution must be applied to both types to obtain the unified form.
/// `Any` is compatible with everything (gradual typing escape).
pub fn unify(t1: &InferType, t2: &InferType) -> Result<TypeSubst, UnifyError> {
    let mut fresh_row_var = 0;
    unify_core(
        t1,
        t2,
        &TypeSubst::empty(),
        Span::default(),
        &mut fresh_row_var,
    )
}

/// Unify with an explicit source span for error reporting.
pub fn unify_with_span(
    t1: &InferType,
    t2: &InferType,
    span: Span,
) -> Result<TypeSubst, UnifyError> {
    let mut fresh_row_var = 0;
    unify_core(t1, t2, &TypeSubst::empty(), span, &mut fresh_row_var)
}

/// Follow variable chains in a substitution to resolve the head constructor.
///
/// Only resolves top-level `Var` nodes — does NOT recurse into `Fun`/`App`/`Tuple`
/// children. This is O(chain_length), typically O(1), instead of the O(type_size)
/// full `apply_type_subst` walk.
fn resolve_head<'a>(ty: &'a InferType, subst: &'a TypeSubst) -> &'a InferType {
    const MAX_DEPTH: usize = 128;
    let mut current = ty;
    for _ in 0..MAX_DEPTH {
        match current {
            InferType::Var(v) => match subst.get(*v) {
                Some(bound) => current = bound,
                None => break,
            },
            _ => break,
        }
    }
    current
}

/// Check whether type variable `v` occurs anywhere in `ty`, resolving variables
/// through `ctx_subst` lazily at each level (without building a fully-resolved tree).
fn occurs_in_with_ctx(v: TypeVarId, ty: &InferType, ctx_subst: &TypeSubst) -> bool {
    match resolve_head(ty, ctx_subst) {
        InferType::Var(w) => *w == v,
        InferType::Con(_) => false,
        InferType::App(_, args) => args.iter().any(|a| occurs_in_with_ctx(v, a, ctx_subst)),
        InferType::Fun(params, ret, _) => {
            params.iter().any(|p| occurs_in_with_ctx(v, p, ctx_subst))
                || occurs_in_with_ctx(v, ret, ctx_subst)
        }
        InferType::Tuple(elems) => elems.iter().any(|e| occurs_in_with_ctx(v, e, ctx_subst)),
    }
}

/// Unify two types with full context: substitution, span, and row-variable state.
///
/// This is the core unification entry point. It:
/// 1. Resolves variables lazily through `ctx_subst` (via `resolve_head`)
///    instead of requiring callers to pre-apply substitutions
/// 2. Tracks a `fresh_row_var` counter for fresh row variables during
///    effect-row unification
/// 3. Returns only NEW bindings — callers compose them into their context afterward
pub fn unify_core(
    expected: &InferType,
    actual: &InferType,
    ctx_subst: &TypeSubst,
    span: Span,
    fresh_row_var: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    let expected_head = resolve_head(expected, ctx_subst);
    let actual_head = resolve_head(actual, ctx_subst);

    match (expected_head, actual_head) {
        // Any is compatible with everything (gradual typing)
        (InferType::Con(TypeConstructor::Any), _) | (_, InferType::Con(TypeConstructor::Any)) => {
            Ok(TypeSubst::empty())
        }

        // Identical types unify trivially
        (InferType::Con(c1), InferType::Con(c2)) if c1 == c2 => Ok(TypeSubst::empty()),

        // Type variable on the left
        (InferType::Var(v), t) => bind_var_with_ctx(*v, t, ctx_subst, span),

        // Type variable on the right
        (t, InferType::Var(v)) => bind_var_with_ctx(*v, t, ctx_subst, span),

        // Two type applications: same constructor, same arity
        (InferType::App(c1, args1), InferType::App(c2, args2))
            if c1 == c2 && args1.len() == args2.len() =>
        {
            unify_many(args1, args2, ctx_subst, span, fresh_row_var)
        }

        // Function types: same arity
        (InferType::Fun(params1, ret1, effects1), InferType::Fun(params2, ret2, effects2))
            if params1.len() == params2.len() =>
        {
            unify_fun_types(
                params1,
                ret1,
                effects1,
                params2,
                ret2,
                effects2,
                ctx_subst,
                span,
                fresh_row_var,
            )
        }

        // Function types with different arity.
        (InferType::Fun(params1, ..), InferType::Fun(params2, ..)) => Err(UnifyError::mismatch(
            expected_head.clone(),
            actual_head.clone(),
            span,
            UnifyErrorDetail::FunArityMismatch {
                expected: params1.len(),
                actual: params2.len(),
            },
        )),

        // Tuple types: same length
        (InferType::Tuple(elems1), InferType::Tuple(elems2)) if elems1.len() == elems2.len() => {
            unify_many(elems1, elems2, ctx_subst, span, fresh_row_var)
        }

        // Everything else is a mismatch
        _ => Err(UnifyError::mismatch(
            expected_head.clone(),
            actual_head.clone(),
            span,
            UnifyErrorDetail::None,
        )),
    }
}

/// Unify two lists of types pairwise, composing the substitutions.
fn unify_many(
    ts1: &[InferType],
    ts2: &[InferType],
    ctx_subst: &TypeSubst,
    span: Span,
    fresh_row_var: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    debug_assert_eq!(ts1.len(), ts2.len());
    let mut local_subst = TypeSubst::empty();

    for (t1, t2) in ts1.iter().zip(ts2.iter()) {
        // Apply only the small local subst (accumulated from earlier pairs);
        // ctx_subst is handled lazily via resolve_head in the recursive call.
        let t1_sub = t1.apply_type_subst(&local_subst);
        let t2_sub = t2.apply_type_subst(&local_subst);
        let s = unify_core(&t1_sub, &t2_sub, ctx_subst, span, fresh_row_var)?;
        local_subst = local_subst.compose(&s);
    }
    Ok(local_subst)
}

/// Unify two function types: effects, parameters (pairwise), and return type.
///
/// Assumes both parameter lists have the same length. Composes substitutions
/// incrementally so that later parameters see bindings from earlier ones.
fn unify_fun_types(
    params1: &[InferType],
    ret1: &InferType,
    effects1: &InferEffectRow,
    params2: &[InferType],
    ret2: &InferType,
    effects2: &InferEffectRow,
    ctx_subst: &TypeSubst,
    span: Span,
    fresh_row_var: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    let mut subst = TypeSubst::empty();
    let row_subst = unify_effect_rows(effects1, effects2, span, fresh_row_var, ctx_subst, &subst)?;
    subst = subst.compose(&row_subst);

    for (index, (p1, p2)) in params1.iter().zip(params2.iter()).enumerate() {
        let p1_sub = p1.apply_type_subst(&subst);
        let p2_sub = p2.apply_type_subst(&subst);
        let s = unify_core(&p1_sub, &p2_sub, ctx_subst, span, fresh_row_var).map_err(|e| {
            UnifyError::mismatch(
                e.expected,
                e.actual,
                e.span,
                UnifyErrorDetail::FunParamMismatch { index },
            )
        })?;
        subst = subst.compose(&s);
    }

    let ret1_sub = ret1.apply_type_subst(&subst);
    let ret2_sub = ret2.apply_type_subst(&subst);
    let s2 = unify_core(&ret1_sub, &ret2_sub, ctx_subst, span, fresh_row_var).map_err(|e| {
        UnifyError::mismatch(
            e.expected,
            e.actual,
            e.span,
            UnifyErrorDetail::FunReturnMismatch,
        )
    })?;
    Ok(subst.compose(&s2))
}

/// Resolve an effect row through both local and context substitutions.
///
/// Applies local first (small, from current unification), then context
/// (large, from InferCtx). Each `apply_row_subst` just follows the tail
/// variable, so this is O(1) per call.
fn resolve_row(
    row: &InferEffectRow,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> InferEffectRow {
    row.apply_row_subst(local_subst).apply_row_subst(ctx_subst)
}

/// Unify two effect rows by dispatching on their tail structure.
///
/// Cases handled:
/// 1. Both closed — require identical effect sets
/// 2. One open, one closed — bind the tail to the difference set
/// 3. Both open, same tail — require identical effect sets
/// 4. Both open, different tails — introduce a fresh residual row variable
fn unify_effect_rows(
    left: &InferEffectRow,
    right: &InferEffectRow,
    span: Span,
    fresh_row_var: &mut u32,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let left_resolved = resolve_row(left, ctx_subst, local_subst);
    let right_resolved = resolve_row(right, ctx_subst, local_subst);
    let left_set: HashSet<_> = left_resolved.concrete().iter().copied().collect();
    let right_set: HashSet<_> = right_resolved.concrete().iter().copied().collect();

    match (left_resolved.tail(), right_resolved.tail()) {
        // Both closed with identical effect sets
        (None, None) if left_set == right_set => Ok(TypeSubst::empty()),

        // Both open with same tail and identical effect sets
        (Some(l), Some(r)) if l == r && left_set == right_set => Ok(TypeSubst::empty()),

        (Some(tail), None) => unify_open_with_closed(
            tail,
            &left_set,
            &right_set,
            &left_resolved,
            &right_resolved,
            span,
            ctx_subst,
            local_subst,
        ),

        (None, Some(tail)) => unify_open_with_closed(
            tail,
            &right_set,
            &left_set,
            &left_resolved,
            &right_resolved,
            span,
            ctx_subst,
            local_subst,
        ),

        (Some(left_tail), Some(right_tail)) => unify_both_open_rows(
            left_tail,
            right_tail,
            &left_set,
            &right_set,
            span,
            fresh_row_var,
            ctx_subst,
            local_subst,
        ),

        _ => Err(UnifyError::effect_row_mismatch(
            left_resolved,
            right_resolved,
            span,
        )),
    }
}

/// Unify an open row (with `open_tail`) against a closed row.
///
/// Succeeds when the open row's concrete effects are a subset of the closed row's.
/// Binds the tail variable to the difference (effects in closed but not in open).
fn unify_open_with_closed(
    open_tail: TypeVarId,
    open_set: &HashSet<Identifier>,
    closed_set: &HashSet<Identifier>,
    left_resolved: &InferEffectRow,
    right_resolved: &InferEffectRow,
    span: Span,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    if !open_set.is_subset(closed_set) {
        return Err(UnifyError::effect_row_mismatch(
            left_resolved.clone(),
            right_resolved.clone(),
            span,
        ));
    }
    let diff = closed_set
        .difference(open_set)
        .copied()
        .collect::<HashSet<_>>();
    unify_row_var(
        open_tail,
        InferEffectRow::closed_from_symbols(diff),
        span,
        ctx_subst,
        local_subst,
    )
}

/// Unify two open rows with different tail variables.
///
/// Introduces a fresh residual row variable so that each tail absorbs the
/// effects unique to the other side plus the shared residual.
fn unify_both_open_rows(
    left_tail: TypeVarId,
    right_tail: TypeVarId,
    left_set: &HashSet<Identifier>,
    right_set: &HashSet<Identifier>,
    span: Span,
    fresh_row_var: &mut u32,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let residual = *fresh_row_var;
    *fresh_row_var += 1;

    let left_extra = left_set
        .difference(right_set)
        .copied()
        .collect::<HashSet<_>>();
    let right_extra = right_set
        .difference(left_set)
        .copied()
        .collect::<HashSet<_>>();

    let left_bind = InferEffectRow::open_from_symbols(right_extra, residual);
    let right_bind = InferEffectRow::open_from_symbols(left_extra, residual);

    let s1 = unify_row_var(left_tail, left_bind, span, ctx_subst, local_subst)?;
    let merged_local = local_subst.clone().compose(&s1);
    let s2 = unify_row_var(right_tail, right_bind, span, ctx_subst, &merged_local)?;
    Ok(s1.compose(&s2))
}

/// Bind a row variable to a resolved row, checking for occurs violations.
fn unify_row_var(
    row_var: TypeVarId,
    row: InferEffectRow,
    span: Span,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let resolved = resolve_row(&row, ctx_subst, local_subst);

    if resolved.tail().is_some_and(|tail| tail == row_var) {
        return Err(UnifyError::effect_row_mismatch(
            InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), row_var),
            resolved,
            span,
        ));
    }

    let mut subst = TypeSubst::empty();
    subst.insert_row(row_var, resolved);
    Ok(subst)
}

/// Bind a type variable to a type, checking for infinite types.
fn bind_var_with_ctx(
    v: TypeVarId,
    ty: &InferType,
    ctx_subst: &TypeSubst,
    span: Span,
) -> Result<TypeSubst, UnifyError> {
    // Trivial: v is already the same variable
    if let InferType::Var(w) = ty
        && *w == v
    {
        return Ok(TypeSubst::empty());
    }

    // Occurs check: v must not appear free in ty (resolving through ctx_subst)
    if occurs_in_with_ctx(v, ty, ctx_subst) {
        // Resolve for the error message so the user sees meaningful types
        let ty_resolved = ty.apply_type_subst(ctx_subst);
        return Err(UnifyError::occurs(v, ty_resolved, span));
    }

    let mut subst = TypeSubst::empty();
    subst.insert(v, ty.clone());
    Ok(subst)
}

#[cfg(test)]
mod tests {
    use crate::{
        diagnostics::position::{Position, Span},
        types::{
            infer_effect_row::InferEffectRow,
            infer_type::InferType,
            type_constructor::TypeConstructor,
            unify::{unify, unify_with_span},
            unify_error::{UnifyErrorDetail, UnifyErrorKind},
        },
    };

    fn infer_var(id: u32) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    fn bool_t() -> InferType {
        InferType::Con(TypeConstructor::Bool)
    }

    #[test]
    fn unify_var_with_concrete_type_binds_variable() {
        let subst = unify(&infer_var(0), &int()).expect("should unify");
        assert_eq!(subst.get(0), Some(&int()));
    }

    #[test]
    fn unify_function_types_propagates_param_substitution_to_return() {
        let left = InferType::Fun(
            vec![infer_var(0)],
            Box::new(infer_var(0)),
            InferEffectRow::closed_empty(),
        );
        let right = InferType::Fun(vec![int()], Box::new(int()), InferEffectRow::closed_empty());

        let subst = unify(&left, &right).expect("function unification should succeed");
        assert_eq!(subst.get(0), Some(&int()));
    }

    #[test]
    fn unify_occurs_check_reports_error_kind() {
        let left = infer_var(0);
        let right = InferType::App(TypeConstructor::List, vec![infer_var(0)]);

        let err = unify(&left, &right).expect_err("occurs check should fail");
        assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
        assert_eq!(err.detail, UnifyErrorDetail::None);
        assert_eq!(err.expected, infer_var(0));
        assert_eq!(err.actual, right);
    }

    #[test]
    fn unify_mismatch_reports_span_and_types() {
        let span = Span::new(Position::new(3, 5), Position::new(3, 12));
        let err = unify_with_span(&int(), &bool_t(), span).expect_err("should mismatch");

        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
        assert_eq!(err.detail, UnifyErrorDetail::None);
        assert_eq!(err.expected, int());
        assert_eq!(err.actual, bool_t());
        assert_eq!(err.span, span);
    }

    #[test]
    fn unify_any_is_compatible_with_everything() {
        let any = InferType::Con(TypeConstructor::Any);
        let tuple = InferType::Tuple(vec![int(), bool_t()]);
        let subst = unify(&any, &tuple).expect("Any should unify with every type");
        assert!(subst.is_empty());
    }

    #[test]
    fn unify_fun_arity_mismatch_sets_detail() {
        let left = InferType::Fun(vec![int()], Box::new(int()), InferEffectRow::closed_empty());
        let right = InferType::Fun(
            vec![int(), int()],
            Box::new(int()),
            InferEffectRow::closed_empty(),
        );
        let err = unify(&left, &right).expect_err("should mismatch on arity");
        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
        assert_eq!(
            err.detail,
            UnifyErrorDetail::FunArityMismatch {
                expected: 1,
                actual: 2
            }
        );
    }

    #[test]
    fn unify_fun_param_mismatch_sets_detail() {
        let left = InferType::Fun(vec![int()], Box::new(int()), InferEffectRow::closed_empty());
        let right = InferType::Fun(
            vec![bool_t()],
            Box::new(int()),
            InferEffectRow::closed_empty(),
        );
        let err = unify(&left, &right).expect_err("should mismatch on parameter type");
        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
        assert_eq!(err.detail, UnifyErrorDetail::FunParamMismatch { index: 0 });
    }

    #[test]
    fn unify_fun_return_mismatch_sets_detail() {
        let left = InferType::Fun(vec![int()], Box::new(int()), InferEffectRow::closed_empty());
        let right = InferType::Fun(
            vec![int()],
            Box::new(bool_t()),
            InferEffectRow::closed_empty(),
        );
        let err = unify(&left, &right).expect_err("should mismatch on return type");
        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
        assert_eq!(err.detail, UnifyErrorDetail::FunReturnMismatch);
    }

    // --- Effect row unification tests ---

    use super::unify_effect_rows;
    use crate::syntax::symbol::Symbol;
    use crate::types::type_subst::TypeSubst;

    fn sym(i: u32) -> Symbol {
        Symbol::new(i)
    }

    fn effectful_fun(params: Vec<InferType>, ret: InferType, effects: InferEffectRow) -> InferType {
        InferType::Fun(params, Box::new(ret), effects)
    }

    #[test]
    fn effect_rows_both_closed_identical_unify() {
        let row = InferEffectRow::closed_from_symbols([sym(1), sym(2)]);
        let mut fresh = 0;
        let result = unify_effect_rows(
            &row,
            &row,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn effect_rows_both_closed_different_fail() {
        let left = InferEffectRow::closed_from_symbols([sym(1)]);
        let right = InferEffectRow::closed_from_symbols([sym(2)]);
        let mut fresh = 0;
        let result = unify_effect_rows(
            &left,
            &right,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn effect_rows_open_with_closed_binds_tail_to_diff() {
        let open = InferEffectRow::open_from_symbols([sym(1)], 10);
        let closed = InferEffectRow::closed_from_symbols([sym(1), sym(2)]);
        let mut fresh = 0;
        let subst = unify_effect_rows(
            &open,
            &closed,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        )
        .expect("open subset of closed should unify");
        let bound = subst.get_row(10).expect("tail 10 should be bound");
        assert!(bound.concrete().contains(&sym(2)));
        assert!(!bound.concrete().contains(&sym(1)));
        assert_eq!(bound.tail(), None);
    }

    #[test]
    fn effect_rows_open_not_subset_of_closed_fails() {
        let open = InferEffectRow::open_from_symbols([sym(3)], 10);
        let closed = InferEffectRow::closed_from_symbols([sym(1)]);
        let mut fresh = 0;
        let result = unify_effect_rows(
            &open,
            &closed,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn effect_rows_both_open_same_tail_identical_unify() {
        let row = InferEffectRow::open_from_symbols([sym(1)], 10);
        let mut fresh = 0;
        let result = unify_effect_rows(
            &row,
            &row,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn effect_rows_both_open_different_tails_introduces_residual() {
        let left = InferEffectRow::open_from_symbols([sym(1)], 10);
        let right = InferEffectRow::open_from_symbols([sym(2)], 11);
        let mut fresh = 100;
        let subst = unify_effect_rows(
            &left,
            &right,
            Span::default(),
            &mut fresh,
            &TypeSubst::empty(),
            &TypeSubst::empty(),
        )
        .expect("both open with different tails should unify");

        // fresh should have been incremented for the residual variable
        assert_eq!(fresh, 101);

        // tail 10 should be bound to {sym(2)} | residual(100)
        let bound_left = subst.get_row(10).expect("tail 10 should be bound");
        assert!(bound_left.concrete().contains(&sym(2)));
        assert_eq!(bound_left.tail(), Some(100));

        // tail 11 should be bound to {sym(1)} | residual(100)
        let bound_right = subst.get_row(11).expect("tail 11 should be bound");
        assert!(bound_right.concrete().contains(&sym(1)));
        assert_eq!(bound_right.tail(), Some(100));
    }

    #[test]
    fn unify_fun_types_with_different_closed_effects_fails() {
        let left = effectful_fun(
            vec![int()],
            int(),
            InferEffectRow::closed_from_symbols([sym(1)]),
        );
        let right = effectful_fun(
            vec![int()],
            int(),
            InferEffectRow::closed_from_symbols([sym(2)]),
        );
        let err = unify(&left, &right).expect_err("different effects should mismatch");
        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
    }

    #[test]
    fn unify_fun_types_with_open_effect_row_binds_tail() {
        let left = effectful_fun(
            vec![int()],
            int(),
            InferEffectRow::open_from_symbols([sym(1)], 10),
        );
        let right = effectful_fun(
            vec![int()],
            int(),
            InferEffectRow::closed_from_symbols([sym(1), sym(2)]),
        );
        let subst = unify(&left, &right).expect("open effect row should unify with superset");
        let bound = subst.get_row(10).expect("row var 10 should be bound");
        assert!(bound.concrete().contains(&sym(2)));
        assert_eq!(bound.tail(), None);
    }
}
