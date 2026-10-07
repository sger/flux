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
//! - **A class-method call is keyed at the call**, not the callee: its
//!   predicate (origin `MethodCall`) is raised once the arguments' types are
//!   known, while the call is the current expression. Ids are assigned in
//!   post-order, so the call's id follows its callee's and arguments'.
//! - **An operator is keyed at the operator expression itself**, with origin
//!   `InferredOperator`.
//!
//! - **A method on the enclosing definition's own dictionary is answered from
//!   it:** `lt(x, y)` inside `fn f<a: Ord>` records `FromGiven` with an empty
//!   path; one reached through a superclass records the superclass path.
//! - **Each predicate is raised once per site.** Passes that infer the same
//!   expressions a second time — the recursive-return refinement, and checking
//!   a propagatable argument before inferring it — discard what they raise.
//!
//! One kind of site has no evidence by construction: a predicate the solver
//! files as `UnresolvedAfterGeneralization`, raised in an unannotated helper
//! that uses an operator on a type it does not fix and that today's rule does
//! not generalize. Five such sites exist in `Flow.List`. Generalizing
//! constrained definitions (0.0.8 plan, step 2b) gives them a context to be
//! answered from; until then a consumer must not treat their absence as an
//! internal error.

use std::collections::HashMap;

use crate::{
    ast::type_infer::constraint::{
        SchemeConstraint, WantedClassConstraint, WantedClassConstraintOrigin,
    },
    source::position::Span,
    syntax::{Identifier, expression::ExprId},
    types::{
        class_disposition::Evidence,
        class_id::ClassId,
        translate::{DictArg, evidence_to_arg},
    },
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
    definitions: HashMap<ExprId, DefinitionParams>,
}

/// A generalized definition's dictionary parameters — GHC's `abs_ev_vars`.
///
/// Stored with the evidence so that lowering reads a definition's parameters
/// from the same place its call sites' evidence comes from, by the
/// definition's id, instead of finding its scheme by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionParams {
    /// The givens that become dictionary parameters, in the order the
    /// definition receives them.
    pub params: Vec<SchemeConstraint>,
    /// For each given, by its position among the definition's givens, the
    /// parameter it is — `None` for a given that gets none.
    pub param_of_given: Vec<Option<u16>>,
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

    /// The dictionary arguments `expr` passes, in argument order.
    ///
    /// `expr` is the site the predicates were keyed at: the callee identifier
    /// for a call, the operator itself for an operator. `givens` are the
    /// enclosing definition's predicates, in the order it receives their
    /// dictionaries (see [`evidence_to_arg`]).
    ///
    /// A marker predicate takes an index but no argument, so it is dropped: a
    /// dictionary's position is its index among the non-marker predicates.
    ///
    /// `Some(vec![])` when `expr` raised nothing — unlike [`Self::args_for`],
    /// because every identifier is asked, and raising nothing means passing
    /// nothing. `None` when any raised predicate has no answer, or an answer
    /// [`evidence_to_arg`] cannot build. Never a short list.
    pub fn dict_args_at(&self, expr: ExprId, givens: &[SchemeConstraint]) -> Option<Vec<DictArg>> {
        if self.raised(expr) == 0 {
            return Some(Vec::new());
        }
        let args = self
            .args_for(expr)?
            .into_iter()
            .map(|evidence| evidence_to_arg(evidence, givens))
            .collect::<Option<Vec<_>>>()?;
        Some(
            args.into_iter()
                .filter(|arg| *arg != DictArg::None)
                .collect(),
        )
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

    /// Record every generalized definition's dictionary parameters, read
    /// from the implication each one closed.
    ///
    /// A given becomes a parameter when its type mentions only variables the
    /// definition quantified — the rule `Quantified::into_scheme` keeps a
    /// scheme constraint by — and its class carries a dictionary. Those are
    /// the two filters `dictionary_constraints` applies to the scheme, in the
    /// same order, so the list matches the parameters elaboration creates.
    pub fn record_definitions(
        &mut self,
        wanted: &crate::ast::type_infer::constraint::WantedConstraints,
        class_env: &crate::types::class_env::ClassEnv,
    ) {
        for implication in &wanted.implications {
            let quantified: std::collections::HashSet<_> =
                implication.quantified.iter().copied().collect();
            let mut params = Vec::new();
            let param_of_given = implication
                .givens
                .iter()
                .map(|given| {
                    let is_param = given
                        .type_args
                        .iter()
                        .flat_map(crate::types::infer_type::InferType::free_vars)
                        .all(|var| quantified.contains(&var))
                        && class_env.constraint_needs_dictionary(given);
                    is_param.then(|| {
                        params.push(given.clone());
                        u16::try_from(params.len() - 1)
                            .expect("a definition holds under 2^16 parameters")
                    })
                })
                .collect();
            let recorded = DefinitionParams {
                params,
                param_of_given,
            };
            // A definition closes one implication. Should it ever close a
            // second, the two must agree, or one of them is wrong.
            let existing = self
                .definitions
                .entry(implication.definition)
                .or_insert_with(|| recorded.clone());
            debug_assert_eq!(
                *existing, recorded,
                "definition {:?} closed two implications with different parameters",
                implication.definition
            );
            self.record_definitions(&implication.wanted, class_env);
        }
    }

    /// The dictionary parameters of the definition with this id, if it
    /// generalized a context.
    pub fn definition(&self, id: ExprId) -> Option<&DefinitionParams> {
        self.definitions.get(&id)
    }

    /// Every recorded definition, in id order.
    pub fn definitions(&self) -> Vec<(ExprId, &DefinitionParams)> {
        let mut definitions: Vec<_> = self.definitions.iter().map(|(id, d)| (*id, d)).collect();
        definitions.sort_unstable_by_key(|(id, _)| *id);
        definitions
    }

    pub fn is_empty(&self) -> bool {
        self.by_site.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_site.len()
    }
}
