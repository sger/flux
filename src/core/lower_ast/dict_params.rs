//! Dictionary parameters from evidence (0.0.8 plan, step 1d).
//!
//! A definition that generalized a context receives one dictionary per
//! recorded parameter, `EvidenceMap::definition(id).params`, in that order:
//! GHC's `abs_ev_vars`. Lowering creates those binders here and remembers
//! which binder is which parameter, so that evidence naming a parameter
//! (`DictArg::Param { owner, index, .. }`) can be rendered as a variable.
//!
//! **Until the switch-over (1e)** the existing paths still pass the
//! dictionary arguments, and elaboration still rewrites the bodies. A binder
//! is only created where elaboration would add the same parameters: a
//! top-level or module definition whose scheme asks for the same classes in
//! the same order. Elaboration then finds them by their canonical names and
//! adds none of its own (`rewrite_constrained_functions`). Anywhere else the
//! two would disagree, nothing is created, and the disagreement is recorded
//! for the evidence diff report.

use crate::{
    ast::type_infer::constraint::SchemeConstraint,
    core::{CoreBinder, CoreBinderId, FluxRep},
    syntax::{Identifier, expression::ExprId, interner::Interner},
    types::class_id::ClassId,
};

use super::{AstLowerer, evidence::DefinitionDisagreement};

impl AstLowerer<'_> {
    /// The dictionary parameters to put ahead of `params` for the definition
    /// `id` named `name`, already recorded as its evidence parameters.
    ///
    /// A definition generated with explicit `__dict_*` parameters (an
    /// instance method with a context, or a forwarder) keeps them: each
    /// evidence parameter is matched to the explicit one with its canonical
    /// name, and nothing is prepended.
    pub(super) fn evidence_dict_params(
        &mut self,
        id: ExprId,
        name: Identifier,
        params: &[CoreBinder],
    ) -> Vec<CoreBinder> {
        let (Some(evidence), Some(class_env), Some(interner)) =
            (self.evidence, self.class_env, self.interner)
        else {
            return Vec::new();
        };
        let Some(definition) = evidence.definition(id).filter(|d| !d.params.is_empty()) else {
            return Vec::new();
        };
        let Some(names) = canonical_param_names(&definition.params, interner) else {
            self.disagree(id, DefinitionDisagreement::UninternedName);
            return Vec::new();
        };

        let explicit = params
            .iter()
            .take_while(|param| {
                crate::types::class_env::is_dictionary_name(interner.resolve(param.name))
            })
            .copied()
            .collect::<Vec<_>>();
        if !explicit.is_empty() {
            let matched = names
                .iter()
                .map(|name| explicit.iter().find(|param| param.name == *name).copied())
                .collect::<Option<Vec<_>>>();
            match matched {
                Some(binders) => self.remember_dict_params(id, &binders),
                None => self.disagree(id, DefinitionDisagreement::ExplicitParams),
            }
            return Vec::new();
        }

        let elaborated = self
            .scheme_for_current_function(name)
            .map(|scheme| {
                scheme
                    .constraints
                    .iter()
                    .filter(|constraint| class_env.constraint_needs_dictionary(constraint))
                    .map(|constraint| constraint.class_id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let recorded = classes(&definition.params);
        if elaborated != recorded {
            let disagreement = if elaborated.is_empty() {
                DefinitionDisagreement::NoScheme
            } else {
                DefinitionDisagreement::Classes
            };
            self.disagree(id, disagreement);
            return Vec::new();
        }
        // Two dictionaries for one class: which is which depends on the type
        // arguments, and elaboration pairs them with the scheme by position.
        // Not worth matching until elaboration's half is gone (1e).
        if has_repeated_class(&recorded) {
            self.disagree(id, DefinitionDisagreement::RepeatedClass);
            return Vec::new();
        }

        let binders = names
            .into_iter()
            .map(|name| {
                let binder_id = CoreBinderId(self.next_binder_id);
                self.next_binder_id += 1;
                CoreBinder::with_rep(binder_id, name, FluxRep::BoxedRep)
            })
            .collect::<Vec<_>>();
        self.remember_dict_params(id, &binders);
        binders
    }

    fn remember_dict_params(&mut self, owner: ExprId, binders: &[CoreBinder]) {
        for (index, binder) in binders.iter().enumerate() {
            let index = u16::try_from(index).expect("a definition holds under 2^16 parameters");
            self.dict_param_binders.insert((owner, index), *binder);
        }
    }

    fn disagree(&mut self, owner: ExprId, disagreement: DefinitionDisagreement) {
        self.shadow.definitions.insert(owner, disagreement);
    }
}

/// The name of each parameter: the class's dictionary prefix, then `_n` for
/// the class's `n`-th repeat. These are the names elaboration and the AST
/// path give the same parameters, so each finds the other's. `None` if one
/// was never interned.
fn canonical_param_names(
    params: &[SchemeConstraint],
    interner: &Interner,
) -> Option<Vec<Identifier>> {
    let mut occurrences = std::collections::HashMap::<ClassId, usize>::new();
    params
        .iter()
        .map(|param| {
            let occurrence = occurrences.entry(param.class_id).or_insert(0);
            let suffix = if *occurrence == 0 {
                String::new()
            } else {
                format!("_{occurrence}")
            };
            *occurrence += 1;
            let prefix = crate::types::class_env::dictionary_prefix(param.class_id, interner);
            interner.lookup(&format!("{prefix}{suffix}"))
        })
        .collect()
}

fn classes(params: &[SchemeConstraint]) -> Vec<ClassId> {
    params.iter().map(|param| param.class_id).collect()
}

fn has_repeated_class(classes: &[ClassId]) -> bool {
    classes
        .iter()
        .enumerate()
        .any(|(index, class)| classes[..index].contains(class))
}
