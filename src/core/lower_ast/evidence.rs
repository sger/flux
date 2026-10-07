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
//! existing `Call` paths still decide what lowering produces; the record is
//! there to be compared with them, so that every disagreement is found before
//! evidence becomes the only source (step 1e).

use crate::{
    ast::type_infer::constraint::SchemeConstraint, syntax::expression::ExprId,
    types::translate::DictArg,
};

use super::AstLowerer;

/// What the emitter built at one site that raised predicates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EmittedDictArgs {
    /// Every raised predicate was answered and translated. Empty when they
    /// were all markers.
    Built(Vec<DictArg>),
    /// A raised predicate has no answer, or one that cannot be translated.
    ///
    /// After the switch-over this is an internal error naming the site,
    /// except for a predicate the solver filed as
    /// `UnresolvedAfterGeneralization` (see `types/evidence.rs`).
    Unbuildable,
}

impl AstLowerer<'_> {
    /// Record the dictionary arguments the occurrence `id` passes.
    ///
    /// A site that raised nothing is not recorded, so the record holds only
    /// the sites a dictionary decision was made at.
    pub(super) fn emit_dict_args(&mut self, id: ExprId) {
        let Some(evidence) = self.evidence else {
            return;
        };
        if evidence.raised(id) == 0 {
            return;
        }
        let givens = self.current_dictionary_givens();
        let emitted = match evidence.dict_args_at(id, &givens) {
            Some(args) => EmittedDictArgs::Built(args),
            None => EmittedDictArgs::Unbuildable,
        };
        self.emitted_dict_args.insert(id, emitted);
    }

    /// The enclosing definition's predicates that carry a dictionary, in the
    /// order it receives them.
    ///
    /// Markers are left out, because they get no parameter: a
    /// [`DictArg::Param`] index counts dictionaries, not predicates. Empty
    /// outside a function body.
    ///
    /// The scheme comes from the same lookup the existing paths use
    /// (`scheme_for_current_function`), so a definition whose scheme is found
    /// wrongly is wrong in both, and a comparison will not see it.
    fn current_dictionary_givens(&self) -> Vec<SchemeConstraint> {
        let Some(scheme) = self
            .current_function_name
            .and_then(|name| self.scheme_for_current_function(name))
        else {
            return Vec::new();
        };
        scheme
            .constraints
            .into_iter()
            .filter(|constraint| {
                self.class_env
                    .is_none_or(|class_env| class_env.constraint_needs_dictionary(constraint))
            })
            .collect()
    }
}
