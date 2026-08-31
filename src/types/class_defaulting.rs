use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::{
        SchemeConstraint, WantedClassConstraint, WantedClassConstraintOrigin,
    },
    diagnostics::Diagnostic,
    syntax::interner::Interner,
    types::{
        TypeVarId,
        class_disposition::{Disposition, DispositionedConstraint, SolveOutcome, SolveScope},
        class_env::ClassEnv,
        class_solver::{solve_class_constraints_dispositioned, split},
        infer_type::InferType,
        type_constructor::TypeConstructor,
        type_subst::TypeSubst,
    },
};

#[derive(Debug)]
pub struct FinalizedBindingClassConstraints {
    pub infer_type: InferType,
    pub scheme_constraints: Vec<SchemeConstraint>,
    /// Substitution produced by numeric defaulting. Callers compose this into
    /// the inference context's substitution so downstream expressions see the
    /// defaulted types (see `InferCtx::finalize_binding`).
    pub default_subst: TypeSubst,
    pub diagnostics: Vec<Diagnostic>,
    /// The outcome assigned to every wanted predicate this binding owns
    /// (Proposal 0179 Stage 3). Exactly one entry per supplied constraint, so
    /// no obligation can go unaccounted for.
    pub dispositions: Vec<DispositionedConstraint>,
}

#[derive(Debug, Default)]
struct VarConstraintSummary {
    saw_num_var_obligation: bool,
    blocked: bool,
}

/// Finalize one binding's class obligations before generalization.
///
/// Behavior:
/// - Apply the current substitution to the binding type and wanted constraints.
/// - Default truly ambiguous single-parameter `Num` variables to `Int`.
/// - Validate the finalized concrete obligations against the class environment.
/// - Return the updated type plus residual scheme constraints.
pub fn finalize_binding_class_constraints(
    infer_type: &InferType,
    env_free_vars: &HashSet<TypeVarId>,
    constraints: &[WantedClassConstraint],
    current_subst: &TypeSubst,
    class_env: Option<&ClassEnv>,
    interner: &Interner,
    mode: GeneralizationMode,
) -> FinalizedBindingClassConstraints {
    let resolved_type = infer_type.apply_type_subst(current_subst);
    let resolved_constraints = apply_wanted_constraints_subst(constraints, current_subst);
    let public_vars: HashSet<TypeVarId> = resolved_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();

    let default_subst = build_numeric_default_subst(&resolved_constraints, &public_vars, interner);
    let finalized_type = resolved_type.apply_type_subst(&default_subst);
    let finalized_constraints =
        apply_wanted_constraints_subst(&resolved_constraints, &default_subst);
    let outcome = class_env
        .map(|env| {
            solve_class_constraints_dispositioned(
                &finalized_constraints,
                SolveScope::Binding,
                env,
                interner,
            )
        })
        .unwrap_or_default();
    let diagnostics: Vec<Diagnostic> = outcome.diagnostics().cloned().collect();

    // Which predicates this binding generalizes is still decided by
    // `collect_scheme_constraints`. Stage 3 folds that decision into the
    // solver's disposition (THIH's `split`); until then, record the outcome
    // on the dispositions so both halves report the same thing.
    let scheme_constraints =
        collect_scheme_constraints(&finalized_constraints, &finalized_type, env_free_vars, mode);
    let dispositions = mark_generalized(outcome, &scheme_constraints);

    FinalizedBindingClassConstraints {
        infer_type: finalized_type,
        scheme_constraints,
        default_subst,
        diagnostics,
        dispositions,
    }
}

/// Reconcile the solver's outcome with the constraints generalization kept.
///
/// A predicate the solver left `Stuck` because it was still polymorphic is
/// not undecided if this binding went on to quantify it — it was
/// generalized, and its obligation now transfers to every call site. Marking
/// it here keeps the two halves of the decision consistent while they remain
/// separate functions.
fn mark_generalized(
    outcome: SolveOutcome,
    scheme_constraints: &[SchemeConstraint],
) -> Vec<DispositionedConstraint> {
    outcome
        .dispositions
        .into_iter()
        .map(|mut entry| {
            if matches!(entry.disposition, Disposition::Stuck { .. })
                && let Some(scheme_constraint) = scheme_constraints.iter().find(|candidate| {
                    candidate.class_name == entry.wanted.class_name
                        && candidate.type_args == entry.wanted.type_args
                })
            {
                entry.disposition = Disposition::Generalized {
                    scheme_constraint: scheme_constraint.clone(),
                };
            }
            entry
        })
        .collect()
}

fn apply_wanted_constraints_subst(
    constraints: &[WantedClassConstraint],
    subst: &TypeSubst,
) -> Vec<WantedClassConstraint> {
    constraints
        .iter()
        .cloned()
        .map(|mut constraint| {
            constraint.type_args = constraint
                .type_args
                .iter()
                .map(|ty| ty.apply_type_subst(subst))
                .collect();
            constraint
        })
        .collect()
}

fn build_numeric_default_subst(
    constraints: &[WantedClassConstraint],
    public_vars: &HashSet<TypeVarId>,
    interner: &Interner,
) -> TypeSubst {
    // Pre-intern the `Num` class name once. If it was never interned in this
    // session no `Num` constraints exist, so every var gets marked `blocked`
    // (nothing to default) and we still do the work — but an identifier-equality
    // check per constraint is cheaper than a per-constraint string compare, and
    // it keeps the extension point clear for future numeric classes (Fractional,
    // Integral, etc.) where we would intern additional IDs here.
    let num_id = interner.lookup("Num");

    let mut summaries: HashMap<TypeVarId, VarConstraintSummary> = HashMap::new();

    for constraint in constraints {
        let vars = constraint
            .type_args
            .iter()
            .flat_map(InferType::free_vars)
            .collect::<HashSet<_>>();
        if vars.is_empty() {
            continue;
        }

        let is_single_num = constraint.type_args.len() == 1
            && num_id.is_some_and(|id| id == constraint.class_name)
            && matches!(constraint.type_args.first(), Some(InferType::Var(_)));

        if is_single_num && constraint.origin != WantedClassConstraintOrigin::ExplicitBound {
            if let Some(InferType::Var(var)) = constraint.type_args.first() {
                summaries.entry(*var).or_default().saw_num_var_obligation = true;
            }
            continue;
        }

        for var in vars {
            summaries.entry(var).or_default().blocked = true;
        }
    }

    let mut subst = TypeSubst::empty();
    for (var, summary) in summaries {
        if summary.saw_num_var_obligation && !summary.blocked && !public_vars.contains(&var) {
            subst.insert(var, InferType::Con(TypeConstructor::Int));
        }
    }
    subst
}

/// Whether a binding's obligations can be passed as dictionary parameters.
///
/// Dictionary elaboration rewrites top-level definitions and their call sites
/// (`insert_dict_args_at_call_sites`). A `let`-bound lambda inside a function
/// body is a `CoreExpr::Let`, not a definition, so a dictionary parameter
/// added to its scheme would never be supplied by its callers and the call
/// would fail with an arity mismatch at runtime.
///
/// Until elaboration reaches nested bindings, only definitions retain
/// dictionary-carrying obligations. This is a deliberate, narrow restriction
/// on *which* obligations become dictionary parameters, not a return to
/// dropping them: a `let` binding whose obligation cannot be discharged is
/// still reported by the solver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneralizationMode {
    /// A top-level definition; elaboration rewrites its call sites.
    Definition,
    /// A nested binding; elaboration cannot supply dictionaries here.
    NestedBinding,
}

/// Determine which obligations this binding retains on its scheme.
///
/// This is THIH's `split`: predicates are *partitioned* by whether their
/// variables are quantified here, rather than filtered by a list of ad-hoc
/// conditions. A partition cannot discard, which is what makes Proposal 0179
/// Goal 2 ("never silently discarded") structural rather than a property that
/// has to be separately maintained.
///
/// Every predicate mentioning a variable this binding quantifies is retained,
/// regardless of how it was emitted or which class it names. The four
/// pre-Stage-3 escape hatches this replaces each dropped real obligations:
///
/// - operator-derived predicates were discarded outright, so
///   `fn double<a>(x: a) -> a { x + x }` generalized without `Num<a>` and
///   accepted `double("ab")`;
/// - structured predicates over built-in classes were discarded, losing
///   `Eq<List<a>>`;
/// - deduplication keyed only on bare type variables, so `C<List<a>>` and
///   `C<Option<a>>` collided and one was lost;
/// - retention required all arguments to be bare variables, so anything else
///   vanished.
fn collect_scheme_constraints(
    constraints: &[WantedClassConstraint],
    infer_type: &InferType,
    env_free_vars: &HashSet<TypeVarId>,
    mode: GeneralizationMode,
) -> Vec<SchemeConstraint> {
    let quantified: HashSet<TypeVarId> = infer_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();

    let (_deferred, retained) = split(constraints, env_free_vars, &quantified);

    let mut result: Vec<SchemeConstraint> = Vec::new();
    for constraint in retained {
        // Only obligations over variables this binding quantifies become
        // scheme constraints; a fully concrete predicate was already
        // discharged against an instance by the solver.
        let mentions_quantified = constraint
            .type_args
            .iter()
            .flat_map(InferType::free_vars)
            .any(|var| quantified.contains(&var));
        if !mentions_quantified {
            continue;
        }

        // A nested binding cannot receive a dictionary parameter, so an
        // obligation that would need one is left for the enclosing scope to
        // discharge rather than recorded on a scheme no caller can satisfy.
        if mode == GeneralizationMode::NestedBinding
            && constraint.origin == WantedClassConstraintOrigin::InferredOperator
        {
            continue;
        }

        let candidate = SchemeConstraint {
            class_name: constraint.class_name,
            type_args: constraint.type_args.clone(),
        };
        // Deduplicate on the whole predicate, so two distinct structured
        // obligations over the same variable both survive.
        if !result.contains(&candidate) {
            result.push(candidate);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{GeneralizationMode, finalize_binding_class_constraints};
    use crate::{
        ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin},
        diagnostics::position::Span,
        syntax::interner::Interner,
        types::{
            class_env::ClassEnv, infer_type::InferType, type_constructor::TypeConstructor,
            type_subst::TypeSubst,
        },
    };

    fn var(id: u32) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    fn bool_() -> InferType {
        InferType::Con(TypeConstructor::Bool)
    }

    fn wanted(
        class_name: crate::syntax::Identifier,
        type_args: Vec<InferType>,
        origin: WantedClassConstraintOrigin,
    ) -> WantedClassConstraint {
        WantedClassConstraint {
            class_name,
            type_args,
            span: Span::default(),
            origin,
            originated_from_concrete_type: false,
        }
    }

    #[test]
    fn defaults_truly_ambiguous_num_variable_to_int() {
        let mut interner = Interner::new();
        let num = interner.intern("Num");
        let finalized = finalize_binding_class_constraints(
            &bool_(),
            &HashSet::new(),
            &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            &TypeSubst::empty(),
            None,
            &interner,
            GeneralizationMode::Definition,
        );

        assert_eq!(finalized.infer_type, bool_());
        assert_eq!(finalized.default_subst.get(0), Some(&int()));
        assert!(finalized.scheme_constraints.is_empty());
    }

    #[test]
    fn does_not_default_num_variable_that_appears_in_public_type() {
        let mut interner = Interner::new();
        let num = interner.intern("Num");
        let finalized = finalize_binding_class_constraints(
            &InferType::Fun(
                vec![var(0)],
                Box::new(var(0)),
                crate::types::infer_effect_row::InferEffectRow::closed_empty(),
            ),
            &HashSet::new(),
            &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            &TypeSubst::empty(),
            None,
            &interner,
            GeneralizationMode::Definition,
        );

        assert!(finalized.default_subst.is_empty());
        assert_eq!(
            finalized.infer_type,
            InferType::Fun(
                vec![var(0)],
                Box::new(var(0)),
                crate::types::infer_effect_row::InferEffectRow::closed_empty()
            )
        );
    }

    #[test]
    fn explicit_num_bound_stays_constrained() {
        let mut interner = Interner::new();
        let num = interner.intern("Num");
        let finalized = finalize_binding_class_constraints(
            &var(0),
            &HashSet::new(),
            &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::ExplicitBound,
            )],
            &TypeSubst::empty(),
            None,
            &interner,
            GeneralizationMode::Definition,
        );

        assert!(finalized.default_subst.is_empty());
        assert_eq!(finalized.scheme_constraints.len(), 1);
        assert_eq!(
            finalized.scheme_constraints[0].type_args,
            vec![InferType::Var(0)]
        );
    }

    #[test]
    fn mixed_num_and_eq_constraints_do_not_default() {
        let mut interner = Interner::new();
        let num = interner.intern("Num");
        let eq = interner.intern("Eq");
        let finalized = finalize_binding_class_constraints(
            &bool_(),
            &HashSet::new(),
            &[
                wanted(num, vec![var(0)], WantedClassConstraintOrigin::MethodCall),
                wanted(eq, vec![var(0)], WantedClassConstraintOrigin::MethodCall),
            ],
            &TypeSubst::empty(),
            None,
            &interner,
            GeneralizationMode::Definition,
        );

        assert!(finalized.default_subst.is_empty());
    }

    #[test]
    fn validates_concrete_constraints_after_defaulting() {
        let mut interner = Interner::new();
        let mut class_env = ClassEnv::new();
        class_env.register_builtins(&mut interner);
        let num = interner.lookup("Num").expect("Num should be interned");
        let finalized = finalize_binding_class_constraints(
            &bool_(),
            &HashSet::new(),
            &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            &TypeSubst::empty(),
            Some(&class_env),
            &interner,
            GeneralizationMode::Definition,
        );

        assert!(
            finalized.diagnostics.is_empty(),
            "defaulted Num<Int> should validate cleanly, got: {:?}",
            finalized.diagnostics
        );
    }
}
