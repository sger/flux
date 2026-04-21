//! The static typing boundary contract (Proposal 0167, Part 1).
//!
//! Flux static typing is defined in terms of **boundaries** where a type must
//! be concrete. This module centralizes the classification so that every
//! compiler pass asking "is this a strict boundary?" answers the same way.
//!
//! # Why a shared classification
//!
//! Before this module, strict-typing checks each hardcoded their own notion
//! of where concreteness is required: the strict-mode annotated-let check,
//! the public-function return-type check, and the effect operation arity
//! check each computed this locally. That made the contract hard to reason
//! about and easy to regress.
//!
//! A `BoundaryKind` value is the single source of truth for that question.
//! Later phases of Proposal 0167 consume it from:
//!
//! - `static_type_validation` (unresolved-residue rule, Part 3)
//! - `compiler/statement` (typed-let validation, Part 4)
//! - the Core-adjacent contract pass (Part 7)
//!
//! Phase 1 of the proposal only introduces the enum, its documentation, and
//! the construction helpers. No existing pass is rewired yet.

use crate::diagnostics::position::Span;

/// A place in a program where a type is required to be fully concrete.
///
/// "Concrete" here means: after final substitution, the type contains no
/// free type variables other than those legitimately quantified by the
/// surrounding scheme. See [`BoundaryKind::requires_concrete`] for the
/// per-variant contract.
///
/// Variants are ordered from most-externally-observable (public signatures,
/// module interfaces) to most-internal (backend lowering). Diagnostic
/// ranking (Proposal 0167 Part 5) may use this ordering to prefer reporting
/// an outer-boundary failure over an inner one at the same span.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BoundaryKind {
    /// The signature of a function visible outside its defining module.
    ///
    /// Strict mode rejects any residual free variable that is not a
    /// legitimate `forall`-quantified parameter.
    PublicFunctionSignature,

    /// A `let` binding carrying an explicit type annotation.
    ///
    /// The declared annotation is the boundary; the initializer's inferred
    /// type must unify with it, and the resulting type must be concrete.
    AnnotatedLet,

    /// A function with an explicit return type annotation.
    ///
    /// The annotated return type must be concrete; the body's tail
    /// expression must have a type that unifies with it.
    AnnotatedReturn,

    /// An argument supplied to `perform`, or a value flowing into an
    /// effect handler arm's resumption.
    ///
    /// Effect payloads cross runtime boundaries (the handler state machine)
    /// and must have concrete representations.
    EffectBoundary,

    /// A binding re-exported through a module's public interface.
    ///
    /// Whatever scheme is visible to importers must be fully concrete;
    /// otherwise imports silently specialize and diverge across call sites.
    ModuleInterfaceBoundary,

    /// A value whose type must be concrete for native / backend lowering
    /// (representation selection, Aether reuse analysis, LLVM emission).
    ///
    /// Unlike the other variants, this one is usually reached *after*
    /// Core lowering, not before.
    BackendConcreteBoundary,
}

/// Metadata attached to a reported boundary violation.
///
/// Produced by the boundary-aware checks in later phases; held here so
/// diagnostic ranking can compare failures by kind as well as by span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundaryViolation {
    pub kind: BoundaryKind,
    pub span: Span,
}

impl BoundaryKind {
    /// A short, human-readable label for diagnostics and debug output.
    ///
    /// Not user-facing syntax — these strings surface in error messages
    /// that already frame the rule (e.g. "in the public signature of `foo`").
    pub fn label(self) -> &'static str {
        match self {
            Self::PublicFunctionSignature => "public function signature",
            Self::AnnotatedLet => "annotated let binding",
            Self::AnnotatedReturn => "annotated return type",
            Self::EffectBoundary => "effect operation",
            Self::ModuleInterfaceBoundary => "module interface",
            Self::BackendConcreteBoundary => "backend representation",
        }
    }

    /// Whether a boundary of this kind demands a concrete type (no
    /// illegal free variables) after final substitution.
    ///
    /// Every variant currently returns `true`. The method exists because
    /// Part 3 of the proposal may introduce a `Conditional` variant whose
    /// concreteness depends on runtime feature flags (e.g. effect
    /// boundaries only in strict mode). Leaving it as a method now avoids
    /// a second API break later.
    pub fn requires_concrete(self) -> bool {
        match self {
            Self::PublicFunctionSignature
            | Self::AnnotatedLet
            | Self::AnnotatedReturn
            | Self::EffectBoundary
            | Self::ModuleInterfaceBoundary
            | Self::BackendConcreteBoundary => true,
        }
    }
}

impl BoundaryViolation {
    /// Construct a boundary violation at the given source span. Used by
    /// passes that need to attach `BoundaryKind` metadata to a diagnostic
    /// without building the full `Diagnostic` yet.
    pub fn new(kind: BoundaryKind, span: Span) -> Self {
        Self { kind, span }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_boundary_kind_has_nonempty_label() {
        use BoundaryKind::*;
        for kind in [
            PublicFunctionSignature,
            AnnotatedLet,
            AnnotatedReturn,
            EffectBoundary,
            ModuleInterfaceBoundary,
            BackendConcreteBoundary,
        ] {
            assert!(!kind.label().is_empty(), "empty label for {kind:?}");
        }
    }

    #[test]
    fn every_boundary_kind_requires_concrete_today() {
        use BoundaryKind::*;
        for kind in [
            PublicFunctionSignature,
            AnnotatedLet,
            AnnotatedReturn,
            EffectBoundary,
            ModuleInterfaceBoundary,
            BackendConcreteBoundary,
        ] {
            assert!(kind.requires_concrete(), "{kind:?} must require concrete");
        }
    }

    #[test]
    fn boundary_kind_is_copy_and_hashable() {
        // Derives are load-bearing: downstream phases store BoundaryKind in
        // HashSets keyed by (kind, span) for diagnostic ranking. If someone
        // removes one of the derives, this test fails loud.
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(BoundaryKind::AnnotatedLet);
        set.insert(BoundaryKind::AnnotatedLet);
        assert_eq!(set.len(), 1);
    }
}
