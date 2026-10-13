//! Dictionary parameters from evidence (0.0.8 plan, step 1d).
//!
//! A definition that generalized a context receives one dictionary per
//! recorded parameter, `EvidenceMap::definition(id).params`, in that order:
//! GHC's `abs_ev_vars`. Lowering creates those binders here and remembers
//! which binder is which parameter, so that evidence naming a parameter
//! (`DictArg::Param { owner, index, .. }`) can be rendered as a variable.
//!
//! Since step 1e these are the only dictionary parameters: every definition
//! evidence records parameters for gets them here, nested functions
//! included.

use crate::{
    ast::type_infer::constraint::SchemeConstraint,
    core::{CoreBinder, CoreBinderId, CoreExpr, FluxRep},
    source::position::Span,
    syntax::{Identifier, expression::ExprId, interner::Interner},
    types::{class_id::ClassId, translate::DictArg},
};

use super::AstLowerer;

impl AstLowerer<'_> {
    /// The dictionary parameters to put ahead of `params` for the definition
    /// `id` named `name`, already recorded as its evidence parameters.
    ///
    /// A definition generated with explicit `__dict_*` parameters (an
    /// instance method with a context, or a forwarder) keeps them: each
    /// evidence parameter is matched to the explicit one with its canonical
    /// name, and nothing is prepended.
    ///
    /// The parameters are created from evidence alone, wherever the
    /// definition is. A recorded parameter with no name, or with no explicit
    /// parameter to match, is an internal error.
    pub(super) fn evidence_dict_params(
        &mut self,
        id: ExprId,
        name: Identifier,
        params: &[CoreBinder],
        span: Span,
    ) -> Vec<CoreBinder> {
        let (Some(evidence), Some(interner)) = (self.evidence, self.interner) else {
            return Vec::new();
        };
        let Some(definition) = evidence.definition(id).filter(|d| !d.params.is_empty()) else {
            return Vec::new();
        };
        self.next_body_constrained = Some(name);
        let Some(names) = canonical_param_names(&definition.params, interner) else {
            self.internal_error(name, span, "a dictionary parameter name was never interned");
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
                None => self.internal_error(
                    name,
                    span,
                    "its explicit dictionary parameters are not the ones evidence records",
                ),
            }
            return Vec::new();
        }
        let binders = self.fresh_dict_binders(names);
        self.remember_dict_params(id, &binders);
        binders
    }

    fn fresh_dict_binders(&mut self, names: Vec<Identifier>) -> Vec<CoreBinder> {
        names
            .into_iter()
            .map(|name| {
                let binder_id = CoreBinderId(self.next_binder_id);
                self.next_binder_id += 1;
                CoreBinder::with_rep(binder_id, name, FluxRep::BoxedRep)
            })
            .collect()
    }

    /// Record that the definition `name` at `span` cannot be lowered from
    /// its evidence.
    fn internal_error(&mut self, name: Identifier, span: Span, problem: &str) {
        let name = self
            .interner
            .and_then(|interner| interner.try_resolve(name))
            .unwrap_or("<unnamed>");
        self.site_error(span, &format!("the definition `{name}`: {problem}"));
    }

    /// The Core for one dictionary argument evidence names.
    ///
    /// - `Global`: the instance's dictionary, a global.
    /// - `Applied`: that global applied to the dictionaries its context
    ///   needs, the shape of [`CoreExpr::dictionary_ref`].
    /// - `Param`: the owner's parameter, then the superclass slots along
    ///   `path`.
    ///
    /// `None` when the dictionary was never interned, or the parameter has no
    /// binder: lowering did not create the owner's parameters. A `DictArg::None`
    /// is a marker and passes nothing, so it is `None` too; `dict_args_at`
    /// never returns one.
    pub(super) fn render_dict_arg(&self, arg: &DictArg, span: Span) -> Option<CoreExpr> {
        match arg {
            DictArg::Global { instance } => Some(CoreExpr::external_var(
                instance.dict_name(self.interner?)?,
                span,
            )),
            DictArg::Applied { instance, context } => Some(CoreExpr::App {
                func: Box::new(CoreExpr::external_var(
                    instance.dict_name(self.interner?)?,
                    span,
                )),
                args: context
                    .iter()
                    .map(|arg| self.render_dict_arg(arg, span))
                    .collect::<Option<_>>()?,
                span,
            }),
            DictArg::Param { owner, index, path } => {
                let binder = self
                    .dict_param_binders
                    .get(&(*owner, u16::try_from(*index).ok()?))?;
                Some(
                    path.iter()
                        .fold(CoreExpr::bound_var(binder, span), |object, &index| {
                            CoreExpr::TupleField {
                                object: Box::new(object),
                                index,
                                span,
                            }
                        }),
                )
            }
            DictArg::None => None,
        }
    }

    fn remember_dict_params(&mut self, owner: ExprId, binders: &[CoreBinder]) {
        for (index, binder) in binders.iter().enumerate() {
            let index = u16::try_from(index).expect("a definition holds under 2^16 parameters");
            self.dict_param_binders.insert((owner, index), *binder);
        }
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
