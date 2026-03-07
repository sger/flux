use std::collections::HashSet;

use crate::{
    diagnostics::position::Span,
    syntax::Identifier,
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType,
        type_constructor::TypeConstructor, type_subst::TypeSubst,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyErrorKind {
    /// Structural mismatch: the two types are incompatible.
    Mismatch,
    /// Occurs check failed: a type variable appears inside the type it would be bound to.
    OccursCheck(TypeVarId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyErrorDetail {
    None,
    FunArityMismatch { expected: usize, actual: usize },
    FunParamMismatch { index: usize },
    FunReturnMismatch,
}

/// Error produced when two types cannot be unified.
#[derive(Debug, Clone)]
pub struct UnifyError {
    pub expected: InferType,
    pub actual: InferType,
    pub kind: UnifyErrorKind,
    pub detail: UnifyErrorDetail,
    /// Source span where the mismatch was detected (may be `Span::default()`).
    pub span: Span,
}

impl UnifyError {
    /// Build an effect-row mismatch error by wrapping both rows in synthetic
    /// `() -> Unit with row` Fun types (compatible with the generic diagnostic path).
    pub(crate) fn effect_row_mismatch(
        left: InferEffectRow,
        right: InferEffectRow,
        span: Span,
    ) -> Self {
        Self::mismatch(
            InferType::Fun(
                vec![],
                Box::new(InferType::Con(TypeConstructor::Unit)),
                left,
            ),
            InferType::Fun(
                vec![],
                Box::new(InferType::Con(TypeConstructor::Unit)),
                right,
            ),
            span,
            UnifyErrorDetail::None,
        )
    }

    fn mismatch(
        expected: InferType,
        actual: InferType,
        span: Span,
        detail: UnifyErrorDetail,
    ) -> Self {
        UnifyError {
            expected,
            actual,
            kind: UnifyErrorKind::Mismatch,
            detail,
            span,
        }
    }

    fn occurs(v: TypeVarId, ty: InferType, span: Span) -> Self {
        let expected = InferType::Var(v);
        UnifyError {
            expected,
            actual: ty,
            kind: UnifyErrorKind::OccursCheck(v),
            detail: UnifyErrorDetail::None,
            span,
        }
    }
}

/// Unify two types, returning the minimal substitution that makes them equal.
///
/// The substitution must be applied to both types to obtain the unified form.
/// `Any` is compatible with everything (gradual typing escape).
#[allow(clippy::result_large_err)]
pub fn unify(t1: &InferType, t2: &InferType) -> Result<TypeSubst, UnifyError> {
    let mut next_row_var_id = 0;
    unify_with_span_and_row_var_counter(
        t1,
        t2,
        &TypeSubst::empty(),
        Span::default(),
        &mut next_row_var_id,
    )
}

/// Unify with an explicit source span for error reporting.
#[allow(clippy::result_large_err)]
pub fn unify_with_span(
    t1: &InferType,
    t2: &InferType,
    span: Span,
) -> Result<TypeSubst, UnifyError> {
    let mut next_row_var_id = 0;
    unify_with_span_and_row_var_counter(t1, t2, &TypeSubst::empty(), span, &mut next_row_var_id)
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
            InferType::Var(v) => match subst.get_type(*v) {
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

/// Core unification with lazy variable resolution through a context substitution.
///
/// Instead of requiring callers to pre-resolve types via `apply_type_subst`,
/// this function resolves variables lazily through `ctx_subst` at each recursion
/// level using `resolve_head`. This eliminates 2 full type-tree walks per
/// unification call in `InferCtx::try_unify_and_compose_subst`.
///
/// The returned substitution contains only NEW bindings from this unification.
/// Callers compose it into their context substitution afterward.
#[allow(clippy::result_large_err)]
pub fn unify_with_span_and_row_var_counter(
    t1: &InferType,
    t2: &InferType,
    ctx_subst: &TypeSubst,
    span: Span,
    next_row_var_id: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    let t1_head = resolve_head(t1, ctx_subst);
    let t2_head = resolve_head(t2, ctx_subst);

    match (t1_head, t2_head) {
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
            unify_many(args1, args2, ctx_subst, span, next_row_var_id)
        }

        // Function types: same arity and same effect set
        (InferType::Fun(params1, ret1, effects1), InferType::Fun(params2, ret2, effects2))
            if params1.len() == params2.len() =>
        {
            let mut subst = TypeSubst::empty();
            let row_subst =
                unify_effect_rows(effects1, effects2, span, next_row_var_id, ctx_subst, &subst)?;
            subst = subst.compose(&row_subst);
            for (index, (p1, p2)) in params1.iter().zip(params2.iter()).enumerate() {
                let p1_sub = p1.apply_type_subst(&subst);
                let p2_sub = p2.apply_type_subst(&subst);
                let s = unify_with_span_and_row_var_counter(
                    &p1_sub,
                    &p2_sub,
                    ctx_subst,
                    span,
                    next_row_var_id,
                )
                .map_err(|e| {
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
            let s2 = unify_with_span_and_row_var_counter(
                &ret1_sub,
                &ret2_sub,
                ctx_subst,
                span,
                next_row_var_id,
            )
            .map_err(|e| {
                UnifyError::mismatch(
                    e.expected,
                    e.actual,
                    e.span,
                    UnifyErrorDetail::FunReturnMismatch,
                )
            })?;
            Ok(subst.compose(&s2))
        }

        // Function types with different arity.
        (InferType::Fun(params1, ..), InferType::Fun(params2, ..)) => Err(UnifyError::mismatch(
            t1_head.clone(),
            t2_head.clone(),
            span,
            UnifyErrorDetail::FunArityMismatch {
                expected: params1.len(),
                actual: params2.len(),
            },
        )),

        // Tuple types: same length
        (InferType::Tuple(elems1), InferType::Tuple(elems2)) if elems1.len() == elems2.len() => {
            unify_many(elems1, elems2, ctx_subst, span, next_row_var_id)
        }

        // Everything else is a mismatch
        _ => Err(UnifyError::mismatch(
            t1_head.clone(),
            t2_head.clone(),
            span,
            UnifyErrorDetail::None,
        )),
    }
}

/// Unify two lists of types pairwise, composing the substitutions.
#[allow(clippy::result_large_err)]
fn unify_many(
    ts1: &[InferType],
    ts2: &[InferType],
    ctx_subst: &TypeSubst,
    span: Span,
    next_row_var_id: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    debug_assert_eq!(ts1.len(), ts2.len());
    let mut local_subst = TypeSubst::empty();

    for (t1, t2) in ts1.iter().zip(ts2.iter()) {
        // Apply only the small local subst (accumulated from earlier pairs);
        // ctx_subst is handled lazily via resolve_head in the recursive call.
        let t1_sub = t1.apply_type_subst(&local_subst);
        let t2_sub = t2.apply_type_subst(&local_subst);
        let s = unify_with_span_and_row_var_counter(
            &t1_sub,
            &t2_sub,
            ctx_subst,
            span,
            next_row_var_id,
        )?;
        local_subst = local_subst.compose(&s);
    }
    Ok(local_subst)
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

#[allow(clippy::result_large_err)]
fn unify_effect_rows(
    left: &InferEffectRow,
    right: &InferEffectRow,
    span: Span,
    next_row_var_id: &mut u32,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let left_resolved = resolve_row(left, ctx_subst, local_subst);
    let right_resolved = resolve_row(right, ctx_subst, local_subst);
    let left_set: HashSet<_> = left_resolved.concrete().iter().copied().collect();
    let right_set: HashSet<_> = right_resolved.concrete().iter().copied().collect();

    match (left_resolved.tail(), right_resolved.tail()) {
        (None, None) => {
            if left_set == right_set {
                Ok(TypeSubst::empty())
            } else {
                Err(UnifyError::effect_row_mismatch(
                    left_resolved,
                    right_resolved,
                    span,
                ))
            }
        }
        (Some(left_tail), None) => {
            if left_set.is_subset(&right_set) {
                let diff = right_set
                    .difference(&left_set)
                    .copied()
                    .collect::<HashSet<_>>();
                unify_row_var(
                    left_tail,
                    InferEffectRow::closed_from_symbols(diff),
                    span,
                    ctx_subst,
                    local_subst,
                )
            } else {
                Err(UnifyError::effect_row_mismatch(
                    left_resolved,
                    right_resolved,
                    span,
                ))
            }
        }
        (None, Some(right_tail)) => {
            if right_set.is_subset(&left_set) {
                let diff = left_set
                    .difference(&right_set)
                    .copied()
                    .collect::<HashSet<_>>();
                unify_row_var(
                    right_tail,
                    InferEffectRow::closed_from_symbols(diff),
                    span,
                    ctx_subst,
                    local_subst,
                )
            } else {
                Err(UnifyError::effect_row_mismatch(
                    left_resolved,
                    right_resolved,
                    span,
                ))
            }
        }
        (Some(left_tail), Some(right_tail)) if left_tail == right_tail => {
            if left_set == right_set {
                Ok(TypeSubst::empty())
            } else {
                Err(UnifyError::effect_row_mismatch(
                    left_resolved,
                    right_resolved,
                    span,
                ))
            }
        }
        (Some(left_tail), Some(right_tail)) => {
            let residual = *next_row_var_id;
            *next_row_var_id += 1;
            let left_extra = left_set
                .difference(&right_set)
                .copied()
                .collect::<HashSet<_>>();
            let right_extra = right_set
                .difference(&left_set)
                .copied()
                .collect::<HashSet<_>>();

            let left_bind = InferEffectRow::open_from_symbols(right_extra, residual);
            let right_bind = InferEffectRow::open_from_symbols(left_extra, residual);
            let s1 = unify_row_var(left_tail, left_bind, span, ctx_subst, local_subst)?;
            // For the second binding, merge s1 into local so tail resolution sees it.
            let merged_local = local_subst.clone().compose(&s1);
            let s2 = unify_row_var(right_tail, right_bind, span, ctx_subst, &merged_local)?;
            Ok(s1.compose(&s2))
        }
    }
}

#[allow(clippy::result_large_err)]
fn unify_row_var(
    row_var: TypeVarId,
    row: InferEffectRow,
    span: Span,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let resolved = resolve_row(&row, ctx_subst, local_subst);

    if row_var_occurs_in_row(row_var, &resolved, ctx_subst, local_subst) {
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

fn row_var_occurs_in_row(
    row_var: TypeVarId,
    row: &InferEffectRow,
    ctx_subst: &TypeSubst,
    local_subst: &TypeSubst,
) -> bool {
    let resolved = resolve_row(row, ctx_subst, local_subst);
    resolved.tail().is_some_and(|tail| tail == row_var)
}

/// Bind a type variable to a type, checking for infinite types.
#[allow(clippy::result_large_err)]
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
            unify_error::{UnifyErrorDetail, UnifyErrorKind, unify, unify_with_span},
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
}
