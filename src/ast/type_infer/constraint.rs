use crate::{
    ast::type_infer::ReportContext,
    diagnostics::position::Span,
    types::{infer_effect_row::InferEffectRow, infer_type::InferType},
};

/// A type constraint collected during HM inference.
///
/// The current engine solves constraints eagerly (at the point of emission).
/// This type exists to provide structural separation between constraint
/// generation and solving — future work (typeclasses, row-polymorphic records)
/// can switch to deferred solving by collecting constraints first and
/// processing them in a separate pass.
///
/// Constraint variants:
/// - [`Unify`](Constraint::Unify): two types must be equal
/// - [`EffectSubset`](Constraint::EffectSubset): callee effects must be
///   available in the ambient scope
#[derive(Debug, Clone)]
pub(super) enum Constraint {
    /// `t1 = t2` — two types must unify at the given span.
    Unify {
        t1: InferType,
        t2: InferType,
        span: Span,
        context: ReportContext,
    },
    /// Callee effect row must be a subset of the ambient effect row.
    EffectSubset {
        required: InferEffectRow,
        available: InferEffectRow,
        span: Span,
    },
}
