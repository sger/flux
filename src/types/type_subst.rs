//! Substitution map used by unification and type inference.

use crate::types::{TypeVarId, infer_effect_row::InferEffectRow, infer_type::InferType};
use std::collections::HashMap;

/// A substitution mapping type variables to types.
///
/// Uses **lazy normalization**: stored values may contain variable references
/// that are themselves mapped in the substitution. Full resolution happens
/// at read time via `apply_type_subst`, which recursively follows chains.
///
/// The acyclicity invariant (enforced by the occurs check in unification)
/// guarantees that chain-following always terminates.
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
    pub fn get(&self, type_var_id: TypeVarId) -> Option<&InferType> {
        self.type_bindings.get(&type_var_id)
    }

    /// Insert a new binding `type_var_id → infer_type`.
    ///
    /// Under lazy normalization, values may contain other mapped variables.
    /// Full normalization is performed by `apply_type_subst` on lookup.
    ///
    /// Panics in debug builds if `infer_type` directly references `type_var_id`
    /// (self-reference would create an infinite chain). The full occurs check
    /// (preventing `?a → Foo<?a>`) is enforced by `unify_with_span` before
    /// any `insert` call.
    pub fn insert(&mut self, type_var_id: TypeVarId, infer_type: InferType) {
        debug_assert!(
            !matches!(&infer_type, InferType::Var(v) if *v == type_var_id),
            "occurs check: inserting {type_var_id} -> {infer_type} would create a self-referential chain"
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

    /// Compose this substitution with `other`.
    ///
    /// `compose(s1, s2)` produces a substitution equivalent to first applying
    /// `s2`, then applying `s1` to the result — i.e. `s1 ∘ s2`.
    ///
    /// If both substitutions contain the same key, this substitution's binding
    /// is retained.
    ///
    /// Uses lazy normalization: incoming values from `other` are applied through
    /// `self` to collapse one level of transitive links, but stored values in
    /// `self` are **not** re-walked. Full resolution happens at read time via
    /// `apply_type_subst`, which recursively follows variable chains.
    pub fn compose(mut self, other: &TypeSubst) -> TypeSubst {
        // Merge type bindings from `other` into `self`.
        // Each `other` value is rewritten through `self` so transitive links are
        // collapsed for the incoming entry (e.g. {a -> b} composed with {b -> Int}
        // produces {b -> Int} in the merged view).
        // Existing keys in `self` win by design to preserve left-bias of composition.
        for (type_var_id, infer_type) in &other.type_bindings {
            if !self.type_bindings.contains_key(type_var_id) {
                let applied = infer_type.apply_type_subst(&self);
                self.type_bindings.insert(*type_var_id, applied);
            }
        }

        // Same merge strategy for row bindings.
        // `apply_row_subst` follows row-tail chains and unions concrete effects,
        // so each imported row binding is normalized against what `self` already knows.
        for (row_var_id, row) in &other.row_bindings {
            if !self.row_bindings.contains_key(row_var_id) {
                let applied = row.apply_row_subst(&self);
                self.row_bindings.insert(*row_var_id, applied);
            }
        }

        // Drop trivial self-bindings (?t -> ?t), which are no-ops and can
        // otherwise participate in substitution cycles.
        self.type_bindings
            .retain(|key, infer_type| !matches!(infer_type, InferType::Var(v) if v == key));

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
    fn compose_resolves_chains_on_lookup() {
        let mut left = TypeSubst::empty();
        left.insert(0, var(1));

        let mut right = TypeSubst::empty();
        right.insert(1, int());

        let composed = left.compose(&right);

        // Observable: apply_type_subst follows chains to full resolution.
        assert_eq!(var(0).apply_type_subst(&composed), int());
        assert_eq!(var(1).apply_type_subst(&composed), int());
        // Direct get(1) returns Int (from right, applied through self).
        assert_eq!(composed.get(1), Some(&int()));
    }

    #[test]
    fn compose_drops_trivial_self_bindings() {
        let mut left = TypeSubst::empty();
        left.insert(0, var(0));

        let composed = left.compose(&TypeSubst::empty());

        assert!(composed.get(0).is_none());
    }
}
