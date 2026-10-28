use crate::{
    diagnostics::position::Span,
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyErrorKind {
    /// Structural mismatch: the two types are incompatible.
    Mismatch,
    /// Occurs check failed: a type variable appears inside the type it would be bound to.
    OccursCheck(TypeVarId),
    /// Rigid (skolem) type variable would be bound to a non-identical type.
    /// Proposal 0159: declared type parameters are rigid for the duration of
    /// checking; attempting to unify them with anything other than themselves
    /// violates the declared signature contract.
    RigidBind(TypeVarId),
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
    /// The two effect rows disagree. Kept apart from `detail`, which the
    /// function-parameter wrap overwrites, so diagnostics can still tell an
    /// effect error from a type error.
    pub effect_row: bool,
}

impl UnifyError {
    /// Build an effect-row mismatch error by wrapping both rows in synthetic
    /// `() -> Unit with row` Fun types (compatible with the generic diagnostic path).
    /// A function-parameter mismatch replaces them with the real parameter types.
    pub(crate) fn effect_row_mismatch(
        left: InferEffectRow,
        right: InferEffectRow,
        span: Span,
    ) -> Self {
        UnifyError {
            effect_row: true,
            ..Self::mismatch(
                InferType::Fun(vec![], Box::new(InferType::Con(TypeConstructor::Unit)), left),
                InferType::Fun(vec![], Box::new(InferType::Con(TypeConstructor::Unit)), right),
                span,
                UnifyErrorDetail::None,
            )
        }
    }

    pub(crate) fn mismatch(
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
            effect_row: false,
        }
    }

    pub(crate) fn occurs(v: TypeVarId, ty: InferType, span: Span) -> Self {
        let expected = InferType::Var(v);
        UnifyError {
            expected,
            actual: ty,
            kind: UnifyErrorKind::OccursCheck(v),
            detail: UnifyErrorDetail::None,
            span,
            effect_row: false,
        }
    }

    /// Build a rigid-variable-escape error when a skolem would be bound to a
    /// non-identical type during unification.
    pub(crate) fn rigid_bind(v: TypeVarId, ty: InferType, span: Span) -> Self {
        UnifyError {
            expected: InferType::Var(v),
            actual: ty,
            kind: UnifyErrorKind::RigidBind(v),
            detail: UnifyErrorDetail::None,
            span,
            effect_row: false,
        }
    }
}
