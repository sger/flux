use std::collections::HashSet;

use crate::{
    diagnostics::position::Span,
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
    let mut fresh = 0;
    unify_with_span_and_fresh(t1, t2, Span::default(), &mut fresh)
}

/// Unify with an explicit source span for error reporting.
#[allow(clippy::result_large_err)]
pub fn unify_with_span(
    t1: &InferType,
    t2: &InferType,
    span: Span,
) -> Result<TypeSubst, UnifyError> {
    let mut fresh = 0;
    unify_with_span_and_fresh(t1, t2, span, &mut fresh)
}

#[allow(clippy::result_large_err)]
pub fn unify_with_span_and_fresh(
    t1: &InferType,
    t2: &InferType,
    span: Span,
    fresh: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    match (t1, t2) {
        // Any is compatible with everything (gradual typing)
        (InferType::Con(TypeConstructor::Any), _) | (_, InferType::Con(TypeConstructor::Any)) => {
            Ok(TypeSubst::empty())
        }

        // Identical types unify trivially
        (InferType::Con(c1), InferType::Con(c2)) if c1 == c2 => Ok(TypeSubst::empty()),

        // Type variable on the left
        (InferType::Var(v), t) => bind_var(*v, t, span),

        // Type variable on the right
        (t, InferType::Var(v)) => bind_var(*v, t, span),

        // Two type applications: same constructor, same arity
        (InferType::App(c1, args1), InferType::App(c2, args2))
            if c1 == c2 && args1.len() == args2.len() =>
        {
            unify_many(args1, args2, span, fresh)
        }

        // Function types: same arity and same effect set
        (InferType::Fun(params1, ret1, effects1), InferType::Fun(params2, ret2, effects2))
            if params1.len() == params2.len() =>
        {
            let mut subst = TypeSubst::empty();
            let row_subst = unify_effect_rows(effects1, effects2, span, fresh, &subst)?;
            subst = subst.compose(&row_subst);
            for (index, (p1, p2)) in params1.iter().zip(params2.iter()).enumerate() {
                let p1_sub = p1.apply_type_subst(&subst);
                let p2_sub = p2.apply_type_subst(&subst);
                let s = unify_with_span(&p1_sub, &p2_sub, span).map_err(|e| {
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
            let s2 = unify_with_span_and_fresh(&ret1_sub, &ret2_sub, span, fresh).map_err(|e| {
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
            t1.clone(),
            t2.clone(),
            span,
            UnifyErrorDetail::FunArityMismatch {
                expected: params1.len(),
                actual: params2.len(),
            },
        )),

        // Tuple types: same length
        (InferType::Tuple(elems1), InferType::Tuple(elems2)) if elems1.len() == elems2.len() => {
            unify_many(elems1, elems2, span, fresh)
        }

        // Everything else is a mismatch
        _ => Err(UnifyError::mismatch(
            t1.clone(),
            t2.clone(),
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
    span: Span,
    fresh: &mut u32,
) -> Result<TypeSubst, UnifyError> {
    debug_assert_eq!(ts1.len(), ts2.len());
    let mut subst = TypeSubst::empty();
    for (t1, t2) in ts1.iter().zip(ts2.iter()) {
        let t1_sub = t1.apply_type_subst(&subst);
        let t2_sub = t2.apply_type_subst(&subst);
        let s = unify_with_span_and_fresh(&t1_sub, &t2_sub, span, fresh)?;
        subst = subst.compose(&s);
    }
    Ok(subst)
}

#[allow(clippy::result_large_err)]
fn unify_effect_rows(
    left: &InferEffectRow,
    right: &InferEffectRow,
    span: Span,
    fresh: &mut u32,
    current_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let left_resolved = left.apply_row_subst(current_subst);
    let right_resolved = right.apply_row_subst(current_subst);
    let left_set: HashSet<_> = left_resolved.concrete().iter().copied().collect();
    let right_set: HashSet<_> = right_resolved.concrete().iter().copied().collect();

    match (left_resolved.tail(), right_resolved.tail()) {
        (None, None) => {
            if left_set == right_set {
                Ok(TypeSubst::empty())
            } else {
                Err(UnifyError::mismatch(
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        left_resolved,
                    ),
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        right_resolved,
                    ),
                    span,
                    UnifyErrorDetail::None,
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
                    current_subst,
                )
            } else {
                Err(UnifyError::mismatch(
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        left_resolved,
                    ),
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        right_resolved,
                    ),
                    span,
                    UnifyErrorDetail::None,
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
                    current_subst,
                )
            } else {
                Err(UnifyError::mismatch(
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        left_resolved,
                    ),
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        right_resolved,
                    ),
                    span,
                    UnifyErrorDetail::None,
                ))
            }
        }
        (Some(left_tail), Some(right_tail)) if left_tail == right_tail => {
            if left_set == right_set {
                Ok(TypeSubst::empty())
            } else {
                Err(UnifyError::mismatch(
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        left_resolved,
                    ),
                    InferType::Fun(
                        vec![],
                        Box::new(InferType::Con(TypeConstructor::Unit)),
                        right_resolved,
                    ),
                    span,
                    UnifyErrorDetail::None,
                ))
            }
        }
        (Some(left_tail), Some(right_tail)) => {
            let residual = *fresh;
            *fresh += 1;
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
            let s1 = unify_row_var(left_tail, left_bind, span, current_subst)?;
            let merged = current_subst.clone().compose(&s1);
            let s2 = unify_row_var(right_tail, right_bind, span, &merged)?;
            Ok(s1.compose(&s2))
        }
    }
}

#[allow(clippy::result_large_err)]
fn unify_row_var(
    row_var: TypeVarId,
    row: InferEffectRow,
    span: Span,
    current_subst: &TypeSubst,
) -> Result<TypeSubst, UnifyError> {
    let resolved = row.apply_row_subst(current_subst);

    if row_var_occurs_in_row(row_var, &resolved, current_subst) {
        return Err(UnifyError::mismatch(
            InferType::Var(row_var),
            InferType::Fun(
                vec![],
                Box::new(InferType::Con(TypeConstructor::Unit)),
                resolved,
            ),
            span,
            UnifyErrorDetail::None,
        ));
    }

    let mut subst = TypeSubst::empty();
    subst.insert_row(row_var, resolved);
    Ok(subst)
}

fn row_var_occurs_in_row(
    row_var: TypeVarId,
    row: &InferEffectRow,
    current_subst: &TypeSubst,
) -> bool {
    let resolved = row.apply_row_subst(current_subst);
    resolved.tail().is_some_and(|tail| tail == row_var)
}

/// Bind a type variable to a type, checking for infinite types.
#[allow(clippy::result_large_err)]
fn bind_var(v: TypeVarId, ty: &InferType, span: Span) -> Result<TypeSubst, UnifyError> {
    // Trivial: v is already the same variable
    if let InferType::Var(w) = ty
        && *w == v
    {
        return Ok(TypeSubst::empty());
    }

    // Occurs check: v must not appear free in ty
    if ty.free_type_vars().contains(&v) {
        return Err(UnifyError::occurs(v, ty.clone(), span));
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
