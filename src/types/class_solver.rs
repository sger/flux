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
    ast::type_infer::constraint::{WantedClassConstraint, WantedClassConstraintOrigin},
    diagnostics::position::Span,
    diagnostics::{Diagnostic, DiagnosticBuilder, diagnostic_for},
    syntax::{Identifier, interner::Interner, type_expr::TypeExpr},
    types::{
        TypeVarId,
        class_disposition::{
            Disposition, DispositionedConstraint, Evidence, SolveOutcome, SolveScope, StuckReason,
        },
        class_env::ClassEnv,
        infer_type::InferType,
        type_constructor::TypeConstructor,
    },
};

use super::super::diagnostics::compiler_errors::{
    NO_INSTANCE, OVERLAPPING_INSTANCES, UNDETERMINED_CLASS_PARAMETER,
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
            disposition: classify_constraint(constraint, scope, class_env, interner),
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
    scope: SolveScope,
    class_env: &ClassEnv,
    interner: &Interner,
) -> Disposition {
    // Operator constraints that originated from unresolved type variables
    // should not become standalone missing-instance diagnostics just
    // because later inference happened to concretize them.
    if constraint.origin == WantedClassConstraintOrigin::InferredOperator
        && !constraint.originated_from_concrete_type
    {
        return Disposition::Stuck {
            reason: StuckReason::NonConcreteOperator,
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

    // Only check concrete types by default — variables are left unsolved
    // for now. Function-shaped type arguments are already specific enough
    // to reject for marker classes like `Sendable`, even if their inner
    // parameter/return slots still contain variables.
    if !constraint.type_args.iter().all(is_solvable_type_arg) {
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
    if class_env.lookup_class(constraint.class_name).is_none() {
        return Disposition::Stuck {
            reason: StuckReason::UnknownClass,
        };
    }

    let has_matching_instance = has_satisfied_instance(
        constraint.class_name,
        &constraint.type_args,
        class_env,
        interner,
        &mut HashSet::new(),
    );

    // Two or more instances matching the same predicate would make evidence
    // selection depend on declaration order. Report it rather than silently
    // taking the first (Proposal 0179 Stage 3).
    let candidates = class_env
        .candidate_instances(constraint.class_name, &constraint.type_args, interner)
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

    if has_matching_instance {
        // Stage 4 replaces this placeholder with the instance and
        // substitution that `resolve_instance_with_subst` already computes,
        // so dictionary elaboration can stop re-resolving from scratch.
        return Disposition::Solved {
            evidence: Evidence::Unrecorded,
        };
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

fn has_satisfied_instance(
    class_name: Identifier,
    type_args: &[InferType],
    class_env: &ClassEnv,
    interner: &Interner,
    seen: &mut HashSet<String>,
) -> bool {
    let key = format!(
        "{}<{}>",
        interner.resolve(class_name),
        type_args
            .iter()
            .map(|ty| display_type(ty, interner))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !seen.insert(key.clone()) {
        return true;
    }

    let result = has_structural_builtin_instance(class_name, type_args, class_env, interner, seen)
        || class_env
            .resolve_instance_with_subst(class_name, type_args, interner)
            .is_some_and(|(instance, subst)| {
                instance.context.iter().all(|ctx| {
                    let resolved_args: Option<Vec<InferType>> = ctx
                        .type_args
                        .iter()
                        .map(|arg| instantiate_context_type_expr(arg, &subst, interner))
                        .collect();
                    resolved_args.is_some_and(|args| {
                        args.iter().all(is_concrete_type)
                            && has_satisfied_instance(
                                ctx.class_name,
                                &args,
                                class_env,
                                interner,
                                seen,
                            )
                    })
                })
            });

    seen.remove(&key);
    result
}

fn has_structural_builtin_instance(
    class_name: Identifier,
    type_args: &[InferType],
    class_env: &ClassEnv,
    interner: &Interner,
    seen: &mut HashSet<String>,
) -> bool {
    let class_name = interner.resolve(class_name);
    if !matches!(class_name, "Eq" | "Ord" | "Sendable") || type_args.len() != 1 {
        return false;
    }

    match &type_args[0] {
        InferType::Tuple(elements) => elements.iter().all(|elem| {
            has_satisfied_instance_for_single(class_name, elem, class_env, interner, seen)
        }),
        InferType::App(TypeConstructor::Option, args)
        | InferType::App(TypeConstructor::List, args)
        | InferType::App(TypeConstructor::Array, args) => args.first().is_some_and(|arg| {
            has_satisfied_instance_for_single(class_name, arg, class_env, interner, seen)
        }),
        // `Sendable<Map<k, v>>` requires both the keys and values to be
        // sendable. `Eq` and `Ord` are not currently auto-derived for `Map`
        // (the existing rules only cover `Option`/`List`/`Array`), so this
        // arm only fires for `Sendable`.
        InferType::App(TypeConstructor::Map, args) if class_name == "Sendable" => {
            args.iter().all(|arg| {
                has_satisfied_instance_for_single(class_name, arg, class_env, interner, seen)
            })
        }
        InferType::App(TypeConstructor::Either, args) => args.iter().all(|arg| {
            has_satisfied_instance_for_single(class_name, arg, class_env, interner, seen)
        }),
        _ => false,
    }
}

fn has_satisfied_instance_for_single(
    class_name: &str,
    ty: &InferType,
    class_env: &ClassEnv,
    interner: &Interner,
    seen: &mut HashSet<String>,
) -> bool {
    interner.lookup(class_name).is_some_and(|class_id| {
        has_satisfied_instance(
            class_id,
            std::slice::from_ref(ty),
            class_env,
            interner,
            seen,
        )
    })
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
        .instances_matching_known_args(constraint.class_name, &known, interner)
        .take(2)
        .count();
    if candidates < 2 {
        return None;
    }

    let class_def = class_env.lookup_class(constraint.class_name)?;
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
