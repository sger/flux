use crate::{
    diagnostics::position::Span,
    types::{
        TypeVarId, infer_type::InferType, type_constructor::TypeConstructor, type_subst::TypeSubst,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyErrorKind {
    /// Structural mismatch: the two types are incompatible.
    Mismatch,
    /// Occurs check failed: a type variable appears inside the type it would be bound to.
    OccursCheck(TypeVarId),
}

/// Error produced when two types cannot be unified.
#[derive(Debug, Clone)]
pub struct UnifyError {
    pub expected: InferType,
    pub actual: InferType,
    pub kind: UnifyErrorKind,
    /// Source span where the mismatch was detected (may be `Span::default()`).
    pub span: Span,
}

impl UnifyError {
    fn mismatch(expected: InferType, actual: InferType, span: Span) -> Self {
        UnifyError {
            expected,
            actual,
            kind: UnifyErrorKind::Mismatch,
            span,
        }
    }

    fn occurs(v: TypeVarId, ty: InferType, span: Span) -> Self {
        let expected = InferType::Var(v);
        UnifyError {
            expected,
            actual: ty,
            kind: UnifyErrorKind::OccursCheck(v),
            span,
        }
    }
}

/// Unify two types, returning the minimal substitution that makes them equal.
///
/// The substitution must be applied to both types to obtain the unified form.
/// `Any` is compatible with everything (gradual typing escape).
pub fn unify(t1: &InferType, t2: &InferType) -> Result<TypeSubst, UnifyError> {
    unify_with_span(t1, t2, Span::default())
}

/// Unify with an explicit source span for error reporting.
pub fn unify_with_span(
    t1: &InferType,
    t2: &InferType,
    span: Span,
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
            unify_many(args1, args2, span)
        }

        // Function types: same arity
        (InferType::Fun(params1, ret1, _), InferType::Fun(params2, ret2, _))
            if params1.len() == params2.len() =>
        {
            let subst = unify_many(params1, params2, span)?;
            let ret1_sub = ret1.apply_type_subst(&subst);
            let ret2_sub = ret2.apply_type_subst(&subst);
            let s2 = unify_with_span(&ret1_sub, &ret2_sub, span)?;
            Ok(subst.compose(&s2))
        }

        // Tuple types: same length
        (InferType::Tuple(elems1), InferType::Tuple(elems2)) if elems1.len() == elems2.len() => {
            unify_many(elems1, elems2, span)
        }

        // Everything else is a mismatch
        _ => Err(UnifyError::mismatch(t1.clone(), t2.clone(), span)),
    }
}

/// Unify two lists of types pairwise, composing the substitutions.
fn unify_many(ts1: &[InferType], ts2: &[InferType], span: Span) -> Result<TypeSubst, UnifyError> {
    debug_assert_eq!(ts1.len(), ts2.len());
    let mut subst = TypeSubst::empty();
    for (t1, t2) in ts1.iter().zip(ts2.iter()) {
        let t1_sub = t1.apply_type_subst(&subst);
        let t2_sub = t2.apply_type_subst(&subst);
        let s = unify_with_span(&t1_sub, &t2_sub, span)?;
        subst = subst.compose(&s);
    }
    Ok(subst)
}

/// Bind a type variable to a type, checking for infinite types.
fn bind_var(v: TypeVarId, ty: &InferType, span: Span) -> Result<TypeSubst, UnifyError> {
    // Trivial: v is already the same variable
    if let InferType::Var(w) = ty {
        if *w == v {
            return Ok(TypeSubst::empty());
        }
    }

    // Occurs check: v must not appear free in ty
    if ty.free_vars().contains(&v) {
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
            infer_type::InferType,
            type_constructor::TypeConstructor,
            unify_error::{UnifyErrorKind, unify, unify_with_span},
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
        let left = InferType::Fun(vec![infer_var(0)], Box::new(infer_var(0)), vec![]);
        let right = InferType::Fun(vec![int()], Box::new(int()), vec![]);

        let subst = unify(&left, &right).expect("function unification should succeed");
        assert_eq!(subst.get(0), Some(&int()));
    }

    #[test]
    fn unify_occurs_check_reports_error_kind() {
        let left = infer_var(0);
        let right = InferType::App(TypeConstructor::List, vec![infer_var(0)]);

        let err = unify(&left, &right).expect_err("occurs check should fail");
        assert_eq!(err.kind, UnifyErrorKind::OccursCheck(0));
        assert_eq!(err.expected, infer_var(0));
        assert_eq!(err.actual, right);
    }

    #[test]
    fn unify_mismatch_reports_span_and_types() {
        let span = Span::new(Position::new(3, 5), Position::new(3, 12));
        let err = unify_with_span(&int(), &bool_t(), span).expect_err("should mismatch");

        assert_eq!(err.kind, UnifyErrorKind::Mismatch);
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
}
