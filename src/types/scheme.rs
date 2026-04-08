//! Polymorphic type schemes for Hindley-Milner inference.
//!
//! A [`Scheme`] pairs a type body with a list of universally quantified
//! type-variable IDs. It is the representation used for let-polymorphism:
//! generalized bindings store schemes in the typing environment, and each use
//! site instantiates that scheme with fresh variables.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    ast::type_infer::constraint::SchemeConstraint,
    syntax::{Identifier, symbol::Symbol},
    types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType, type_subst::TypeSubst,
    },
};

/// A type scheme — a type with universally quantified type variables.
///
/// `∀ a b. a -> b -> a` is represented as:
/// ```text
/// Scheme { forall: [0, 1], ty: Fun([Var(0), Var(1)], Var(0)) }
/// ```
///
/// Conceptually:
/// - `forall` contains bound variables (universally quantified)
/// - `infer_type` is the body where those variables may appear
///
/// During inference:
/// - `generalize` computes a scheme from an inferred monotype
/// - `instantiate` creates a fresh monotype each time the scheme is used
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheme {
    /// Universally quantified type variables (the `∀` binders).
    pub forall: Vec<TypeVarId>,
    /// Class constraints on quantified type variables.
    ///
    /// E.g., `forall a. Eq<a> => a -> a -> Bool` has one constraint
    /// `SchemeConstraint { class_name: Eq, type_var: <id of a> }`.
    ///
    /// Used by dictionary elaboration (Proposal 0145, Step 5b) to determine
    /// which dictionary parameters a polymorphic function requires.
    #[serde(default)]
    pub constraints: Vec<SchemeConstraint>,
    /// Scheme body type.
    ///
    /// This may contain `Var(v)` for variables listed in `forall`, as well as
    /// other free variables not bound by this scheme.
    pub infer_type: InferType,
}

impl Scheme {
    /// Collect all `Symbol`s referenced by this scheme's type body.
    pub fn collect_symbols(&self, out: &mut HashSet<Symbol>) {
        self.infer_type.collect_symbols(out);
    }

    /// Replace Symbol IDs according to `remap`. Returns a new scheme.
    pub fn remap_symbols(&self, remap: &HashMap<Symbol, Symbol>) -> Self {
        Scheme {
            forall: self.forall.clone(),
            constraints: self.constraints.clone(),
            infer_type: self.infer_type.remap_symbols(remap),
        }
    }

    /// Constructs a monomorphic scheme (`forall = []`).
    ///
    /// Use this when a binding should remain monomorphic in the environment.
    pub fn mono(infer_type: InferType) -> Self {
        Scheme {
            forall: Vec::new(),
            constraints: Vec::new(),
            infer_type,
        }
    }

    /// Instantiates this scheme into a monotype.
    ///
    /// Each quantified variable in `forall` is replaced with a fresh
    /// [`InferType::Var`] using `counter`, which is advanced as fresh IDs are
    /// allocated.
    ///
    /// Returns the instantiated type and the mapping from old to new vars.
    ///
    /// The returned mapping is useful for debugging and tests; inference usually
    /// only needs the instantiated type.
    pub fn instantiate(&self, counter: &mut u32) -> (InferType, HashMap<TypeVarId, TypeVarId>, Vec<SchemeConstraint>) {
        let mut mapping: HashMap<TypeVarId, TypeVarId> = HashMap::new();

        for &v in &self.forall {
            let fresh = *counter;
            *counter += 1;
            mapping.insert(v, fresh);
        }

        // Determine which quantified vars appear only as row-tail variables.
        // Row vars are in free_vars() but not in free_type_vars() — we only
        // insert row bindings for those, avoiding spurious row substitutions
        // for plain type variables.
        let type_vars = self.infer_type.free_type_vars();

        let type_subst: TypeSubst = {
            let mut s = TypeSubst::empty();
            for (&old, &new) in &mapping {
                s.insert(old, InferType::Var(new));
                if !type_vars.contains(&old) {
                    // `old` appears only as a row-tail variable in the body.
                    s.insert_row(
                        old,
                        InferEffectRow::open_from_symbols(std::iter::empty::<Identifier>(), new),
                    );
                }
            }
            s
        };

        let constraints = self
            .constraints
            .iter()
            .map(|c| SchemeConstraint {
                class_name: c.class_name,
                type_vars: c.type_vars.iter().map(|v| mapping.get(v).copied().unwrap_or(*v)).collect(),
            })
            .collect();

        (self.infer_type.apply_type_subst(&type_subst), mapping, constraints)
    }

    /// Returns free type variables in the body that are not quantified by
    /// `forall`.
    ///
    /// This corresponds to `FV(∀a. t) = FV(t) - {a}`.
    pub fn free_vars(&self) -> HashSet<TypeVarId> {
        let forall_set: HashSet<TypeVarId> = self.forall.iter().copied().collect();
        self.infer_type
            .free_vars()
            .difference(&forall_set)
            .copied()
            .collect()
    }
}

/// Generalize a type over all type variables that are free in `ty` but not
/// free anywhere in `env_free_vars` (the environment's free variables).
///
/// This is the "let-generalization" step of Algorithm W:
/// - Variables free in the env are *monomorphic* (they might be constrained by
///   the surrounding context).
/// - Variables free in `ty` but not the env are truly polymorphic.
///
/// Effect-row tail variables (`|e`) are `TypeVarId`s on `InferEffectRow` and
/// are included via `free_vars()` → `collect_row_free_vars`, so they participate
/// in `forall` quantification alongside regular type variables.
pub fn generalize(infer_type: &InferType, env_free_vars: &HashSet<TypeVarId>) -> Scheme {
    let mut free: Vec<TypeVarId> = infer_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();
    // Keep quantifier order stable for reproducible diagnostics and tests.
    free.sort_unstable();
    Scheme {
        forall: free,
        constraints: Vec::new(),
        infer_type: infer_type.clone(),
    }
}

/// Generalize a type with class constraints.
///
/// Like [`generalize`], but also attaches class constraints on quantified
/// variables. Constraints whose `type_var` is not in the `forall` set
/// (i.e., the type was resolved to a concrete type) are discarded — they
/// have already been validated by `solve_class_constraints`.
pub fn generalize_with_constraints(
    infer_type: &InferType,
    env_free_vars: &HashSet<TypeVarId>,
    constraints: Vec<SchemeConstraint>,
) -> Scheme {
    let mut free: Vec<TypeVarId> = infer_type
        .free_vars()
        .difference(env_free_vars)
        .copied()
        .collect();
    free.sort_unstable();
    let forall_set: HashSet<TypeVarId> = free.iter().copied().collect();
    let filtered: Vec<SchemeConstraint> = constraints
        .into_iter()
        .filter(|c| c.type_vars.iter().all(|v| forall_set.contains(v)))
        .collect();
    Scheme {
        forall: free,
        constraints: filtered,
        infer_type: infer_type.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{Scheme, generalize};
    use crate::types::{
        infer_effect_row::InferEffectRow, infer_type::InferType, type_constructor::TypeConstructor,
    };

    fn infer_var(id: u32) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    #[test]
    fn generalize_sorts_forall_deterministically() {
        let infer_type = InferType::Fun(
            vec![
                infer_var(2),
                infer_var(0),
                InferType::Tuple(vec![infer_var(1), infer_var(2)]),
            ],
            Box::new(infer_var(0)),
            InferEffectRow::closed_empty(),
        );

        let scheme = generalize(&infer_type, &HashSet::new());
        assert_eq!(scheme.forall, vec![0, 1, 2]);
    }

    #[test]
    fn instantiate_is_fresh_and_avoids_capture_of_free_vars() {
        let scheme = Scheme {
            forall: vec![0],
            constraints: vec![],
            // ?1 is free in the scheme body (not quantified) and must remain unchanged.
            infer_type: InferType::Fun(
                vec![infer_var(0)],
                Box::new(infer_var(1)),
                InferEffectRow::closed_empty(),
            ),
        };

        let mut counter = 10;
        let (instantiated, mapping, _constraints) = scheme.instantiate(&mut counter);

        assert_eq!(mapping.get(&0), Some(&10));
        assert_eq!(counter, 11);
        assert_eq!(
            instantiated,
            InferType::Fun(
                vec![infer_var(10)],
                Box::new(infer_var(1)),
                InferEffectRow::closed_empty()
            )
        );
    }

    #[test]
    fn instantiate_produces_distinct_fresh_vars_per_call() {
        let scheme = Scheme {
            forall: vec![0, 1],
            constraints: vec![],
            infer_type: InferType::Fun(
                vec![infer_var(0)],
                Box::new(infer_var(1)),
                InferEffectRow::closed_empty(),
            ),
        };

        let mut counter = 20;
        let (first, first_mapping, _) = scheme.instantiate(&mut counter);
        let (second, second_mapping, _) = scheme.instantiate(&mut counter);

        assert_eq!(first_mapping.get(&0), Some(&20));
        assert_eq!(first_mapping.get(&1), Some(&21));
        assert_eq!(second_mapping.get(&0), Some(&22));
        assert_eq!(second_mapping.get(&1), Some(&23));
        assert_eq!(counter, 24);
        assert_eq!(
            first,
            InferType::Fun(
                vec![infer_var(20)],
                Box::new(infer_var(21)),
                InferEffectRow::closed_empty()
            )
        );
        assert_eq!(
            second,
            InferType::Fun(
                vec![infer_var(22)],
                Box::new(infer_var(23)),
                InferEffectRow::closed_empty()
            )
        );
    }

    #[test]
    fn generalize_excludes_env_free_vars() {
        let infer_type = InferType::Fun(
            vec![infer_var(0), infer_var(1), int()],
            Box::new(infer_var(2)),
            InferEffectRow::closed_empty(),
        );
        let env_free_vars = HashSet::from([1, 42]);

        let scheme = generalize(&infer_type, &env_free_vars);

        assert_eq!(scheme.forall, vec![0, 2]);
        assert_eq!(scheme.infer_type, infer_type);
    }

    #[test]
    fn collect_symbols_delegates_to_body() {
        use crate::syntax::symbol::Symbol;

        let adt_sym = Symbol::new(7);
        let scheme = Scheme {
            forall: vec![0],
            constraints: vec![],
            infer_type: InferType::App(TypeConstructor::Adt(adt_sym), vec![infer_var(0)]),
        };

        let mut out = HashSet::new();
        scheme.collect_symbols(&mut out);
        assert!(out.contains(&adt_sym));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn remap_symbols_rewrites_body_and_preserves_forall() {
        use crate::syntax::symbol::Symbol;
        use std::collections::HashMap;

        let old_sym = Symbol::new(5);
        let new_sym = Symbol::new(50);
        let scheme = Scheme {
            forall: vec![0, 1],
            constraints: vec![],
            infer_type: InferType::Fun(
                vec![InferType::App(
                    TypeConstructor::Adt(old_sym),
                    vec![infer_var(0)],
                )],
                Box::new(infer_var(1)),
                InferEffectRow::closed_empty(),
            ),
        };

        let remap = HashMap::from([(old_sym, new_sym)]);
        let remapped = scheme.remap_symbols(&remap);

        assert_eq!(remapped.forall, vec![0, 1]);
        let expected_type = InferType::Fun(
            vec![InferType::App(
                TypeConstructor::Adt(new_sym),
                vec![infer_var(0)],
            )],
            Box::new(infer_var(1)),
            InferEffectRow::closed_empty(),
        );
        assert_eq!(remapped.infer_type, expected_type);
    }
}
