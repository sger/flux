use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::{
        SchemeConstraint, WantedClassConstraint, WantedClassConstraintOrigin,
    },
    diagnostics::{
        Diagnostic, DiagnosticBuilder, compiler_errors::AMBIGUOUS_TYPE_VARIABLE, diagnostic_for,
    },
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

    let default_subst =
        build_numeric_default_subst(&resolved_constraints, &public_vars, class_env, interner);
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
    let mut diagnostics: Vec<Diagnostic> = outcome.diagnostics().cloned().collect();

    // Which predicates this binding generalizes is still decided by
    // `collect_scheme_constraints`. Stage 3 folds that decision into the
    // solver's disposition (THIH's `split`); until then, record the outcome
    // on the dispositions so both halves report the same thing.
    let scheme_constraints = collect_scheme_constraints(
        &finalized_constraints,
        &finalized_type,
        env_free_vars,
        mode,
        class_env,
        interner,
    );
    let dispositions = mark_generalized(outcome, &scheme_constraints);

    // A declared bound over a variable this signature never mentions cannot be
    // discharged by any caller (Haskell Report §4.3.4).
    let quantified: HashSet<TypeVarId> = finalized_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();
    diagnostics.extend(ambiguous_bound_diagnostics(
        &finalized_constraints,
        &quantified,
        env_free_vars,
        interner,
    ));

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
                    candidate.class_id == entry.wanted.class_id
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

/// The types tried for a defaultable variable, in order.
///
/// Haskell's `default (Integer, Double)` over Flux's own numeric tower. `Int`
/// comes first so that every program that already defaulted keeps the type it
/// had; `Float` is reached only by a variable whose other obligations `Int`
/// cannot meet, which previously had no terminal state at all.
const NUMERIC_DEFAULT_CANDIDATES: [TypeConstructor; 2] =
    [TypeConstructor::Int, TypeConstructor::Float];

/// Resolve ambiguous variables by defaulting, following GHC's
/// `disambigGroup` (`GHC.Tc.Solver.Default`) and Note [How type-class
/// constraints are defaulted].
///
/// The four steps, and what each one means here:
///
/// 1. Group the *unary* obligations — those of the form `C<v>` — by their
///    variable, and record which variables a non-unary obligation mentions.
///    GHC's `findDefaultableGroups` blocks a variable only on the non-unary
///    ones; `Num<v>` beside `Show<v>` is still a defaultable group, whereas
///    `Num<v>` beside `Convert<v, b>` is not.
/// 2. Keep a group only if some obligation in it belongs to a class that has a
///    default declaration — for Flux, `Num`.
/// 3. Try each candidate in order, keeping it only if it discharges *every*
///    obligation in the group, not merely the `Num` one.
/// 4. Commit the first candidate that survives.
///
/// The commit is a substitution the caller composes and then re-solves
/// against, so a candidate accepted here on an instance head whose own context
/// turns out to be unsatisfiable is still reported by the solver rather than
/// silently accepted.
///
/// Step 3 is what the previous implementation lacked: it blocked a variable as
/// soon as any second obligation mentioned it and then committed to `Int`
/// unverified, so `Num<v>` with `Show<v>` defaulted to nothing and stayed
/// stuck, while `Num<v>` alone defaulted to `Int` whether or not `Int` had the
/// instance.
///
/// ## This never fires today, and that is the point of measuring it
///
/// Traced over all 1,305 `.flx` programs in the repository, no variable ever
/// reaches step 2: every candidate group is blocked first. Flux has no
/// `Num`-polymorphic literal — `1` is `Int`, not `Num<a> => a` — so an
/// ambiguous numeric variable, the thing defaulting exists to resolve, does
/// not arise. `Float` is therefore unreachable rather than a language change,
/// and the residue Proposal 0183 is escalating (`Ord`, `Eq`, `Sendable`)
/// contains no `Num` obligation for defaulting to act on.
///
/// It is written to GHC's rule anyway because the alternative is a function
/// that is *both* dead and wrong: when Flux gives literals a `Num` bound, the
/// unverified single-candidate version would start committing to `Int` without
/// checking, and the failure would look like a mis-inferred type rather than a
/// defaulting bug.
fn build_numeric_default_subst(
    constraints: &[WantedClassConstraint],
    public_vars: &HashSet<TypeVarId>,
    class_env: Option<&ClassEnv>,
    interner: &Interner,
) -> TypeSubst {
    // Pre-intern the `Num` class name once. If it was never interned in this
    // session no `Num` obligation exists and no group passes step 2. This is
    // also the extension point for further defaultable classes (`Fractional`,
    // `Integral`), each of which would bring its own candidate list.
    let num_id = interner.lookup("Num");
    let is_num = |constraint: &WantedClassConstraint| {
        num_id.is_some_and(|id| {
            constraint.class_id.module.is_empty() && constraint.class_id.name == id
        })
    };

    // Step 1.
    let mut unary: HashMap<TypeVarId, Vec<&WantedClassConstraint>> = HashMap::new();
    let mut blocked: HashSet<TypeVarId> = HashSet::new();
    for constraint in constraints {
        let vars = constraint
            .type_args
            .iter()
            .flat_map(InferType::free_vars)
            .collect::<HashSet<_>>();
        if vars.is_empty() {
            continue;
        }

        // A bound the signature *declares* is not ambiguity: the variable it
        // constrains is the caller's to choose, so defaulting it here would
        // silently monomorphise a polymorphic signature.
        match constraint.type_args.as_slice() {
            [InferType::Var(var)]
                if constraint.origin != WantedClassConstraintOrigin::ExplicitBound =>
            {
                unary.entry(*var).or_default().push(constraint);
            }
            _ => blocked.extend(vars),
        }
    }

    // Sorted so that the substitution is built in the same order on every run,
    // whatever the hash seed. Each variable is decided independently, so this
    // does not change the result — it keeps it reproducible.
    let mut groups: Vec<(TypeVarId, Vec<&WantedClassConstraint>)> = unary.into_iter().collect();
    groups.sort_by_key(|(var, _)| *var);

    let mut subst = TypeSubst::empty();
    for (var, group) in groups {
        if blocked.contains(&var) || public_vars.contains(&var) {
            continue;
        }

        // Step 2.
        if !group.iter().copied().any(is_num) {
            continue;
        }

        // An embedder may run inference with no class environment at all, and
        // for that caller there is nothing to verify a candidate against. Step
        // 3 cannot run, so neither can the relaxation it pays for: only a group
        // that is entirely `Num` defaults, to `Int`, exactly as before.
        let Some(env) = class_env else {
            if group.iter().copied().all(is_num) {
                subst.insert(var, InferType::Con(TypeConstructor::Int));
            }
            continue;
        };

        // Steps 3 and 4.
        for candidate in NUMERIC_DEFAULT_CANDIDATES {
            let candidate_ty = InferType::Con(candidate);
            let discharges_group = group.iter().all(|constraint| {
                env.resolve_instance_with_subst_by_id(
                    constraint.class_id,
                    std::slice::from_ref(&candidate_ty),
                    interner,
                )
                .is_some()
            });
            if discharges_group {
                subst.insert(var, candidate_ty);
                break;
            }
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
    class_env: Option<&ClassEnv>,
    interner: &Interner,
) -> Vec<SchemeConstraint> {
    let quantified: HashSet<TypeVarId> = infer_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();

    let (_deferred, retained) = split(constraints, env_free_vars, &quantified);

    // A signature that names its context owns it. GHC's `decideQuantification`
    // calls this case (P2) in Note [Constraints in partial type signatures]:
    // "Quantify over psig_theta: the user has explicitly specified the entire
    // context. That may mean we have an unsolved residual constraint (Ix a)
    // arising from the RHS of the function. But so be it."
    //
    // Inferring the rest instead is how `fn cmp<a: MyEq>` whose body needs
    // `MyOrd<a>` came to compile: the predicate was quietly added to the
    // scheme, and the mismatch surfaced at whichever caller used a type
    // without that instance. Leaving it out of the scheme leaves it a wanted
    // inside the binding's scope, where it is reported against the context.
    //
    // A signature that names *no* bound is the other case: nothing was
    // specified, so inference still supplies the context, which is what keeps
    // `fn list_size<a>(value: List<a>)` working.
    let declared_context = mode == GeneralizationMode::Definition
        && constraints
            .iter()
            .any(|c| c.origin == WantedClassConstraintOrigin::ExplicitBound);

    let mut result: Vec<SchemeConstraint> = Vec::new();
    for constraint in retained {
        if declared_context && constraint.origin != WantedClassConstraintOrigin::ExplicitBound {
            continue;
        }
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
        if mode == GeneralizationMode::NestedBinding && constraint.origin.is_inferred_operator() {
            continue;
        }

        let candidate = SchemeConstraint {
            class_name: constraint.class_name,
            class_id: constraint.class_id,
            type_args: constraint.type_args.clone(),
        };
        // Deduplicate on the whole predicate, so two distinct structured
        // obligations over the same variable both survive.
        if !result.contains(&candidate) {
            result.push(candidate);
        }
    }

    match class_env {
        Some(class_env) => {
            let reduced = reduce_to_head_normal_form(result, class_env, interner);
            retain_minimal_by_superclasses(reduced, class_env)
        }
        None => result,
    }
}

/// Drop a retained predicate that another retained predicate already implies.
///
/// GHC's `mkMinimalBySCs`, called from `decideQuantification`
/// (`compiler/GHC/Tc/Solver.hs`). Keeping both `Monoid<a>` and its superclass
/// `Semigroup<a>` on a scheme asks every caller for two dictionaries when one
/// carries the other: every `Monoid` dictionary holds `Semigroup` evidence in a
/// superclass slot, which is where the body should project it from.
///
/// Only predicates over the *same* type arguments imply one another, so
/// `Monoid<a>` says nothing about `Semigroup<b>`. Superclass cycles are already
/// rejected as E477, so no pair can eliminate each other.
fn retain_minimal_by_superclasses(
    constraints: Vec<SchemeConstraint>,
    class_env: &ClassEnv,
) -> Vec<SchemeConstraint> {
    let implied: Vec<bool> = constraints
        .iter()
        .map(|candidate| {
            constraints.iter().any(|other| {
                other != candidate
                    && other.type_args == candidate.type_args
                    && class_env
                        .superclass_path(other.class_id, candidate.class_id)
                        .is_some()
            })
        })
        .collect();

    constraints
        .into_iter()
        .zip(implied)
        .filter_map(|(constraint, is_implied)| (!is_implied).then_some(constraint))
        .collect()
}

/// Whether every argument of `constraint` is headed by a type variable.
///
/// THIH's `inHnf`. A predicate over a bare variable is evidence the caller must
/// supply; one over a *constructed* type — `MyEq<List<a>>` — is evidence an
/// instance provides, and keeping it as a scheme constraint asks the caller for
/// a dictionary that the instance already defines.
fn constraint_is_head_normal(constraint: &SchemeConstraint) -> bool {
    fn head_is_var(ty: &InferType) -> bool {
        match ty {
            InferType::Var(_) => true,
            InferType::HktApp(head, _) => head_is_var(head),
            _ => false,
        }
    }
    constraint.type_args.iter().all(head_is_var)
}

/// Replace each predicate that an instance discharges with the context that
/// instance requires, and drop the duplicates that exposes.
///
/// THIH's `toHnfs`. `instance MyEq<a> => MyEq<List<a>>` reduces `MyEq<List<a>>`
/// to `MyEq<a>`, so an instance method calling a sibling method on its own head
/// no longer carries a second dictionary parameter for a predicate the instance
/// itself satisfies (KI-078).
///
/// A predicate no instance matches is kept unchanged: it may still be
/// discharged by a caller, and rejecting it here would report the same missing
/// instance twice.
fn reduce_to_head_normal_form(
    constraints: Vec<SchemeConstraint>,
    class_env: &ClassEnv,
    interner: &Interner,
) -> Vec<SchemeConstraint> {
    let mut result: Vec<SchemeConstraint> = Vec::new();
    for constraint in constraints {
        let original = constraint.clone();
        for reduced in reduce_one(&original, constraint, class_env, interner, 0) {
            if !result.contains(&reduced) {
                result.push(reduced);
            }
        }
    }
    result
}

/// One reduction step. `original` is the predicate reduction started from, and
/// is what an exhausted budget falls back to.
///
/// A context that grows its argument — `instance C<List<a>> => C<a>` — has no
/// head-normal form, so reduction runs to the budget. Returning the predicate
/// *as expanded at that depth* put a 64-deep nested type on the scheme, which
/// then appeared verbatim in diagnostics. The un-reduced predicate is the
/// honest answer: reduction achieved nothing, so it should change nothing.
fn reduce_one(
    original: &SchemeConstraint,
    constraint: SchemeConstraint,
    class_env: &ClassEnv,
    interner: &Interner,
    depth: usize,
) -> Vec<SchemeConstraint> {
    if depth >= crate::types::class_env::MAX_DICTIONARY_RESOLUTION_DEPTH {
        return vec![original.clone()];
    }
    if constraint_is_head_normal(&constraint) {
        return vec![constraint];
    }

    let Some((instance, subst)) = class_env.resolve_instance_with_subst_by_id(
        constraint.class_id,
        &constraint.type_args,
        interner,
    ) else {
        return vec![constraint];
    };

    let mut context = Vec::new();
    for (index, ctx) in instance.context.iter().enumerate() {
        let Some(type_args) = ctx
            .type_args
            .iter()
            .map(|arg| {
                crate::types::class_env::instantiate_instance_type_expr(arg, &subst, interner)
            })
            .collect::<Option<Vec<_>>>()
        else {
            // A context predicate that cannot be instantiated leaves the
            // original in place rather than dropping an obligation.
            return vec![constraint];
        };
        let Some(class_id) = instance
            .context_class_ids
            .get(index)
            .copied()
            .or_else(|| class_env.unique_class_id(ctx.class_name))
        else {
            return vec![constraint];
        };
        context.push(SchemeConstraint {
            class_name: ctx.class_name,
            class_id,
            type_args,
        });
    }

    context
        .into_iter()
        .flat_map(|ctx| reduce_one(original, ctx, class_env, interner, depth + 1))
        .collect()
}

/// Diagnostics for declared bounds whose variables no call can determine.
///
/// Implements the Haskell Report 4.3.4 ambiguity rule (Proposal 0179 Stage 4).
///
/// A bound is ambiguous when it constrains a variable that appears neither in
/// the binding's own type nor in the enclosing environment: instantiating the
/// signature at a call site leaves that variable free, so nothing selects an
/// instance for it.
///
/// Restricted to [`WantedClassConstraintOrigin::ExplicitBound`] on purpose.
/// The Report's rule is about *declared signatures*, and an inference-derived
/// predicate over a fresh variable is routinely refined by a later use — the
/// same reason Stage 3 records those as `Stuck` rather than reporting them.
/// GHC is candid that its own ambiguity check is a good-faith warning rather
/// than a proof of uncallability, so under-reporting here is the safe error.
fn ambiguous_bound_diagnostics(
    constraints: &[WantedClassConstraint],
    quantified: &HashSet<TypeVarId>,
    env_free_vars: &HashSet<TypeVarId>,
    interner: &Interner,
) -> Vec<Diagnostic> {
    constraints
        .iter()
        .filter(|c| c.origin == WantedClassConstraintOrigin::ExplicitBound)
        .filter_map(|constraint| {
            // Report the position rather than the variable's name: a wanted
            // constraint keeps `InferType`s, and every unresolved variable
            // renders alike, so naming one would not distinguish it.
            let position = constraint.type_args.iter().position(|arg| {
                arg.free_vars()
                    .iter()
                    .any(|var| !quantified.contains(var) && !env_free_vars.contains(var))
            })?;

            let class = interner.resolve(constraint.class_name);
            let arity = constraint.type_args.len();
            let subject = if arity == 1 {
                format!("The type argument of `{class}`")
            } else {
                format!("Type argument {} of `{class}`", position + 1)
            };
            Some(
                diagnostic_for(&AMBIGUOUS_TYPE_VARIABLE)
                    .with_span(constraint.span)
                    .with_message(format!(
                        "{subject} is not determined by this signature, so no call can select an \
                         instance for it."
                    )),
            )
        })
        .collect()
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
            class_id: crate::types::class_id::ClassId::from_local_name(class_name),
            type_args,
            span: Span::default(),
            origin,
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

    /// With no class environment there is nothing to verify a candidate
    /// against, so a group spanning more than one class does not default.
    #[test]
    fn mixed_num_and_eq_constraints_do_not_default_without_a_class_env() {
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
        class_env.register_prelude_classes(&mut interner);
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
