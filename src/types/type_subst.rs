//! Substitution map used by unification and type inference.

use std::collections::HashMap;

use crate::types::{TypeVarId, infer_type::InferType};

/// A substitution mapping type variables to types.
///
/// Invariant: the substitution is *idempotent* — no key appears in any value.
/// Use `compose` to merge substitutions while maintaining this invariant.
#[derive(Debug, Clone, Default)]
pub struct TypeSubst(HashMap<TypeVarId, InferType>);

impl TypeSubst {
    /// The identity substitution (no mappings).
    pub fn empty() -> Self {
        TypeSubst(HashMap::new())
    }

    /// Look up a type variable.
    pub fn get(&self, type_var_id: TypeVarId) -> Option<&InferType> {
        self.0.get(&type_var_id)
    }

    /// Insert a new binding `type_var_id → infer_type`.
    ///
    /// Panics in debug builds if `infer_type` contains `type_var_id` (would violate idempotency).
    pub fn insert(&mut self, type_var_id: TypeVarId, infer_type: InferType) {
        debug_assert!(
            !infer_type.free_vars().contains(&type_var_id),
            "occurs check: inserting {type_var_id} -> {infer_type} would create infinite type"
        );
        self.0.insert(type_var_id, infer_type);
    }

    /// Compose this substitution with `other`.
    ///
    /// `compose(s1, s2)` produces a substitution equivalent to first applying
    /// `s2`, then applying `s1` to the result — i.e. `s1 ∘ s2`.
    ///
    /// If both substitutions contain the same key, this substitution's binding
    /// is retained.
    pub fn compose(mut self, other: &TypeSubst) -> TypeSubst {
        // Apply self to all values in other, then add them (don't overwrite existing self bindings)
        for (type_var_id, infer_type) in &other.0 {
            if !self.0.contains_key(type_var_id) {
                let applied = infer_type.apply_type_subst(&self);
                self.0.insert(*type_var_id, applied);
            }
        }

        // Re-apply to all values to maintain idempotency
        let keys: Vec<TypeVarId> = self.0.keys().copied().collect();

        for key in keys {
            if let Some(infer_type) = self.0.get(&key).cloned() {
                let applied = infer_type.apply_type_subst(&self);
                self.0.insert(key, applied);
            }
        }
        self
    }

    /// Returns `true` if no bindings are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over all bindings as `(type_var_id, inferred_type)` pairs.
    ///
    /// Iteration order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = (TypeVarId, &InferType)> {
        self.0.iter().map(|(k, v)| (*k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::TypeSubst;
    use crate::types::{TypeVarId, infer_type::InferType, type_constructor::TypeConstructor};

    fn var(id: TypeVarId) -> InferType {
        InferType::Var(id)
    }

    fn int() -> InferType {
        InferType::Con(TypeConstructor::Int)
    }

    fn bool_t() -> InferType {
        InferType::Con(TypeConstructor::Bool)
    }

    #[test]
    fn empty_substitution_has_no_bindings() {
        let type_subst = TypeSubst::empty();
        assert!(type_subst.is_empty());
        assert!(type_subst.get(0).is_none());
        assert_eq!(type_subst.iter().count(), 0);
    }

    #[test]
    fn insert_and_get_binding() {
        let mut type_subst = TypeSubst::empty();
        type_subst.insert(7, int());

        assert!(!type_subst.is_empty());
        assert_eq!(type_subst.get(7), Some(&int()));
        assert!(type_subst.get(8).is_none());
    }

    #[test]
    fn compose_applies_left_to_right_values() {
        let mut left = TypeSubst::empty();
        left.insert(0, int());

        let mut right = TypeSubst::empty();
        right.insert(1, var(0));

        let composed = left.compose(&right);

        assert_eq!(composed.get(0), Some(&int()));
        assert_eq!(composed.get(1), Some(&int()));
    }

    #[test]
    fn compose_does_not_overwrite_existing_left_binding() {
        let mut left = TypeSubst::empty();
        left.insert(0, int());

        let mut right = TypeSubst::empty();
        right.insert(0, bool_t());

        let composed = left.compose(&right);

        assert_eq!(composed.get(0), Some(&int()));
    }

    #[test]
    fn compose_reapplies_to_keep_values_idempotent() {
        let mut left = TypeSubst::empty();
        left.insert(0, var(1));

        let mut right = TypeSubst::empty();
        right.insert(1, int());

        let composed = left.compose(&right);

        assert_eq!(composed.get(0), Some(&int()));
        assert_eq!(composed.get(1), Some(&int()));
        for (_, infer_type) in composed.iter() {
            assert!(infer_type.free_vars().is_empty());
        }
    }
}
