use std::collections::{HashMap, HashSet};

use crate::{
    ast::type_infer::constraint::{
        SchemeConstraint, WantedClassConstraint, WantedClassConstraintOrigin,
    },
    diagnostics::Diagnostic,
    syntax::interner::Interner,
    types::{
        TypeVarId, class_env::ClassEnv, class_solver::solve_class_constraints,
        infer_type::InferType, type_constructor::TypeConstructor, type_subst::TypeSubst,
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
    let diagnostics = class_env
        .map(|env| solve_class_constraints(&finalized_constraints, env, interner))
        .unwrap_or_default();
    let scheme_constraints =
        collect_scheme_constraints(&finalized_constraints, &finalized_type, env_free_vars);

    FinalizedBindingClassConstraints {
        infer_type: finalized_type,
        scheme_constraints,
        default_subst,
        diagnostics,
    }
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

fn collect_scheme_constraints(
    constraints: &[WantedClassConstraint],
    infer_type: &InferType,
    env_free_vars: &HashSet<TypeVarId>,
) -> Vec<SchemeConstraint> {
    let ty_free: HashSet<TypeVarId> = infer_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for constraint in constraints {
        if constraint.origin == WantedClassConstraintOrigin::InferredOperator {
            continue;
        }
        let vars: Vec<TypeVarId> = constraint
            .type_args
            .iter()
            .filter_map(|ty| match ty {
                InferType::Var(var) => Some(*var),
                _ => None,
            })
            .collect();
        if vars.len() == constraint.type_args.len()
            && vars.iter().all(|var| ty_free.contains(var))
            && seen.insert((constraint.class_name, vars.clone()))
        {
            result.push(SchemeConstraint {
                class_name: constraint.class_name,
                type_vars: vars,
            });
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::finalize_binding_class_constraints;
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
        );

        assert!(finalized.default_subst.is_empty());
        assert_eq!(finalized.scheme_constraints.len(), 1);
        assert_eq!(finalized.scheme_constraints[0].type_vars, vec![0]);
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
        );

        assert!(
            finalized.diagnostics.is_empty(),
            "defaulted Num<Int> should validate cleanly, got: {:?}",
            finalized.diagnostics
        );
    }
}
