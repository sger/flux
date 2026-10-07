//! The evidence emitter: dictionary arguments read from the solver's answer
//! (0.0.8 plan, step 1c).
//!
//! A site's predicates are keyed at the occurrence that raised them — the
//! callee identifier of a call, or the operator itself — so the emitter runs
//! where an occurrence is lowered, not in the `Call` arm. One emitter then
//! covers a call, a bare reference passed as a value, and an operator. This is
//! GHC's model, where evidence wraps the occurrence.
//!
//! **Shadow mode.** What the emitter builds is recorded, not emitted. The
//! existing paths still decide what lowering and dictionary elaboration
//! produce; the record is handed back from lowering so that
//! `core::passes::evidence_diff` can compare it with them, and every
//! disagreement is found before evidence becomes the only source (step 1e).

use std::collections::HashMap;

use crate::{
    source::position::Span,
    syntax::{Identifier, expression::ExprId},
    types::{evidence::EvidenceSite, translate::DictArg},
};

use super::AstLowerer;

/// What the emitter built at one site that raised predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmittedDictArgs {
    /// Every raised predicate was answered and translated. Empty when they
    /// were all markers.
    Built(Vec<DictArg>),
    /// A raised predicate has no answer, or one that cannot be translated.
    ///
    /// After the switch-over this is an internal error naming the site,
    /// except for a predicate the solver filed as
    /// `UnresolvedAfterGeneralization` (see `types/evidence.rs`).
    Unbuildable,
    /// The id's predicates were raised by a different expression.
    ///
    /// `ExprId`s are not stable between inference and lowering everywhere:
    /// code generated between the two — instance-method bodies among it —
    /// is numbered afresh, so an id can name one expression in the evidence
    /// map and another here. The start of the raised predicate's span is
    /// what tells them apart; nothing is built from evidence that fails it.
    Mismatched { raised_at: Span },
}

/// Which kind of occurrence raised a site's predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OccurrenceKind {
    /// An identifier: a call's callee, or a reference passed as a value.
    Identifier(Identifier),
    /// An infix operator.
    Operator,
}

/// One site the emitter visited.
#[derive(Debug, Clone)]
pub struct EmittedSite {
    pub span: Span,
    pub kind: OccurrenceKind,
    pub args: EmittedDictArgs,
}

/// Everything the emitter recorded while lowering one program.
#[derive(Debug, Clone, Default)]
pub struct EvidenceShadow {
    /// Each visited site that raised predicates, by the occurrence's id.
    pub sites: HashMap<ExprId, EmittedSite>,
    /// The span of the call each callee identifier sits in.
    ///
    /// Core carries spans, not ids, and the existing paths do not keep the
    /// callee's span on everything they build: a call they dispatch directly
    /// to an instance method, or project out of a dictionary, carries the
    /// call's span instead.
    pub call_spans: HashMap<ExprId, Span>,
}

impl AstLowerer<'_> {
    /// Record the dictionary arguments the occurrence `id` passes.
    ///
    /// A site that raised nothing is not recorded, so the record holds only
    /// the sites a dictionary decision was made at.
    pub(super) fn emit_dict_args(&mut self, id: ExprId, span: Span, kind: OccurrenceKind) {
        let Some(evidence) = self.evidence else {
            return;
        };
        if evidence.raised(id) == 0 {
            return;
        }
        let raised_at = evidence
            .predicate(&EvidenceSite::new(id, 0))
            .map(|predicate| predicate.span);
        let args = match raised_at {
            Some(raised_at) if raised_at.start != span.start => {
                EmittedDictArgs::Mismatched { raised_at }
            }
            _ => match evidence.dict_args_at(id) {
                Some(args) => EmittedDictArgs::Built(args),
                None => EmittedDictArgs::Unbuildable,
            },
        };
        self.shadow
            .sites
            .insert(id, EmittedSite { span, kind, args });
    }

    /// Remember the span of the call `callee` is the function of.
    pub(super) fn record_call_span(&mut self, callee: ExprId, span: Span) {
        if self.evidence.is_some() {
            self.shadow.call_spans.insert(callee, span);
        }
    }
}
