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

use std::collections::HashMap;

use crate::{syntax::expression::ExprId, types::class_disposition::Evidence};

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
#[derive(Debug, Clone, Default)]
pub struct EvidenceMap {
    by_site: HashMap<EvidenceSite, Evidence>,
}

impl EvidenceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, site: EvidenceSite, evidence: Evidence) {
        self.by_site.insert(site, evidence);
    }

    pub fn get(&self, site: &EvidenceSite) -> Option<&Evidence> {
        self.by_site.get(site)
    }

    /// Every predicate raised at `expr`, in the order a call site passes them.
    ///
    /// Returns `None` when any index in `0..len` is missing, rather than a
    /// short list: a caller building an argument list from this must not be
    /// handed a partial one, because the result would be a call with the wrong
    /// arity that type-checks.
    pub fn args_for(&self, expr: ExprId) -> Option<Vec<&Evidence>> {
        let count = self.by_site.keys().filter(|site| site.expr == expr).count();
        if count == 0 {
            return None;
        }
        (0..count)
            .map(|index| self.get(&EvidenceSite::new(expr, index as u16)))
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by_site.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_site.len()
    }
}
