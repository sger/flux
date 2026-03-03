//! Substitution map used by unification and type inference.

use crate::types::{TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType};
use std::collections::HashMap;

/// A substitution mapping type variables to types.
///
/// Invariant: the substitution is *idempotent* — no key appears in any value.
/// Use `compose` to merge substitutions while maintaining this invariant.
#[derive(Debug, Clone, Default)]
pub struct TypeSubst {
    type_bindings: HashMap<TypeVarId, InferType>,
    row_bindings: HashMap<TypeVarId, InferEffectRow>,
}

impl TypeSubst {
    /// The identity substitution (no mappings).
    pub fn empty() -> Self {
        TypeSubst {
            type_bindings: HashMap::new(),
            row_bindings: HashMap::new(),
        }
    }

    /// Look up a type variable.
    pub fn get_type(&self, type_var_id: TypeVarId) -> Option<&InferType> {
        self.type_bindings.get(&type_var_id)
    }

    /// Insert a new binding `type_var_id → infer_type`.
    ///
    /// Panics in debug builds if `infer_type` contains `type_var_id` (would violate idempotency).
    pub fn insert_type(&mut self, type_var_id: TypeVarId, infer_type: InferType) {
        debug_assert!(
            !infer_type.free_vars().contains(&type_var_id),
            "occurs check: inserting {type_var_id} -> {infer_type} would create infinite type"
        );
        self.type_bindings.insert(type_var_id, infer_type);
    }

    /// Look up an effect-row variable.
    pub fn get_row(&self, row_var_id: TypeVarId) -> Option<&InferEffectRow> {
        self.row_bindings.get(&row_var_id)
    }

    /// Insert a new row binding `row_var_id -> row`.
    ///
    /// The row value is stored as-is and normalized by consumers via
    /// `InferEffectRow::apply_row_subst` when composing substitutions.
    pub fn insert_row(&mut self, row_var_id: TypeVarId, row: InferEffectRow) {
        self.row_bindings.insert(row_var_id, row);
    }

    /// Backwards-compatible alias for [`Self::get_type`].
    pub fn get(&self, type_var_id: TypeVarId) -> Option<&InferType> {
        self.get_type(type_var_id)
    }

    /// Backwards-compatible alias for [`Self::insert_type`].
    pub fn insert(&mut self, type_var_id: TypeVarId, infer_type: InferType) {
        self.insert_type(type_var_id, infer_type);
    }

    /// Compose this substitution with `other`.
    ///
    /// `compose(s1, s2)` produces a substitution equivalent to first applying
    /// `s2`, then applying `s1` to the result — i.e. `s1 ∘ s2`.
    ///
    /// If both substitutions contain the same key, this substitution's binding
    /// is retained.
    pub fn compose(mut self, other: &TypeSubst) -> TypeSubst {
        // Step 1: merge type bindings from `other` into `self`.
        // We first rewrite each `other` value through `self` so transitive links are
        // collapsed early (e.g. {a -> b} composed with {b -> Int} can immediately
        // produce {b -> Int} in the merged view).
        // Existing keys in `self` win by design to preserve left-bias of composition.
        for (type_var_id, infer_type) in &other.type_bindings {
            if !self.type_bindings.contains_key(type_var_id) {
                let applied = infer_type.apply_type_subst(&self);
                self.type_bindings.insert(*type_var_id, applied);
            }
        }

        // Step 2: same merge strategy for row bindings.
        // `apply_row_subst` follows row-tail chains and unions concrete effects,
        // so each imported row binding is normalized against what `self` already knows.
        for (row_var_id, row) in &other.row_bindings {
            if !self.row_bindings.contains_key(row_var_id) {
                let applied = row.apply_row_subst(&self);
                self.row_bindings.insert(*row_var_id, applied);
            }
        }

        // Step 3: enforce idempotency for all type values now present in `self`.
        // Even if a binding originated in `self`, newly merged keys can unlock
        // additional rewrites, so we re-run substitution over every stored value.
        let keys: Vec<TypeVarId> = self.type_bindings.keys().copied().collect();

        for key in keys {
            if let Some(infer_type) = self.type_bindings.get(&key).cloned() {
                let applied = infer_type.apply_type_subst(&self);
                self.type_bindings.insert(key, applied);
            }
        }

        // Step 4: apply the same normalization pass for row values.
        // This keeps row substitutions stable after composition and prevents
        // stale tail chains from surviving in stored bindings.
        let row_keys: Vec<TypeVarId> = self.row_bindings.keys().copied().collect();

        for key in row_keys {
            if let Some(row) = self.row_bindings.get(&key).cloned() {
                let applied = row.apply_row_subst(&self);
                self.row_bindings.insert(key, applied);
            }
        }
        self
    }

    /// Returns `true` if neither type nor row bindings are present.
    pub fn is_empty(&self) -> bool {
        self.type_bindings.is_empty() && self.row_bindings.is_empty()
    }

    /// Iterate over all bindings as `(type_var_id, inferred_type)` pairs.
    ///
    /// Iteration order is unspecified.
    pub fn iter(&self) -> impl Iterator<Item = (TypeVarId, &InferType)> {
        self.type_bindings.iter().map(|(k, v)| (*k, v))
    }

    /// Iterate over all row bindings as `(row_var_id, inferred_row)` pairs.
    ///
    /// Iteration order is unspecified.
    pub fn iter_rows(&self) -> impl Iterator<Item = (TypeVarId, &InferEffectRow)> {
        self.row_bindings.iter().map(|(k, v)| (*k, v))
    }
}

#[cfg(test)]
mod tests {
    use super::TypeSubst;
    use crate::types::{
        TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType,
        type_constructor::TypeConstructor,
    };

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
        assert!(type_subst.get_row(0).is_none());
        assert_eq!(type_subst.iter().count(), 0);
        assert_eq!(type_subst.iter_rows().count(), 0);
    }

    #[test]
    fn row_only_substitution_is_not_empty() {
        let mut type_subst = TypeSubst::empty();
        type_subst.insert_row(3, InferEffectRow::closed_empty());

        assert!(!type_subst.is_empty());
        assert!(type_subst.get(3).is_none());
        assert!(type_subst.get_row(3).is_some());
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
