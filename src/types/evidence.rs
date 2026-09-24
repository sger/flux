//! Where evidence goes once the solver has produced it.
//!
//! The solver already decides how every predicate is discharged and records it
//! as an [`Evidence`] — which instance matched, with what substitution, and
//! what its context needs. Nothing consumed that. Six independent places in
//! `core/` and `compiler/` re-derived the same answer from the class
//! environment instead, kept in agreement by hand-written comments, and
//! produced one bug five times.
//!
//! An [`EvidenceMap`] carries the solver's answer to them. It is keyed by
//! [`EvidenceSite`] — an expression id plus the position of the predicate at
//! that expression — because a span cannot tell two predicates apart when one
//! site raises two for the same class, which is the failure
//! `close_definition_scope` was already recording.
//!
//! Deliberately not serialisable. Evidence is keyed by `ExprId`, which is
//! local to the unit being compiled, so nothing here is cross-module state and
//! no cached artifact carries it.
//!
//! See `docs/proposals/0186_generics_foundations.md`.
//!
//! # How a site is keyed
//!
//! Measured with `FLUX_DBG_EVIDENCE` (0.0.8 plan, step 1b):
//!
//! - **A call to a constrained function is keyed at the callee identifier**,
//!   not at the call: `infer_identifier_expression` raises the scheme's
//!   predicates while the identifier is the current expression. The emitter
//!   therefore reads evidence where the *identifier* is lowered.
//! - **The predicates are the scheme's *minimised* context**, in scheme order.
//!   `fn f<a: Eq + Ord>` raises only `Ord`, because `Eq` is implied by it. The
//!   definition's dictionary parameters must follow the same minimised list.
//! - **A marker class takes an index but no dictionary.** Its evidence is
//!   `Evidence::Marker`, so a dictionary's argument position is its index
//!   among the *non-marker* predicates, not its raw index.
//! - **An operator is keyed at the operator expression itself**, with origin
//!   `InferredOperator`.
//!
//! Two gaps, both open:
//!
//! - **A predicate that *is* the enclosing definition's own context is never
//!   recorded.** `lt(x, y)` inside `fn f<a: Ord>` raises `Ord a`, the solver
//!   marks it `Generalized`, and `close_definition_scope` drops it rather than
//!   keeping it as a wanted solved from the given. So the commonest method
//!   call in a constrained body has no evidence. A method reached *through a
//!   superclass* is recorded (`FromGiven` with a non-empty path).
//! - **An unannotated self-recursive function raises everything twice.**
//!   `refine_unannotated_self_recursive_return` infers the body a second time,
//!   and the same `ExprId`s raise the same predicates again, so an operator in
//!   such a body shows `raised=2` for one dictionary.

use std::collections::HashMap;

use crate::{
    ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin},
    source::position::Span,
    syntax::{Identifier, expression::ExprId},
    types::{class_disposition::Evidence, class_id::ClassId},
};

/// What was raised at a site, answered or not.
///
/// Kept for every raised predicate so a consumer can tell a dictionary
/// predicate from a marker one, and so an unanswered site can be reported
/// with where it came from rather than as a bare expression id.
#[derive(Debug, Clone)]
pub struct RaisedPredicate {
    pub class_name: Identifier,
    pub class_id: ClassId,
    pub origin: WantedClassConstraintOrigin,
    pub span: Span,
}

impl From<&WantedClassConstraint> for RaisedPredicate {
    fn from(wanted: &WantedClassConstraint) -> Self {
        Self {
            class_name: wanted.class_name,
            class_id: wanted.class_id,
            origin: wanted.origin,
            span: wanted.span,
        }
    }
}

/// One predicate raised at one expression.
///
/// `index` is the predicate's position among those the expression raised, in
/// the order the solver was handed them — which is also the order a call site
/// passes its dictionaries, so the index doubles as the argument position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvidenceSite {
    pub expr: ExprId,
    pub index: u16,
}

impl EvidenceSite {
    pub fn new(expr: ExprId, index: u16) -> Self {
        Self { expr, index }
    }
}

/// The evidence the solver produced, by site.
///
/// Records how many predicates each expression *raised*, not only which ones
/// were answered. The answered ones alone cannot say whether the last predicate
/// is missing: a site that raised two and had only the first solved looks, by
/// its keys, exactly like a site that raised one.
#[derive(Debug, Clone, Default)]
pub struct EvidenceMap {
    by_site: HashMap<EvidenceSite, Evidence>,
    raised: HashMap<ExprId, u16>,
    predicates: HashMap<EvidenceSite, RaisedPredicate>,
}

impl EvidenceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `expr` raised one more predicate, and return its site.
    ///
    /// Called for every predicate, answered or not, so that an unanswered one
    /// leaves a hole at its position instead of sliding later ones down.
    pub fn raise(&mut self, expr: ExprId, predicate: RaisedPredicate) -> EvidenceSite {
        let count = self.raised.entry(expr).or_insert(0);
        let site = EvidenceSite::new(expr, *count);
        *count += 1;
        self.predicates.insert(site, predicate);
        site
    }

    /// What was raised at `site` — present for every raised position,
    /// including the unanswered ones.
    pub fn predicate(&self, site: &EvidenceSite) -> Option<&RaisedPredicate> {
        self.predicates.get(site)
    }

    /// Record the answer for a site previously returned by [`Self::raise`].
    pub fn insert(&mut self, site: EvidenceSite, evidence: Evidence) {
        debug_assert!(
            site.index < self.raised(site.expr),
            "evidence recorded for a predicate that was never raised: {site:?}"
        );
        self.by_site.insert(site, evidence);
    }

    pub fn get(&self, site: &EvidenceSite) -> Option<&Evidence> {
        self.by_site.get(site)
    }

    /// How many predicates `expr` raised, answered or not.
    pub fn raised(&self, expr: ExprId) -> u16 {
        self.raised.get(&expr).copied().unwrap_or(0)
    }

    /// Every predicate raised at `expr`, in the order a call site passes them.
    ///
    /// Returns `None` when any of the `raised` positions has no answer —
    /// including the last one — rather than a short list: a caller building an
    /// argument list from this must not be handed a partial one, because the
    /// result would be a call with the wrong arity that type-checks.
    pub fn args_for(&self, expr: ExprId) -> Option<Vec<&Evidence>> {
        let count = self.raised(expr);
        if count == 0 {
            return None;
        }
        (0..count)
            .map(|index| self.get(&EvidenceSite::new(expr, index)))
            .collect()
    }

    /// Every instance named by the evidence recorded here.
    ///
    /// Walks an instance's context as well as its head, because a contextual
    /// instance's dictionary is only buildable if the dictionaries it applies
    /// are too.
    pub fn instances(&self) -> impl Iterator<Item = &crate::types::class_disposition::InstanceKey> {
        fn walk<'e>(
            evidence: &'e Evidence,
            out: &mut Vec<&'e crate::types::class_disposition::InstanceKey>,
        ) {
            match evidence {
                Evidence::FromInstance {
                    instance, context, ..
                } => {
                    out.push(instance);
                    for inner in context {
                        walk(inner, out);
                    }
                }
                Evidence::Structural { components } => {
                    for inner in components {
                        walk(inner, out);
                    }
                }
                Evidence::FromGiven { .. } | Evidence::Marker | Evidence::Unrecorded => {}
            }
        }

        let mut out = Vec::new();
        for evidence in self.by_site.values() {
            walk(evidence, &mut out);
        }
        out.into_iter()
    }

    /// Every recorded site and its evidence. Iteration order is unspecified.
    pub fn entries(&self) -> impl Iterator<Item = (&EvidenceSite, &Evidence)> {
        self.by_site.iter()
    }

    /// Every expression that raised a predicate, with its count, in `ExprId`
    /// order. Deterministic, unlike [`Self::entries`].
    pub fn raised_sites(&self) -> Vec<(ExprId, u16)> {
        let mut sites: Vec<_> = self.raised.iter().map(|(e, n)| (*e, *n)).collect();
        sites.sort_unstable();
        sites
    }

    pub fn is_empty(&self) -> bool {
        self.by_site.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_site.len()
    }
}
