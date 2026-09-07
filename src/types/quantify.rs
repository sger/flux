//! The quantification decision.
//!
//! GHC's `decideQuantification` (`compiler/GHC/Tc/Solver.hs`): one function
//! that settles, together, *which type variables a binding quantifies* and
//! *which predicates it retains as its context*. The two answers are one
//! answer — a predicate is retained precisely because it mentions a variable
//! that was quantified — so deriving them apart is how they drift.
//!
//! Before proposal 0186 the quantified set was recomputed at four sites: twice
//! inside this module and once in each of `generalize` /
//! `generalize_with_constraints`, which rebuilt it from the type and the
//! environment after the fact. [`decide_quantification`] computes it once and
//! hands it out; [`Quantified::into_scheme`] builds the scheme from that exact
//! set rather than re-deriving a set that merely ought to match.

use std::collections::HashSet;

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
        class_defaulting::{apply_wanted_constraints_subst, build_numeric_default_subst},
        class_disposition::{Disposition, DispositionedConstraint, SolveOutcome, SolveScope},
        class_env::ClassEnv,
        class_solver::{solve_class_constraints_dispositioned, split},
        infer_type::InferType,
        scheme::Scheme,
        type_subst::TypeSubst,
    },
};

/// Immutable inputs to [`decide_quantification`].
#[derive(Clone, Copy)]
pub struct QuantifySpec<'a> {
    /// The binding's type as inferred, before the current substitution is
    /// applied.
    pub infer_type: &'a InferType,
    /// Variables free in the enclosing environment. These are monomorphic
    /// here — something outside may still constrain them — so they are never
    /// quantified.
    pub env_free_vars: &'a HashSet<TypeVarId>,
    /// Every predicate this binding owns, in the order the solver was handed
    /// them.
    pub constraints: &'a [WantedClassConstraint],
    pub current_subst: &'a TypeSubst,
    pub class_env: Option<&'a ClassEnv>,
    pub interner: &'a Interner,
    /// Whether the monomorphism restriction applies — see
    /// [`monomorphism_restriction`].
    pub restriction: MonoRestriction,
}

/// The single decision: what this binding quantifies, and over what context.
#[derive(Debug)]
pub struct Quantified {
    /// The binding's type after substitution and numeric defaulting.
    pub infer_type: InferType,
    /// The variables quantified, sorted. Every other answer here is derived
    /// from this set, and [`Quantified::into_scheme`] uses it verbatim.
    pub forall: Vec<TypeVarId>,
    /// The predicates retained as the binding's context.
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

impl Quantified {
    /// Build the binding's scheme from the decision, without re-deriving it.
    ///
    /// A constraint whose variables are not all quantified here is dropped: it
    /// is either already discharged against an instance or deferred to the
    /// enclosing scope, and in neither case can a caller of *this* binding be
    /// asked for it.
    pub fn into_scheme(self) -> Scheme {
        let forall_set: HashSet<TypeVarId> = self.forall.iter().copied().collect();
        let constraints = self
            .scheme_constraints
            .into_iter()
            .filter(|c| {
                c.type_args
                    .iter()
                    .flat_map(InferType::free_vars)
                    .all(|v| forall_set.contains(&v))
            })
            .collect();
        Scheme {
            forall: self.forall,
            constraints,
            infer_type: self.infer_type,
        }
    }
}

/// Decide what one binding quantifies.
///
/// Behavior:
/// - Apply the current substitution to the binding type and wanted constraints.
/// - Default truly ambiguous single-parameter `Num` variables to `Int`.
/// - Compute the quantified set **once**, from the defaulted type.
/// - Partition the predicates against that set (THIH's `split`) and reduce the
///   retained side to a minimal head-normal context.
pub fn decide_quantification(spec: QuantifySpec<'_>) -> Quantified {
    let resolved_type = spec.infer_type.apply_type_subst(spec.current_subst);
    let resolved_constraints = apply_wanted_constraints_subst(spec.constraints, spec.current_subst);
    let public_vars: HashSet<TypeVarId> = resolved_type
        .free_vars()
        .difference(spec.env_free_vars)
        .copied()
        .collect();

    let default_subst = build_numeric_default_subst(
        &resolved_constraints,
        &public_vars,
        spec.class_env,
        spec.interner,
    );
    let finalized_type = resolved_type.apply_type_subst(&default_subst);
    let finalized_constraints =
        apply_wanted_constraints_subst(&resolved_constraints, &default_subst);

    // The one derivation of the quantified set. Everything below reads it.
    let quantified: HashSet<TypeVarId> = finalized_type
        .free_vars()
        .difference(spec.env_free_vars)
        .copied()
        .collect();

    let outcome = spec
        .class_env
        .map(|env| {
            solve_class_constraints_dispositioned(
                &finalized_constraints,
                SolveScope::Binding,
                env,
                spec.interner,
            )
        })
        .unwrap_or_default();
    let mut diagnostics: Vec<Diagnostic> = outcome.diagnostics().cloned().collect();

    let scheme_constraints = collect_scheme_constraints(
        &finalized_constraints,
        &quantified,
        spec.env_free_vars,
        spec.restriction,
        spec.class_env,
        spec.interner,
    );
    let dispositions = mark_generalized(outcome, &scheme_constraints);

    // A declared bound over a variable this signature never mentions cannot be
    // discharged by any caller (Haskell Report §4.3.4).
    diagnostics.extend(ambiguous_bound_diagnostics(
        &finalized_constraints,
        &quantified,
        spec.env_free_vars,
        spec.interner,
    ));

    // Quantifier order is stable for reproducible diagnostics and tests.
    let mut forall: Vec<TypeVarId> = quantified.into_iter().collect();
    forall.sort_unstable();

    Quantified {
        infer_type: finalized_type,
        forall,
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
pub enum MonoRestriction {
    /// The binding quantifies the context it inferred; elaboration rewrites
    /// its call sites to pass the dictionaries.
    Generalize,
    /// The binding does not quantify an *inferred* context: elaboration cannot
    /// give it a dictionary parameter, so such a predicate floats to the
    /// enclosing scope instead. A written bound still quantifies — a complete
    /// signature overrides the restriction.
    Restricted,
}

/// Decide whether the monomorphism restriction applies to a binding group.
///
/// The Haskell Report's rule, by arity: a binding with parameters is a
/// function and generalizes; a nullary binding is a value whose context
/// cannot be re-elaborated per use, and is restricted. A complete signature
/// overrides both — the programmer wrote the type, so it is the type.
///
/// Taking `arity` from the *group* rather than from a member is deliberate:
/// one member's `forall` must not mention a variable another member left free.
pub fn monomorphism_restriction(arity: usize, has_complete_signature: bool) -> MonoRestriction {
    if has_complete_signature || arity > 0 {
        MonoRestriction::Generalize
    } else {
        MonoRestriction::Restricted
    }
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
    quantified: &HashSet<TypeVarId>,
    env_free_vars: &HashSet<TypeVarId>,
    mode: MonoRestriction,
    class_env: Option<&ClassEnv>,
    interner: &Interner,
) -> Vec<SchemeConstraint> {
    let (_deferred, retained) = split(constraints, env_free_vars, quantified);

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
    let declared_context = mode == MonoRestriction::Generalize
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
        if mode == MonoRestriction::Restricted
            && constraint.origin == WantedClassConstraintOrigin::InferredOperator
        {
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

    use super::{MonoRestriction, QuantifySpec, decide_quantification};
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
        let finalized = decide_quantification(QuantifySpec {
            infer_type: &bool_(),
            env_free_vars: &HashSet::new(),
            constraints: &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            current_subst: &TypeSubst::empty(),
            class_env: None,
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

        assert_eq!(finalized.infer_type, bool_());
        assert_eq!(finalized.default_subst.get(0), Some(&int()));
        assert!(finalized.scheme_constraints.is_empty());
    }

    #[test]
    fn does_not_default_num_variable_that_appears_in_public_type() {
        let mut interner = Interner::new();
        let num = interner.intern("Num");
        let finalized = decide_quantification(QuantifySpec {
            infer_type: &InferType::Fun(
                vec![var(0)],
                Box::new(var(0)),
                crate::types::infer_effect_row::InferEffectRow::closed_empty(),
            ),
            env_free_vars: &HashSet::new(),
            constraints: &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            current_subst: &TypeSubst::empty(),
            class_env: None,
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

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
        let finalized = decide_quantification(QuantifySpec {
            infer_type: &var(0),
            env_free_vars: &HashSet::new(),
            constraints: &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::ExplicitBound,
            )],
            current_subst: &TypeSubst::empty(),
            class_env: None,
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

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
        let finalized = decide_quantification(QuantifySpec {
            infer_type: &bool_(),
            env_free_vars: &HashSet::new(),
            constraints: &[
                wanted(num, vec![var(0)], WantedClassConstraintOrigin::MethodCall),
                wanted(eq, vec![var(0)], WantedClassConstraintOrigin::MethodCall),
            ],
            current_subst: &TypeSubst::empty(),
            class_env: None,
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

        assert!(finalized.default_subst.is_empty());
    }

    #[test]
    fn validates_concrete_constraints_after_defaulting() {
        let mut interner = Interner::new();
        let mut class_env = ClassEnv::new();
        class_env.register_builtins(&mut interner);
        class_env.register_prelude_classes(
            &mut crate::types::class_surface::ClassSurface::new(),
            &mut interner,
        );
        let num = interner.lookup("Num").expect("Num should be interned");
        let finalized = decide_quantification(QuantifySpec {
            infer_type: &bool_(),
            env_free_vars: &HashSet::new(),
            constraints: &[wanted(
                num,
                vec![var(0)],
                WantedClassConstraintOrigin::MethodCall,
            )],
            current_subst: &TypeSubst::empty(),
            class_env: Some(&class_env),
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

        assert!(
            finalized.diagnostics.is_empty(),
            "defaulted Num<Int> should validate cleanly, got: {:?}",
            finalized.diagnostics
        );
    }
}

#[cfg(test)]
mod quantification_decision_tests {
    use std::collections::HashSet;

    use super::{MonoRestriction, QuantifySpec, decide_quantification, monomorphism_restriction};
    use crate::{
        syntax::interner::Interner,
        types::{
            infer_effect_row::InferEffectRow, infer_type::InferType,
            type_constructor::TypeConstructor, type_subst::TypeSubst,
        },
    };

    /// The Report's rule, by arity. A written signature wins over both.
    #[test]
    fn restriction_follows_arity_unless_a_signature_says_otherwise() {
        assert_eq!(
            monomorphism_restriction(1, false),
            MonoRestriction::Generalize
        );
        assert_eq!(
            monomorphism_restriction(0, false),
            MonoRestriction::Restricted
        );
        assert_eq!(
            monomorphism_restriction(0, true),
            MonoRestriction::Generalize
        );
    }

    /// The scheme is built from the set the decision produced, not from a set
    /// re-derived afterwards. Before 0186 the quantified set was recomputed at
    /// four sites; this pins the two that used to disagree together.
    #[test]
    fn scheme_quantifies_exactly_the_decided_variables() {
        let interner = Interner::new();
        // `(a, Int) -> b`, with `a` free in the environment.
        let ty = InferType::Fun(
            vec![InferType::Var(0), InferType::Con(TypeConstructor::Int)],
            Box::new(InferType::Var(1)),
            InferEffectRow::closed_empty(),
        );
        let env_free: HashSet<u32> = [0].into_iter().collect();

        let decided = decide_quantification(QuantifySpec {
            infer_type: &ty,
            env_free_vars: &env_free,
            constraints: &[],
            current_subst: &TypeSubst::empty(),
            class_env: None,
            interner: &interner,
            restriction: MonoRestriction::Generalize,
        });

        assert_eq!(decided.forall, vec![1]);
        let scheme = decided.into_scheme();
        assert_eq!(scheme.forall, vec![1]);
        assert_eq!(scheme.infer_type, ty);
    }
}
