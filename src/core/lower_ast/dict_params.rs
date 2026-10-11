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
//!
//! In [`EvidenceMode::Emit`] (1e) elaboration no longer adds parameters, so
//! every definition evidence records parameters for gets them here, nested
//! functions included.

use crate::{
    ast::type_infer::constraint::{SchemeConstraint, WantedClassConstraintOrigin},
    core::{CoreBinder, CoreBinderId, CoreExpr, FluxRep},
    diagnostics::DiagnosticBuilder,
    source::position::Span,
    syntax::{Identifier, expression::ExprId, interner::Interner},
    types::{class_id::ClassId, evidence::EvidenceSite, translate::DictArg},
};

use super::{
    AstLowerer,
    evidence::{DefinitionDisagreement, EvidenceMode},
};

impl AstLowerer<'_> {
    /// The dictionary parameters to put ahead of `params` for the definition
    /// `id` named `name`, already recorded as its evidence parameters.
    ///
    /// A definition generated with explicit `__dict_*` parameters (an
    /// instance method with a context, or a forwarder) keeps them: each
    /// evidence parameter is matched to the explicit one with its canonical
    /// name, and nothing is prepended.
    ///
    /// In [`EvidenceMode::Emit`] the parameters are created from evidence
    /// alone, wherever the definition is. A recorded parameter with no name,
    /// or with no explicit parameter to match, is an internal error.
    pub(super) fn evidence_dict_params(
        &mut self,
        id: ExprId,
        name: Identifier,
        params: &[CoreBinder],
        span: Span,
    ) -> Vec<CoreBinder> {
        let (Some(evidence), Some(class_env), Some(interner)) =
            (self.evidence, self.class_env, self.interner)
        else {
            return Vec::new();
        };
        let Some(definition) = evidence.definition(id).filter(|d| !d.params.is_empty()) else {
            return Vec::new();
        };
        let emit = self.evidence_mode == EvidenceMode::Emit;
        let Some(names) = canonical_param_names(&definition.params, interner) else {
            if emit {
                self.internal_error(name, span, "a dictionary parameter name was never interned");
            }
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
                None if emit => self.internal_error(
                    name,
                    span,
                    "its explicit dictionary parameters are not the ones evidence records",
                ),
                None => self.disagree(id, DefinitionDisagreement::ExplicitParams),
            }
            return Vec::new();
        }
        if emit {
            let binders = self.fresh_dict_binders(names);
            self.remember_dict_params(id, &binders);
            return binders;
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
        let error = crate::ice!(format!(
            "lowering `{name}` at {}:{} from its evidence: {problem}",
            span.start.line, span.start.column
        ))
        .with_span(span);
        self.errors.push(error);
    }

    /// [`Self::evidence_dict_params`] for a function nested in a body.
    ///
    /// Elaboration never adds parameters to a nested function, so in
    /// [`EvidenceMode::Shadow`] none are created here either: the old paths
    /// pass it no dictionaries.
    pub(super) fn nested_evidence_dict_params(
        &mut self,
        id: ExprId,
        name: Identifier,
        params: &[CoreBinder],
        span: Span,
    ) -> Vec<CoreBinder> {
        if self.evidence_mode != EvidenceMode::Emit {
            return Vec::new();
        }
        self.evidence_dict_params(id, name, params, span)
    }

    /// `method`, read out of the dictionary parameter the call's evidence
    /// names, for the call `call_id` at `span`.
    ///
    /// A method call raises one predicate, on the method's class. When the
    /// solver answered it from a given, the evidence is the owner's
    /// parameter and the superclass slots from there to the class; the
    /// method's own slot follows. This is the projection elaboration builds
    /// (`rewrite_expr`), chosen by the solver instead of by the argument
    /// types.
    ///
    /// `None` leaves the call to the existing paths: any other evidence, or
    /// a parameter lowering did not create (see [`Self::evidence_dict_params`]).
    pub(super) fn param_method_call(
        &self,
        call_id: ExprId,
        method: Identifier,
        span: Span,
    ) -> Option<CoreExpr> {
        let (evidence, class_env) = (self.evidence?, self.class_env?);
        if evidence.raised(call_id) != 1 {
            return None;
        }
        let predicate = evidence.predicate(&EvidenceSite::new(call_id, 0))?;
        if predicate.origin != WantedClassConstraintOrigin::MethodCall
            || predicate.span.start != span.start
        {
            return None;
        }
        let args = evidence.dict_args_at(call_id)?;
        let [dictionary @ DictArg::Param { .. }] = args.as_slice() else {
            return None;
        };
        let slot = class_env.method_slot(predicate.class_id, method)?;
        Some(CoreExpr::TupleField {
            object: Box::new(self.render_dict_arg(dictionary, span)?),
            index: slot,
            span,
        })
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
