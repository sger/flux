//! Type class constraint solver — resolves collected constraints against
//! known instances (Proposal 0145, Step 4).
//!
//! After HM inference generates `WantedClassConstraint` entries, this pass
//! checks each constraint:
//! - Concrete type (e.g., `Num<Int>`) → look up instance in ClassEnv
//! - Type variable (e.g., `Num<a>`) → leave unsolved (future: add to scheme)
//!
//! Unsatisfied concrete constraints produce compile errors.

use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::{
        SchemeConstraint, WantedClassConstraint, WantedClassConstraintOrigin, WantedConstraints,
    },
    diagnostics::position::Span,
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for},
    syntax::{Identifier, interner::Interner, type_expr::TypeExpr},
    types::{
        TypeVarId,
        class_disposition::{
            Disposition, DispositionedConstraint, Evidence, InstanceKey, SolveOutcome, SolveScope,
            StuckReason,
        },
        class_env::{ClassEnv, MAX_DICTIONARY_RESOLUTION_DEPTH},
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

use super::super::diagnostics::compiler_errors::{
    COULD_NOT_DEDUCE, INSTANCE_SEARCH_EXHAUSTED, NO_INSTANCE, OVERLAPPING_INSTANCES,
    UNDETERMINED_CLASS_PARAMETER,
};

/// Solve class constraints against known instances.
///
/// For each constraint where the type argument is concrete, verifies that
/// a matching instance exists in the ClassEnv. Emits diagnostics for
/// unsatisfied constraints.
///
/// Returns a list of error diagnostics (empty if all constraints are satisfied).
///
/// This is a thin projection of [`solve_class_constraints_dispositioned`],
/// which assigns every predicate an explicit outcome. Prefer that function
/// where the outcome — not just the errors — is needed.
pub fn solve_class_constraints(
    constraints: &[WantedClassConstraint],
    class_env: &ClassEnv,
    interner: &Interner,
) -> Vec<Diagnostic> {
    solve_class_constraints_dispositioned(
        constraints,
        SolveScope::WholeProgram,
        class_env,
        interner,
    )
    .into_diagnostics()
    .collect()
}

/// Solve a whole wanted tree and project it to diagnostics.
///
/// The tree analogue of [`solve_class_constraints`], for callers that hold an
/// [`InferProgramResult`]'s wanted set and only want the errors.
///
/// [`InferProgramResult`]: crate::ast::type_infer::InferProgramResult
pub fn solve_class_constraint_tree(
    wanted: &WantedConstraints,
    class_env: &ClassEnv,
    interner: &Interner,
) -> Vec<Diagnostic> {
    solve_wanted_tree(wanted, SolveScope::WholeProgram, class_env, interner)
        .into_diagnostics()
        .collect()
}

/// Solve a whole wanted tree, descending into each scope with its context.
///
/// GHC's `solveWanteds` walks `wc_simple` and then `wc_impl`, and inside an
/// implication the `ic_given` predicates are available to discharge wanteds
/// (`compiler/GHC/Tc/Solver/Solve.hs`). That is the step a flat list cannot
/// express: without it a predicate over a rigid type variable can only be
/// recorded as undecided, because no *instance* will ever match a variable.
///
/// Givens accumulate outward-in, so a nested scope may use the context of
/// every scope enclosing it.
pub fn solve_wanted_tree(
    wanted: &WantedConstraints,
    scope: SolveScope,
    class_env: &ClassEnv,
    interner: &Interner,
) -> SolveOutcome {
    let mut dispositions = Vec::new();
    solve_scope(
        wanted,
        &Enclosing::default(),
        scope,
        class_env,
        interner,
        &mut dispositions,
    );
    SolveOutcome { dispositions }
}

fn solve_scope(
    wanted: &WantedConstraints,
    enclosing: &Enclosing<'_>,
    scope: SolveScope,
    class_env: &ClassEnv,
    interner: &Interner,
    out: &mut Vec<DispositionedConstraint>,
) {
    for constraint in &wanted.simple {
        out.push(DispositionedConstraint {
            wanted: constraint.clone(),
            disposition: classify_constraint(constraint, enclosing, scope, class_env, interner),
        });
    }
    for implication in &wanted.implications {
        let mut givens = enclosing.givens.to_vec();
        givens.extend(implication.givens.iter().cloned());
        let nested = Enclosing {
            givens: &givens,
            quantified: &implication.quantified,
        };
        solve_scope(
            &implication.wanted,
            &nested,
            scope,
            class_env,
            interner,
            out,
        );
    }
}

/// What the scope a predicate was raised in offers, and what it binds.
///
/// GHC's `Implication` carries `ic_given` and `ic_skols` together for the same
/// reason: the givens say what may discharge a predicate, and the skolems say
/// whether anything else ever could.
#[derive(Debug, Clone, Copy, Default)]
struct Enclosing<'a> {
    /// Context available for discharge, accumulated outward-in.
    givens: &'a [SchemeConstraint],
    /// Type variables the enclosing binding quantified.
    quantified: &'a [crate::types::TypeVarId],
}

/// Render a predicate with the enclosing scope's variables named.
fn display_predicate_named(
    class_name: Identifier,
    type_args: &[InferType],
    names: &HashMap<TypeVarId, String>,
    interner: &Interner,
) -> String {
    let args = type_args
        .iter()
        .map(|ty| display_type_named(ty, names, interner))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}<{args}>", interner.resolve(class_name))
}

/// Render a type, substituting the canonical name of any variable in `names`.
fn display_type_named(
    ty: &InferType,
    names: &HashMap<TypeVarId, String>,
    interner: &Interner,
) -> String {
    match ty {
        InferType::Var(var) => names
            .get(var)
            .cloned()
            .unwrap_or_else(|| display_type(ty, interner)),
        InferType::App(constructor, args) if !args.is_empty() => format!(
            "{}<{}>",
            display_type(&InferType::App(constructor.clone(), Vec::new()), interner)
                .trim_end_matches("<>"),
            args.iter()
                .map(|arg| display_type_named(arg, names, interner))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => display_type(ty, interner),
    }
}

/// The canonical display name for the n-th quantified variable: `a`, `b`, ….
fn alpha_name(index: usize) -> String {
    let letter = ((index % 26) as u8 + b'a') as char;
    let suffix = index / 26;
    if suffix == 0 {
        letter.to_string()
    } else {
        format!("{letter}{suffix}")
    }
}

/// Report a predicate the enclosing signature does not promise.
///
/// Names the context so the reader can see what *was* promised; an empty one is
/// spelled out rather than rendered as `` so the message stays readable.
fn could_not_deduce(
    constraint: &WantedClassConstraint,
    enclosing: &Enclosing<'_>,
    interner: &Interner,
) -> Disposition {
    // The binding's own variables get their canonical `a`, `b`, … names, in
    // quantification order. Rendered raw they all print as `_`, so
    // "Could not deduce `MyOrd<_>` from the context `MyEq<_>`" would not even
    // say whether the two mention the same variable.
    let names: HashMap<TypeVarId, String> = enclosing
        .quantified
        .iter()
        .enumerate()
        .map(|(index, var)| (*var, alpha_name(index)))
        .collect();

    let wanted = display_predicate_named(
        constraint.class_name,
        &constraint.type_args,
        &names,
        interner,
    );
    let context = if enclosing.givens.is_empty() {
        "no constraints".to_string()
    } else {
        enclosing
            .givens
            .iter()
            .map(|given| {
                display_predicate_named(given.class_name, &given.type_args, &names, interner)
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    Disposition::Diagnosed {
        diagnostic: Box::new(
            diagnostic_for(&COULD_NOT_DEDUCE)
                .with_span(constraint.span)
                .with_message(format!(
                    "Could not deduce `{wanted}` from the context `{context}`."
                ))
                .with_hint_text(format!(
                    "Add `{}` to the bounds this signature declares.",
                    interner.resolve(constraint.class_name)
                )),
        ),
    }
}

/// Whether the context this scope holds already discharges `constraint`.
///
/// THIH's `bySuper`: a predicate is entailed by a given when it *is* that
/// given, or when the given's class has it among its transitive superclasses —
/// `Ord<a>` in scope discharges `Eq<a>`, since every `Ord` dictionary carries
/// an `Eq` one in a superclass slot.
///
/// Matching is syntactic on the type arguments, which is what THIH does: a
/// given is a promise about exactly the type it names.
fn entailed_by_givens(
    class_id: crate::types::class_id::ClassId,
    type_args: &[InferType],
    givens: &[SchemeConstraint],
    class_env: &ClassEnv,
) -> Option<Evidence> {
    givens
        .iter()
        .filter(|given| given.type_args == type_args)
        .find_map(|given| {
            let path = if given.class_id == class_id {
                Vec::new()
            } else {
                class_env.superclass_path(given.class_id, class_id)?
            };
            Some(Evidence::FromGiven {
                given: given.clone(),
                superclass_path: path,
            })
        })
}

/// Solve class constraints, assigning every predicate a [`Disposition`].
///
/// Proposal 0179 Stage 3, Goal 2: every obligation is solved, generalized,
/// recorded as a documented stuck predicate, or reported as an error — never
/// silently discarded. The returned outcome therefore always holds exactly
/// one entry per supplied constraint.
///
/// `scope` distinguishes the per-binding pass from the final whole-program
/// pass. Both currently classify predicates identically; the distinction
/// exists so that generalization and ambiguity can diverge as the remaining
/// Stage 3 steps land.
pub fn solve_class_constraints_dispositioned(
    constraints: &[WantedClassConstraint],
    scope: SolveScope,
    class_env: &ClassEnv,
    interner: &Interner,
) -> SolveOutcome {
    let dispositions: Vec<DispositionedConstraint> = constraints
        .iter()
        .map(|constraint| DispositionedConstraint {
            wanted: constraint.clone(),
            disposition: classify_constraint(
                constraint,
                &Enclosing::default(),
                scope,
                class_env,
                interner,
            ),
        })
        .collect();

    debug_assert_eq!(
        dispositions.len(),
        constraints.len(),
        "every wanted predicate must receive exactly one disposition"
    );

    SolveOutcome { dispositions }
}

/// Assign a single wanted predicate its outcome.
///
/// Each arm below replaces what was previously a bare `continue`, so the
/// reason a predicate went undecided is now recorded rather than lost.
fn classify_constraint(
    constraint: &WantedClassConstraint,
    enclosing: &Enclosing<'_>,
    scope: SolveScope,
    class_env: &ClassEnv,
    interner: &Interner,
) -> Disposition {
    // The context comes first: a predicate the enclosing signature already
    // promises is discharged whatever its shape, and asking about instances or
    // concreteness first would misfile it. `Num<a>` inside `fn f<a: Num>` is
    // the case that matters — no instance matches a rigid variable, so before
    // this it could only be recorded as undecided.
    if let Some(evidence) = entailed_by_givens(
        constraint.class_id,
        &constraint.type_args,
        enclosing.givens,
        class_env,
    ) {
        return Disposition::Solved { evidence };
    }

    // `+` is overloaded over `Num` *and* `String`, and the string half is a
    // built-in rule rather than an instance: `"a" + "b"` lowers to a primitive
    // append and needs no dictionary. `infer_add_operator` applies that rule
    // itself when the operand type is known at emission; here it is applied to
    // the operand that was still a variable then and resolved to `String`
    // since — GHC's M4, decide on the type as it is now.
    if constraint.origin == WantedClassConstraintOrigin::InferredAddOperator
        && matches!(
            constraint.type_args.as_slice(),
            [InferType::Con(TypeConstructor::String)]
        )
    {
        return Disposition::Solved {
            evidence: Evidence::Structural { components: vec![] },
        };
    }

    // A class-method call whose predicate still has an undetermined slot can
    // only be dispatched if the slots it *did* fix single out one instance.
    // When several remain compatible, the call has no way to choose, and
    // committing to one would make the program depend on declaration order.
    //
    // This is GHC's IL5/IL6 (`Note [Rules for instance lookup]`): a wanted
    // that merely *unifies* with several instance heads is not committed to.
    // Reporting it here turns what used to be a runtime `E1009` panic, raised
    // with a synthetic line-0 span, into a compile error at the call.
    if scope == SolveScope::WholeProgram
        && constraint.origin == WantedClassConstraintOrigin::MethodCall
        && constraint.span != Span::default()
        && let Some(diagnostic) = undetermined_parameter_diagnostic(constraint, class_env, interner)
    {
        return Disposition::Diagnosed {
            diagnostic: Box::new(diagnostic),
        };
    }

    // A predicate whose variables the enclosing binding all *quantified* has
    // one chance left: an instance may still match it — THIH's `byInst` does
    // not require ground arguments, which is what lets an instance method call
    // a sibling on its own head. So it skips the "not concrete, give up" exit
    // below and goes to the instance search, and is reported only if that also
    // finds nothing.
    //
    // Only when *every* variable involved is quantified here. One that is not
    // is a variable inference never resolved, which is ambiguity — a different
    // fact, reported elsewhere.
    //
    // A predicate the signature *declares* is excluded: `E489` is about a body
    // asking for more than its signature grants, and a declared bound is the
    // signature. Whether a written bound is itself satisfiable is a separate
    // question — an instance context like `Semigroup<Option<a>> =>` is
    // discharged by the instance the context names, not by the scope it sits
    // in — and answering it here would report the declaration as if the body
    // had overreached.
    let over_quantified = !enclosing.quantified.is_empty()
        && constraint.origin != WantedClassConstraintOrigin::ExplicitBound
        && constraint
            .type_args
            .iter()
            .flat_map(InferType::free_vars)
            .all(|var| enclosing.quantified.contains(&var));

    // Only check concrete types by default — variables are left unsolved
    // for now. Function-shaped type arguments are already specific enough
    // to reject for marker classes like `Sendable`, even if their inner
    // parameter/return slots still contain variables.
    if !over_quantified && !constraint.type_args.iter().all(is_solvable_type_arg) {
        return Disposition::Stuck {
            reason: match scope {
                // A binding may still quantify this variable, in which case
                // `mark_generalized` upgrades the disposition.
                SolveScope::Binding => StuckReason::OuterScopeVariable,
                // Generalization has already had its chance, so a residual
                // variable here is unresolved rather than merely deferred.
                SolveScope::WholeProgram => StuckReason::UnresolvedAfterGeneralization,
            },
        };
    }

    // Skip constraints from compiler-generated code (e.g., dispatch functions).
    // These have default spans (line 0, col 0).
    if constraint.span == Span::default() {
        return Disposition::Stuck {
            reason: StuckReason::SyntheticOrigin,
        };
    }

    // Skip if the class doesn't exist in the environment (already
    // reported by ClassEnv validation as E441).
    if class_env.lookup_class_by_id(constraint.class_id).is_none() {
        return Disposition::Stuck {
            reason: StuckReason::UnknownClass,
        };
    }

    let mut search = InstanceSearch {
        givens: enclosing.givens,
        ..InstanceSearch::default()
    };
    let evidence = solve_instance_evidence(
        constraint.class_id,
        &constraint.type_args,
        class_env,
        interner,
        &mut search,
    );

    // Running out of budget is a different fact from finding no instance, and
    // GHC keeps them apart for the same reason: `simplify_loop`'s `check_limit`
    // raises its own error rather than letting the unsolved constraint speak
    // for it (`compiler/GHC/Tc/Solver/Solve.hs`). Reported as a missing
    // instance, an abandoned search sends the reader looking for something that
    // may already exist.
    if search.exhausted && evidence.is_none() {
        let predicate = display_predicate(constraint, interner);
        return Disposition::Diagnosed {
            diagnostic: Box::new(
                diagnostic_for(&INSTANCE_SEARCH_EXHAUSTED)
                    .with_span(constraint.span)
                    .with_message(format!(
                        "Resolving `{predicate}` exceeded the instance-context depth limit \
                         ({MAX_DICTIONARY_RESOLUTION_DEPTH})."
                    )),
            ),
        };
    }

    // Two or more instances matching the same predicate would make evidence
    // selection depend on declaration order. Report it rather than silently
    // taking the first (Proposal 0179 Stage 3).
    let candidates = class_env
        .candidate_instances_by_id(constraint.class_id, &constraint.type_args, interner)
        .count();
    if candidates > 1 {
        let predicate = display_predicate(constraint, interner);
        return Disposition::Diagnosed {
            diagnostic: Box::new(
                diagnostic_for(&OVERLAPPING_INSTANCES)
                    .with_span(constraint.span)
                    .with_message(format!(
                        "Multiple instances match `{predicate}`; instance selection would depend on declaration order."
                    )),
            ),
        };
    }

    if let Some(evidence) = evidence {
        return Disposition::Solved { evidence };
    }

    // Nothing discharged it, and nothing ever will: the binding has already
    // generalized, so no later scope can supply evidence for a variable it
    // quantified. GHC reports this from inside the implication as
    // "Could not deduce (C a) from the context ...".
    if over_quantified && !constraint.type_args.iter().all(is_solvable_type_arg) {
        return could_not_deduce(constraint, enclosing, interner);
    }

    let type_display = display_type_args(&constraint.type_args, interner);
    let class_display = interner.resolve(constraint.class_name);

    if let WantedClassConstraintOrigin::TaskSpawnCapture { capture_name } = constraint.origin {
        let capture_display = interner.resolve(capture_name);
        return Disposition::Diagnosed {
            diagnostic: Box::new(
                diagnostic_for(&NO_INSTANCE)
                    .with_span(constraint.span)
                    .with_message(format!(
                        "Task.spawn closure captures non-Sendable value `{capture_display}: {type_display}`."
                    ))
                    .with_hint_text(
                        "Only values with a Sendable instance can cross the task worker boundary."
                            .to_string(),
                    ),
            ),
        };
    }

    Disposition::Diagnosed {
        diagnostic: Box::new(
            diagnostic_for(&NO_INSTANCE)
                .with_span(constraint.span)
                .with_message(format!(
                    "No instance for `{class_display}<{type_display}>`."
                ))
                .with_hint_text(format!(
                    "Add an instance: `instance {class_display}<{type_display}> {{ ... }}`"
                )),
        ),
    }
}

/// Solve a predicate against the class environment, recording *how*.
///
/// Returns the evidence that discharges it, or `None` when nothing does. This
/// is THIH's `byInst` extended to carry its own result: the instance and
/// substitution were already computed here before Stage 4 and then thrown
/// away, which is why dictionary elaboration had to resolve a second time and
/// could disagree with the solver.
/// The state of one instance search: the path being explored, and whether the
/// depth budget was ever hit while exploring it.
#[derive(Debug, Default)]
struct InstanceSearch<'a> {
    /// The context every subgoal may be discharged against.
    givens: &'a [SchemeConstraint],
    /// Predicates on the current path, for cycle detection.
    seen: HashSet<String>,
    /// Set when the budget cut a branch short, so the caller can say the
    /// search was abandoned rather than that no instance exists.
    exhausted: bool,
}

fn solve_instance_evidence(
    class_id: crate::types::class_id::ClassId,
    type_args: &[InferType],
    class_env: &ClassEnv,
    interner: &Interner,
    search: &mut InstanceSearch<'_>,
) -> Option<Evidence> {
    // THIH's `entail` in full: a predicate holds if the context proves it, or
    // if an instance matches and the context proves *its* subgoals —
    // `entail ps p = any (elem p . bySuper ce) ps || case byInst ce p of
    // Just qs -> all (entail ps) qs`. Checking the givens only at the top
    // would leave an instance context like `Eq<a> => Eq<List<a>>` unable to
    // discharge the `Eq<a>` its own methods rest on.
    if let Some(evidence) = entailed_by_givens(class_id, type_args, search.givens, class_env) {
        return Some(evidence);
    }

    let class_module = class_id
        .module
        .as_identifier()
        .map(|module| interner.resolve(module))
        .unwrap_or("<prelude>");
    let key = format!(
        "{class_module}.{}<{}>",
        interner.resolve(class_id.name),
        type_args
            .iter()
            .map(|ty| display_type(ty, interner))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !search.seen.insert(key.clone()) {
        // A cycle in the instance-context graph. The predicate is assumed
        // satisfied to keep the search terminating, but there is no finite
        // evidence tree to hand back.
        return Some(Evidence::Unrecorded);
    }

    // `seen` is the current path, so its size is the search depth. A context
    // that grows its argument at every step — `instance Foo<List<a>> => Foo<a>`
    // — never repeats a key, so the check above never fires and only this
    // budget stops the recursion. Without it the search overflowed the
    // compiler's stack. Reporting no evidence turns that into the same
    // diagnostic a missing instance produces.
    if search.seen.len() > MAX_DICTIONARY_RESOLUTION_DEPTH {
        search.seen.remove(&key);
        search.exhausted = true;
        return None;
    }

    // A real instance is tried before the structural rule, so that a predicate
    // an `InstanceDef` can discharge yields evidence naming it — and therefore
    // a dictionary — rather than a solver-only answer. Proposal 0179 Stage 7
    // registers contextual `Eq` instances over the built-in containers for
    // exactly this reason; the structural rule stays as the fallback for the
    // heads that have none (tuples, `Either`, `Array`, and every `Sendable`
    // case, which is a marker class with no dictionary at all).
    //
    // The instance search is a closure so that its `?` and early `return`
    // leave *it* rather than this function — falling through to the structural
    // rule below, and still reaching the `seen.remove` at the end.
    let from_instance = (|| {
        let (instance, subst) =
            class_env.resolve_instance_with_subst_by_id(class_id, type_args, interner)?;

        // Every predicate in the instance's context must itself be
        // discharged, and its evidence becomes a subgoal of this one.
        let context = instance
            .context
            .iter()
            .enumerate()
            .map(|(index, ctx)| {
                let args: Vec<InferType> = ctx
                    .type_args
                    .iter()
                    .map(|arg| instantiate_context_type_expr(arg, &subst, interner))
                    .collect::<Option<_>>()?;
                let context_id = instance
                    .context_class_ids
                    .get(index)
                    .copied()
                    .or_else(|| class_env.unique_class_id(ctx.class_name))?;
                // A subgoal over a type variable used to end the search: with
                // no context to appeal to, nothing could ever discharge it.
                // A scope's givens are exactly that context — THIH's `entail`
                // recurses on subgoals whether or not they are ground — so the
                // guard now only stops a subgoal no given can reach.
                // `instance Dec<a> => Dec<Array<a>>` used from inside
                // `instance Dec<a> => Dec<List<a>>` is the case: the subgoal is
                // `Dec<a>`, which is precisely what the enclosing instance
                // context grants.
                if !args.iter().all(is_concrete_type)
                    && entailed_by_givens(context_id, &args, search.givens, class_env).is_none()
                {
                    return None;
                }
                solve_instance_evidence(context_id, &args, class_env, interner, search)
            })
            .collect::<Option<Vec<_>>>()?;

        // A class with no methods has no dictionary to pass, so naming the
        // instance would imply a runtime value that does not exist.
        if class_env
            .lookup_class_by_id(class_id)
            .is_some_and(|class| class.methods.is_empty())
        {
            return Some(Evidence::Marker);
        }

        Some(Evidence::FromInstance {
            instance: InstanceKey {
                class_id: instance.class_id,
                head_type_args: type_args.to_vec(),
            },
            subst,
            context,
        })
    })();
    let evidence = from_instance.or_else(|| {
        structural_builtin_evidence(class_id, type_args, class_env, interner, search)
            .map(|components| Evidence::Structural { components })
    });

    search.seen.remove(&key);
    evidence
}

/// Evidence for each component a built-in structural rule decomposes into,
/// or `None` when no structural rule applies.
fn structural_builtin_evidence(
    class_id: crate::types::class_id::ClassId,
    type_args: &[InferType],
    class_env: &ClassEnv,
    interner: &Interner,
    search: &mut InstanceSearch<'_>,
) -> Option<Vec<Evidence>> {
    let class_name = interner.resolve(class_id.name);
    if !class_env
        .lookup_class_by_id(class_id)
        .is_some_and(|class| class.is_builtin)
    {
        return None;
    }
    if !matches!(class_name, "Eq" | "Ord" | "Sendable") || type_args.len() != 1 {
        return None;
    }

    let components = |args: &[InferType], search: &mut InstanceSearch<'_>| {
        args.iter()
            .map(|arg| single_evidence(class_name, arg, class_env, interner, search))
            .collect::<Option<Vec<_>>>()
    };

    match &type_args[0] {
        InferType::Tuple(elements) => components(elements, search),
        InferType::App(TypeConstructor::Option, args)
        | InferType::App(TypeConstructor::List, args)
        | InferType::App(TypeConstructor::Array, args) => {
            let arg = args.first()?;
            single_evidence(class_name, arg, class_env, interner, search).map(|ev| vec![ev])
        }
        // `Sendable<Map<k, v>>` requires both the keys and values to be
        // sendable. `Eq` and `Ord` are not currently auto-derived for `Map`
        // (the existing rules only cover `Option`/`List`/`Array`), so this
        // arm only fires for `Sendable`.
        InferType::App(TypeConstructor::Map, args) if class_name == "Sendable" => {
            components(args, search)
        }
        InferType::App(TypeConstructor::Either, args) => components(args, search),
        _ => None,
    }
}

fn single_evidence(
    class_name: &str,
    ty: &InferType,
    class_env: &ClassEnv,
    interner: &Interner,
    search: &mut InstanceSearch<'_>,
) -> Option<Evidence> {
    let class_id = class_env
        .classes
        .values()
        .find(|class| class.is_builtin && interner.resolve(class.name) == class_name)
        .map(|class| class.class_id())?;
    solve_instance_evidence(
        class_id,
        std::slice::from_ref(ty),
        class_env,
        interner,
        search,
    )
}

fn instantiate_context_type_expr(
    expr: &TypeExpr,
    subst: &HashMap<Identifier, InferType>,
    interner: &Interner,
) -> Option<InferType> {
    match expr {
        TypeExpr::Named { name, args, .. } => {
            if args.is_empty()
                && interner
                    .resolve(*name)
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase())
            {
                return subst.get(name).cloned();
            }

            let arg_tys: Option<Vec<InferType>> = args
                .iter()
                .map(|arg| instantiate_context_type_expr(arg, subst, interner))
                .collect();
            let arg_tys = arg_tys?;
            let tc = match interner.resolve(*name) {
                "Int" => TypeConstructor::Int,
                "Float" => TypeConstructor::Float,
                "Bool" => TypeConstructor::Bool,
                "String" => TypeConstructor::String,
                "Unit" | "None" => TypeConstructor::Unit,
                "Never" => TypeConstructor::Never,
                "List" => TypeConstructor::List,
                "Array" => TypeConstructor::Array,
                "Map" => TypeConstructor::Map,
                "Option" => TypeConstructor::Option,
                "Either" => TypeConstructor::Either,
                _ => TypeConstructor::Adt(*name),
            };

            if arg_tys.is_empty() {
                Some(InferType::Con(tc))
            } else {
                Some(InferType::App(tc, arg_tys))
            }
        }
        TypeExpr::Tuple { elements, .. } => Some(InferType::Tuple(
            elements
                .iter()
                .map(|elem| instantiate_context_type_expr(elem, subst, interner))
                .collect::<Option<Vec<_>>>()?,
        )),
        TypeExpr::Function { params, ret, .. } => Some(InferType::Fun(
            params
                .iter()
                .map(|param| instantiate_context_type_expr(param, subst, interner))
                .collect::<Option<Vec<_>>>()?,
            Box::new(instantiate_context_type_expr(ret, subst, interner)?),
            crate::types::infer_effect_row::InferEffectRow::closed_empty(),
        )),
    }
}

/// Check if a type is concrete (not a variable).
fn is_concrete_type(ty: &InferType) -> bool {
    match ty {
        InferType::Var(_) => false,
        InferType::Con(_) => true,
        InferType::App(_, args) => args.iter().all(is_concrete_type),
        InferType::Tuple(elems) => elems.iter().all(is_concrete_type),
        InferType::Fun(params, ret, _) => {
            params.iter().all(is_concrete_type) && is_concrete_type(ret)
        }
        InferType::HktApp(head, args) => {
            is_concrete_type(head) && args.iter().all(is_concrete_type)
        }
        // Not concrete even when every argument is: the type this reduces to
        // is not known until an instance is selected. Answering `true` would
        // let a predicate be solved against an instance chosen for the
        // unreduced application rather than for its result.
        InferType::Assoc(_, _, _) => false,
    }
}

fn is_solvable_type_arg(ty: &InferType) -> bool {
    is_concrete_type(ty) || matches!(ty, InferType::Fun(_, _, _))
}

/// Render a predicate's type arguments as a comma-separated list.
fn display_type_args(type_args: &[InferType], interner: &Interner) -> String {
    type_args
        .iter()
        .map(|ty| display_type(ty, interner))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Render a whole predicate, e.g. `Eq<List<Int>>`.
/// The E459 diagnostic for a predicate with an undetermined class parameter,
/// or `None` when the call can still select an instance.
///
/// Returns `None` when every slot is known (nothing is undetermined), or when
/// the known slots leave at most one compatible instance — a single candidate
/// supplies the missing type from its own head, which is what lets
/// `let s = convert(42)` resolve without an annotation.
fn undetermined_parameter_diagnostic(
    constraint: &WantedClassConstraint,
    class_env: &ClassEnv,
    interner: &Interner,
) -> Option<Diagnostic> {
    let known: Vec<Option<InferType>> = constraint
        .type_args
        .iter()
        .map(|ty| match ty {
            InferType::Var(_) => None,
            other => Some(other.clone()),
        })
        .collect();

    let first_undetermined = known.iter().position(Option::is_none)?;

    // Require that the call fixed at least one slot. A predicate with *every*
    // slot open is the dictionary-passing case — `enc(h)` inside
    // `instance Enc<a> => Enc<List<a>>` constrains a variable the enclosing
    // scheme quantifies, and receives its evidence as a dictionary parameter.
    // Flux spells a rigid scheme-bound variable and a free metavariable both
    // as `InferType::Var`, and whole-program scope does not carry the
    // environment's free-variable set that would tell them apart, so this
    // stays deliberately conservative: a missed case falls through to the
    // previous behaviour, whereas a false positive rejects a working program.
    // The general check, with `env_free_vars` in scope, is the ambiguity work
    // that follows.
    if known.iter().all(Option::is_none) {
        return None;
    }

    let candidates = class_env
        .instances_matching_known_args_by_id(constraint.class_id, &known, interner)
        .take(2)
        .count();
    if candidates < 2 {
        return None;
    }

    let class_def = class_env.lookup_class_by_id(constraint.class_id)?;
    let param = class_def
        .type_params
        .get(first_undetermined)
        .map(|name| interner.resolve(*name).to_string())
        .unwrap_or_else(|| format!("argument {}", first_undetermined + 1));
    let predicate = display_predicate(constraint, interner);

    Some(
        diagnostic_for(&UNDETERMINED_CLASS_PARAMETER)
            .with_span(constraint.span)
            .with_message(format!(
                "Cannot determine `{param}` in `{predicate}`; \
                 several instances are compatible with what this call fixes."
            )),
    )
}

fn display_predicate(constraint: &WantedClassConstraint, interner: &Interner) -> String {
    format!(
        "{}<{}>",
        interner.resolve(constraint.class_name),
        display_type_args(&constraint.type_args, interner)
    )
}

/// Format a type for display in diagnostics.
fn display_type(ty: &InferType, interner: &Interner) -> String {
    crate::ast::type_infer::display_infer_type(ty, interner)
}

/// Whether a predicate is in head-normal form.
///
/// THIH's `inHnf`: a predicate is in head-normal form when its arguments are
/// headed by a type *variable* rather than a concrete constructor. `C<a>` is
/// in HNF and must be retained on the scheme; `C<List<Int>>` is not and can be
/// reduced against an instance.
///
/// A predicate that is not in HNF and cannot be reduced is the genuine
/// missing-instance case.
pub fn in_hnf(type_args: &[InferType]) -> bool {
    type_args.iter().any(is_var_headed)
}

/// Whether a type's outermost constructor is a variable.
fn is_var_headed(ty: &InferType) -> bool {
    match ty {
        InferType::Var(_) => true,
        // A structured argument mentioning a variable is not itself
        // var-headed, but it is still polymorphic, so it cannot be discharged
        // by a concrete instance lookup alone.
        InferType::App(_, args) | InferType::Tuple(args) => args.iter().any(is_var_headed),
        InferType::Fun(params, ret, _) => params.iter().any(is_var_headed) || is_var_headed(ret),
        InferType::HktApp(head, args) => is_var_headed(head) || args.iter().any(is_var_headed),
        _ => false,
    }
}

/// Partition predicates into those deferred to an enclosing scope and those
/// this binding retains on its scheme.
///
/// THIH's `split ce fs gs ps`, where `fs` is the set of variables fixed by the
/// environment and `gs` the variables this binding quantifies. The essential
/// property is that this is a *partition*: every predicate lands in exactly
/// one half, so no obligation can be discarded (Proposal 0179, Goal 2).
///
/// Returns `(deferred, retained)`.
pub fn split(
    constraints: &[WantedClassConstraint],
    env_free_vars: &HashSet<TypeVarId>,
    quantified_vars: &HashSet<TypeVarId>,
) -> (Vec<WantedClassConstraint>, Vec<WantedClassConstraint>) {
    constraints.iter().cloned().partition(|constraint| {
        // A predicate whose variables are all fixed by the environment
        // belongs to an enclosing binding, not this one.
        let vars: HashSet<TypeVarId> = constraint
            .type_args
            .iter()
            .flat_map(InferType::free_vars)
            .collect();
        !vars.is_empty()
            && vars
                .iter()
                .all(|var| env_free_vars.contains(var) && !quantified_vars.contains(var))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::position::Position;
    use crate::syntax::type_expr::TypeExpr;
    use crate::types::class_env::{ClassDef, InstanceDef, MethodSig};
    use crate::types::class_id::{ClassId, ModulePath};

    fn named(interner: &mut Interner, name: &str, args: Vec<TypeExpr>) -> TypeExpr {
        TypeExpr::Named {
            name: interner.intern(name),
            args,
            span: Span::default(),
        }
    }

    /// `class Sizeable<a> { fn size(x: a) -> Int }` plus the given instance
    /// heads, each context-free.
    fn env_with_instances(interner: &mut Interner, heads: &[TypeExpr]) -> ClassEnv {
        let class_name = interner.intern("Sizeable");
        let method_name = interner.intern("size");
        let int = named(interner, "Int", vec![]);
        let a = named(interner, "a", vec![]);
        let class_id = ClassId::new(ModulePath::EMPTY, class_name);

        let mut env = ClassEnv::default();
        env.classes.insert(
            class_id,
            ClassDef {
                name: class_name,
                module: ModulePath::EMPTY,
                is_public: false,
                is_builtin: false,
                type_params: vec![interner.intern("a")],
                superclasses: Vec::new(),
                superclass_class_ids: Vec::new(),
                associated_types: Vec::new(),
                methods: vec![MethodSig {
                    name: method_name,
                    type_params: Vec::new(),
                    param_names: Vec::new(),
                    param_types: vec![a],
                    return_type: int,
                    arity: 1,
                    effects: Vec::new(),
                    default_body: None,
                }],
                default_methods: Vec::new(),
                span: Span::default(),
            },
        );
        for head in heads {
            env.instances.push(InstanceDef {
                origin: crate::types::class_env::InstanceOrigin::Declared,
                class_name,
                class_id,
                instance_module: ModulePath::EMPTY,
                is_public: false,
                type_args: vec![head.clone()],
                context: Vec::new(),
                context_class_ids: Vec::new(),
                method_names: vec![method_name],
                method_effects: Vec::new(),
                associated_types: Vec::new(),
                span: Span::default(),
            });
        }
        env
    }

    fn wanted(class_name: Identifier, type_args: Vec<InferType>) -> WantedClassConstraint {
        WantedClassConstraint {
            class_name,
            class_id: crate::types::class_id::ClassId::from_local_name(class_name),
            type_args,
            span: Span::new(Position::new(1, 0), Position::new(1, 4)),
            origin: WantedClassConstraintOrigin::MethodCall,
        }
    }

    /// Stage 4: a solved predicate names the instance that discharged it, so
    /// dictionary elaboration does not have to resolve a second time and reach
    /// a different answer.
    #[test]
    fn a_solved_predicate_records_the_instance_that_discharged_it() {
        let mut interner = Interner::new();
        let int_head = named(&mut interner, "Int", vec![]);
        let class_env = env_with_instances(&mut interner, &[int_head]);
        let class_name = interner.intern("Sizeable");

        let outcome = solve_class_constraints_dispositioned(
            &[wanted(
                class_name,
                vec![InferType::Con(TypeConstructor::Int)],
            )],
            SolveScope::WholeProgram,
            &class_env,
            &interner,
        );

        let [entry] = &outcome.dispositions[..] else {
            panic!("expected exactly one disposition");
        };
        match &entry.disposition {
            Disposition::Solved {
                evidence: Evidence::FromInstance { instance, .. },
            } => {
                assert_eq!(instance.class_id.name, class_name);
                assert_eq!(
                    instance.head_type_args,
                    vec![InferType::Con(TypeConstructor::Int)]
                );
            }
            other => panic!("expected FromInstance evidence, got {other:?}"),
        }
    }

    /// Overlap detection counts candidates, so it cannot depend on the order
    /// instances happen to arrive in — they are read from `.flxi` files in no
    /// guaranteed order. GHC needed a dedicated mechanism to hold this
    /// property; this test is what keeps it true here.
    #[test]
    fn overlap_detection_does_not_depend_on_instance_order() {
        let verdict = |reversed: bool| {
            let mut interner = Interner::new();
            let list_a = {
                let a = named(&mut interner, "a", vec![]);
                named(&mut interner, "List", vec![a])
            };
            let list_int = {
                let int = named(&mut interner, "Int", vec![]);
                named(&mut interner, "List", vec![int])
            };
            let mut heads = vec![list_a, list_int];
            if reversed {
                heads.reverse();
            }
            let class_env = env_with_instances(&mut interner, &heads);
            let class_name = interner.intern("Sizeable");

            let outcome = solve_class_constraints_dispositioned(
                &[wanted(
                    class_name,
                    vec![InferType::App(
                        TypeConstructor::List,
                        vec![InferType::Con(TypeConstructor::Int)],
                    )],
                )],
                SolveScope::WholeProgram,
                &class_env,
                &interner,
            );
            matches!(
                outcome.dispositions[0].disposition,
                Disposition::Diagnosed { .. }
            )
        };

        assert!(verdict(false), "overlap should be reported");
        assert_eq!(
            verdict(false),
            verdict(true),
            "the verdict must not change when the instances are declared in the opposite order"
        );
    }
}
