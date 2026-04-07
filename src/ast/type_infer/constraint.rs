use crate::{
    ast::type_infer::ReportContext,
    diagnostics::position::Span,
    syntax::Identifier,
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
/// - [`ClassConstraint`](Constraint::ClassConstraint): a type must have a
///   type class instance (e.g., `Num<a>` from `x + y`)
#[derive(Debug, Clone)]
pub enum Constraint {
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
    /// A type must implement a type class (e.g., `Num<a>` from `x + y`).
    ///
    /// Generated when:
    /// - An operator is used: `+` → `Num<a>`, `==` → `Eq<a>`
    /// - A class method is called: `eq(x, y)` → `Eq<typeof(x)>`
    ///
    /// Currently recorded for observability. Step 4 (constraint solving) will
    /// resolve these: concrete types are checked against known instances,
    /// unresolved variables become part of the function's type scheme.
    #[allow(dead_code)]
    Class {
        /// The class name (e.g., `Eq`, `Num`, `Show`).
        class_name: Identifier,
        /// The type that must have an instance.
        type_arg: InferType,
        /// Where in the source the constraint arose.
        span: Span,
    },
}

/// A collected class constraint exposed to downstream phases.
///
/// This is the public version of `Constraint::ClassConstraint`, suitable
/// for inclusion in `InferProgramResult`.
#[derive(Debug, Clone)]
pub struct WantedClassConstraint {
    /// The class name (e.g., `Eq`, `Num`, `Show`).
    pub class_name: Identifier,
    /// The type that must have an instance (may be concrete or a variable).
    pub type_arg: InferType,
    /// Where in the source the constraint arose.
    pub span: Span,
}
