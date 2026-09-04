use serde::{Deserialize, Serialize};

use crate::{
    ast::type_infer::ReportContext,
    diagnostics::position::Span,
    syntax::Identifier,
    types::{class_id::ClassId, infer_effect_row::InferEffectRow, infer_type::InferType},
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
        /// Canonical semantic identity of the class.
        class_id: ClassId,
        /// The type(s) that must have an instance. Single-param classes have
        /// one entry; multi-param classes (e.g., `Convert<a, b>`) have multiple.
        type_args: Vec<InferType>,
        /// Where in the source the constraint arose.
        span: Span,
    },
}

/// A collected class constraint exposed to downstream phases.
///
/// This is the public version of `Constraint::ClassConstraint`, suitable
/// for inclusion in `InferProgramResult`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WantedClassConstraintOrigin {
    ExplicitBound,
    InferredOperator,
    MethodCall,
    SchemeUse,
    TaskSpawnCapture { capture_name: Identifier },
}

#[derive(Debug, Clone)]
pub struct WantedClassConstraint {
    /// The class name (e.g., `Eq`, `Num`, `Show`).
    pub class_name: Identifier,
    /// Canonical semantic identity of the class. `class_name` is retained as
    /// the source spelling used in diagnostics only.
    pub class_id: ClassId,
    /// The type(s) that must have an instance. Single-param classes have
    /// one entry; multi-param classes (e.g., `Convert<a, b>`) have multiple.
    pub type_args: Vec<InferType>,
    /// Where in the source the constraint arose.
    pub span: Span,
    /// Why this constraint was emitted.
    pub origin: WantedClassConstraintOrigin,
    /// Whether the constraint was emitted from an already-concrete type.
    pub originated_from_concrete_type: bool,
}

/// A wanted set, shaped as a tree rather than a list (Proposal 0183, R1).
///
/// Mirrors GHC's `WantedConstraints` (`compiler/GHC/Tc/Types/Constraint.hs`),
/// whose `wc_simple` / `wc_impl` split is what lets a predicate be discharged
/// by a *binding's* context instead of only by an instance. A flat list has no
/// way to say "this obligation is satisfied by the `Eq<a>` its enclosing
/// signature promises", which is why a body needing more than its signature
/// grants is currently accepted and blamed on the caller instead.
///
/// During inference `simple` doubles as the emission log: a binding captures
/// the constraints it raised by remembering the length before inferring its
/// body and slicing from there, which is what GHC's `captureConstraints` does.
/// R2 turns each captured slice into an [`Implication`].
#[derive(Debug, Clone, Default)]
pub struct WantedConstraints {
    /// Predicates not yet attributed to any enclosing binding's context.
    pub simple: Vec<WantedClassConstraint>,
    /// Nested scopes, each carrying the context it may discharge against.
    pub implications: Vec<Implication>,
}

/// One binding's residual obligations, together with what it may assume.
///
/// GHC's `Implication` (`ic_skols`, `ic_given`, `ic_wanted`). `givens` are the
/// predicates the binding quantified into its scheme — every call site pays
/// for them by passing a dictionary — so inside `wanted` they may be treated
/// as already discharged.
#[derive(Debug, Clone)]
pub struct Implication {
    /// The context this scope may assume: the scheme's retained predicates.
    pub givens: Vec<SchemeConstraint>,
    /// Obligations raised inside the binding, still to be discharged.
    pub wanted: WantedConstraints,
    /// The binding this scope belongs to, for diagnostics.
    pub span: Span,
    /// The bound name, used to say which definition an error belongs to.
    pub binder: Identifier,
}

impl WantedConstraints {
    /// Whether every obligation in the tree has been discharged.
    ///
    /// GHC's `isSolvedWC`: an empty `simple` is not enough on its own, because
    /// an implication can hold obligations of its own.
    pub fn is_solved(&self) -> bool {
        self.simple.is_empty() && self.implications.iter().all(|i| i.wanted.is_solved())
    }

    /// Number of predicates emitted so far, for capture-window slicing.
    pub fn emitted(&self) -> usize {
        self.simple.len()
    }

    /// The predicates emitted since `start`, as GHC's `captureConstraints`
    /// returns the constraints raised while checking one binding's body.
    pub fn captured_since(&self, start: usize) -> Vec<WantedClassConstraint> {
        self.simple[start..].to_vec()
    }
}

/// A class constraint attached to a type scheme.
///
/// Records that a quantified type variable must have a class instance.
/// For example, `forall a. Eq<a> => a -> a -> Bool` has one `SchemeConstraint`
/// with `class_name = Eq` and a type argument containing `a`.
///
/// Used by dictionary elaboration (Proposal 0145, Step 5b) to determine
/// which dictionary parameters a polymorphic function requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemeConstraint {
    /// The class name (e.g., `Eq`, `Num`).
    pub class_name: Identifier,
    /// Canonical semantic identity of the constrained class.
    pub class_id: ClassId,
    /// The complete type arguments of the predicate. Structured arguments are
    /// preserved so downstream phases do not need to reconstruct them.
    pub type_args: Vec<InferType>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::interner::Interner;
    use crate::types::type_constructor::TypeConstructor;

    fn wanted(class_name: Identifier) -> WantedClassConstraint {
        WantedClassConstraint {
            class_name,
            class_id: ClassId::from_local_name(class_name),
            type_args: vec![InferType::Con(TypeConstructor::Int)],
            span: Span::default(),
            origin: WantedClassConstraintOrigin::ExplicitBound,
            originated_from_concrete_type: true,
        }
    }

    fn implication(binder: Identifier, wanted_set: WantedConstraints) -> Implication {
        Implication {
            givens: Vec::new(),
            wanted: wanted_set,
            span: Span::default(),
            binder,
        }
    }

    /// An empty `simple` is not enough: GHC's `isSolvedWC` also requires every
    /// implication to be solved, which is the whole reason for the tree.
    #[test]
    fn an_unsolved_implication_keeps_the_tree_unsolved() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let f = interner.intern("f");

        let inner = WantedConstraints {
            simple: vec![wanted(eq)],
            implications: Vec::new(),
        };
        let tree = WantedConstraints {
            simple: Vec::new(),
            implications: vec![implication(f, inner)],
        };

        assert!(tree.simple.is_empty());
        assert!(
            !tree.is_solved(),
            "an obligation inside an implication is still an obligation"
        );
        assert!(WantedConstraints::default().is_solved());
    }

    /// The capture window is how a binding claims the constraints its body
    /// raised — GHC's `captureConstraints`, spelled as a slice.
    #[test]
    fn capture_window_returns_only_what_was_emitted_after_it_opened() {
        let mut interner = Interner::new();
        let eq = interner.intern("Eq");
        let ord = interner.intern("Ord");

        let mut tree = WantedConstraints::default();
        tree.simple.push(wanted(eq));
        let start = tree.emitted();
        tree.simple.push(wanted(ord));

        let captured = tree.captured_since(start);
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].class_name, ord);
    }
}
