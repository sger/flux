//! Numeric defaulting.
//!
//! The mechanics the quantification decision leans on: applying the current
//! substitution to a binding's wanted predicates, and the Haskell Report's
//! defaulting rule for a truly ambiguous numeric variable. The decision that
//! *uses* them lives in [`crate::types::quantify`].

use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin},
    syntax::interner::Interner,
    types::{
        TypeVarId, class_env::ClassEnv, infer_type::InferType, type_constructor::TypeConstructor,
        type_subst::TypeSubst,
    },
};

pub(crate) fn apply_wanted_constraints_subst(
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
pub(crate) fn build_numeric_default_subst(
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
